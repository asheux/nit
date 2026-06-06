use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, Weak};
use std::thread;
use std::time::{Duration, Instant};

use super::worker::{PoolWorker, WorkerInner, WorkerKey, WorkerSpawn};
use super::{POOL_GC_INTERVAL, POOL_IDLE_MAX};

pub struct ClaudePool {
    cap: usize,
    state: Mutex<PoolState>,
    available: Condvar,
    next_id: AtomicU64,
    shutting_down: AtomicBool,
    spawner: Box<dyn WorkerSpawn>,
}

struct PoolState {
    idle: VecDeque<Box<WorkerInner>>,
    busy: usize,
}

enum IdleClaim {
    Reused(PoolWorker),
    Dead(Box<WorkerInner>),
    Empty,
}

impl ClaudePool {
    pub fn new(cap: usize, spawner: Box<dyn WorkerSpawn>) -> Arc<Self> {
        let cap = cap.max(1);
        let pool = Arc::new(Self {
            cap,
            state: Mutex::new(PoolState {
                idle: VecDeque::new(),
                busy: 0,
            }),
            available: Condvar::new(),
            next_id: AtomicU64::new(1),
            shutting_down: AtomicBool::new(false),
            spawner,
        });
        spawn_gc_thread(Arc::clone(&pool));
        pool
    }

    pub fn capacity(&self) -> usize {
        self.cap
    }

    pub fn snapshot_sizes(&self) -> (usize, usize) {
        let state = lock_recover(&self.state);
        (state.idle.len(), state.busy)
    }

    pub fn checkout(
        self: &Arc<Self>,
        key: &WorkerKey,
        agent_id: &str,
        timeout: Duration,
    ) -> Option<PoolWorker> {
        if self.shutting_down.load(Ordering::Acquire) {
            return None;
        }
        let deadline = Instant::now() + timeout;
        let mut state = self.state.lock().ok()?;
        loop {
            match self.take_idle(&mut state, key, agent_id) {
                IdleClaim::Reused(worker) => return Some(worker),
                IdleClaim::Dead(dead) => {
                    drop(state);
                    dead.shutdown();
                    state = self.state.lock().ok()?;
                    continue;
                }
                IdleClaim::Empty => {}
            }

            if state.busy + state.idle.len() < self.cap {
                state.busy = state.busy.saturating_add(1);
                drop(state);
                return self.spawn_or_refund(key, agent_id);
            }

            state = self.wait_for_slot(state, deadline)?;
        }
    }

    /// Claim the affinity-preferred idle slot for `key` while the state lock is
    /// held. `Reused` has already incremented `busy`; `Dead` is a slot that died
    /// while parked and must be shut down off-lock (it blocks) before the scan
    /// retries; `Empty` falls through to the spawn path.
    fn take_idle(
        self: &Arc<Self>,
        state: &mut PoolState,
        key: &WorkerKey,
        agent_id: &str,
    ) -> IdleClaim {
        let Some(idx) = find_idle_match(&state.idle, key, agent_id) else {
            return IdleClaim::Empty;
        };
        let mut candidate = state.idle.remove(idx).expect("idx valid");
        if candidate.dead.load(Ordering::Acquire) {
            return IdleClaim::Dead(candidate);
        }
        candidate.agent_affinity = agent_id.to_string();
        candidate.last_used_at = Instant::now();
        state.busy = state.busy.saturating_add(1);
        IdleClaim::Reused(PoolWorker::new(candidate, Arc::clone(self)))
    }

    fn spawn_or_refund(self: &Arc<Self>, key: &WorkerKey, agent_id: &str) -> Option<PoolWorker> {
        match self.spawner.spawn(key, agent_id) {
            Ok(spawned) => {
                let id = self.next_id.fetch_add(1, Ordering::Relaxed);
                let inner = WorkerInner::from_spawned(key.clone(), agent_id, spawned, id);
                Some(PoolWorker::new(inner, Arc::clone(self)))
            }
            Err(_err) => {
                // Reservation refund. `.ok()?` (not `lock_recover`) so a poisoned
                // lock skips the refund and propagates `None`, matching the original.
                let mut state = self.state.lock().ok()?;
                state.busy = state.busy.saturating_sub(1);
                self.available.notify_all();
                None
            }
        }
    }

    // `'g` keeps the returned guard bound to the same lock acquisition we waited on.
    fn wait_for_slot<'g>(
        &self,
        state: MutexGuard<'g, PoolState>,
        deadline: Instant,
    ) -> Option<MutexGuard<'g, PoolState>> {
        let now = Instant::now();
        if now >= deadline {
            return None;
        }
        let (state, timed_out) = self.available.wait_timeout(state, deadline - now).ok()?;
        if timed_out.timed_out() {
            return None;
        }
        Some(state)
    }

    pub fn shutdown(&self) {
        if self.shutting_down.swap(true, Ordering::AcqRel) {
            return;
        }
        let drained: Vec<Box<WorkerInner>> = lock_recover(&self.state).idle.drain(..).collect();
        for inner in drained {
            inner.shutdown();
        }
        self.available.notify_all();
        // The GC thread observes `shutting_down` on its next tick (~1s) and
        // exits on its own. Not joined here so shutdown stays prompt.
    }

    pub(crate) fn return_idle(self: Arc<Self>, mut inner: Box<WorkerInner>) {
        inner.detach_receiver();
        let alive = matches!(inner.child.try_wait(), Ok(None));
        let healthy = alive && !inner.dead.load(Ordering::Acquire);
        if !healthy || self.shutting_down.load(Ordering::Acquire) {
            self.destroy(inner);
            return;
        }
        inner.last_used_at = Instant::now();
        let mut state = lock_recover(&self.state);
        state.busy = state.busy.saturating_sub(1);
        state.idle.push_back(inner);
        self.available.notify_one();
    }

    // `Box<WorkerInner>` matches the slot type stored in the pool's idle
    // VecDeque, so callers can hand back boxes they popped without an
    // intermediate unbox step.
    #[allow(clippy::boxed_local)]
    pub(crate) fn destroy(self: Arc<Self>, inner: Box<WorkerInner>) {
        {
            let mut state = lock_recover(&self.state);
            state.busy = state.busy.saturating_sub(1);
        }
        (*inner).shutdown();
        self.available.notify_all();
    }

    pub fn gc_sweep(self: &Arc<Self>) {
        self.gc_sweep_at(Instant::now());
    }

    /// `gc_sweep` with an injectable "now", used by tests so they can step
    /// the clock forward without relying on `Instant::checked_sub` (which
    /// underflows on platforms where the monotonic clock is anchored at
    /// boot and the process is only seconds old — e.g. CI Windows runners).
    pub(crate) fn gc_sweep_at(self: &Arc<Self>, now: Instant) {
        let aged: VecDeque<Box<WorkerInner>> = {
            let mut state = lock_recover(&self.state);
            let (aged, keep): (VecDeque<_>, VecDeque<_>) = state.idle.drain(..).partition(|slot| {
                now.saturating_duration_since(slot.last_used_at) >= POOL_IDLE_MAX
                    || slot.dead.load(Ordering::Acquire)
            });
            state.idle = keep;
            aged
        };
        for slot in aged {
            slot.shutdown();
        }
    }
}

impl Drop for ClaudePool {
    fn drop(&mut self) {
        self.shutting_down.store(true, Ordering::Release);
        let drained: Vec<Box<WorkerInner>> = lock_recover(&self.state).idle.drain(..).collect();
        for inner in drained {
            inner.shutdown();
        }
    }
}

fn find_idle_match(
    idle: &VecDeque<Box<WorkerInner>>,
    key: &WorkerKey,
    agent_id: &str,
) -> Option<usize> {
    let mut compatible: Option<usize> = None;
    for (idx, slot) in idle.iter().enumerate().filter(|(_, slot)| &slot.key == key) {
        if slot.agent_affinity == agent_id {
            return Some(idx);
        }
        compatible.get_or_insert(idx);
    }
    compatible
}

fn spawn_gc_thread(pool: Arc<ClaudePool>) {
    let weak = Arc::downgrade(&pool);
    drop(pool);
    let _ = thread::Builder::new()
        .name("nit-claude-pool-gc".into())
        .spawn(move || gc_loop(weak));
}

fn gc_loop(weak: Weak<ClaudePool>) {
    // Tick every second so `shutdown()` is observed within ~1s, but only sweep
    // once per `POOL_GC_INTERVAL`; the pool stays alive only via `weak`, so an
    // upgrade failure means it was dropped and the thread should exit.
    let tick = Duration::from_secs(1);
    let mut since_sweep = Duration::ZERO;
    loop {
        thread::sleep(tick);
        since_sweep += tick;
        let Some(pool) = weak.upgrade() else { break };
        if pool.shutting_down.load(Ordering::Acquire) {
            break;
        }
        if since_sweep >= POOL_GC_INTERVAL {
            pool.gc_sweep();
            since_sweep = Duration::ZERO;
        }
    }
}

// Pool state stays consistent across panics, so a poisoned lock is recovered
// rather than propagated.
pub(crate) fn lock_recover<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    match mutex.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}
