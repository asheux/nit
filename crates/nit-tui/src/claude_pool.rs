//! Warm pool of long-lived `claude -p --input-format stream-json` workers.
//!
//! Gated behind `NIT_CLAUDE_POOL=1`. The default (unset or `=0`) keeps the
//! cold-spawn path in `claude_runner.rs` byte-identical to today's per-turn
//! subprocess behaviour — that path is the rollback story for at least one
//! release after the pool flag flips to default-on.
//!
//! ## Slot model
//!
//! Each slot owns:
//! - a long-lived `claude -p` child with persistent stdin
//! - a stdout reader thread that demultiplexes stream-json lines into a
//!   per-checkout [`crossbeam_channel`] bounded at [`POOL_LINE_CAPACITY`]
//! - a stderr collector capped at [`POOL_STDERR_CAP_BYTES`] (100 MB tail
//!   window, mirroring `STDOUT_TAIL_CAP_BYTES` in `claude_runner.rs`)
//!
//! Slots are keyed by [`WorkerKey`] — a turn can only reuse a slot whose
//! spawn-time CLI args match its own (model, cwd, read-only allowlist).
//! Agent-affinity is a secondary preference inside a key match: same-agent
//! reuse is preferred, then any compatible slot, then a fresh spawn.
//!
//! ## Failure isolation
//!
//! Every unhealthy condition (BrokenPipe on write, stream-json `error`
//! envelope, non-zero child exit, auth banner, operator cancel, idle
//! timeout, age GC) replaces the slot rather than returning it. The
//! premise is that a slot in an unknown state poisons the next turn; the
//! cost of a replacement (one extra spawn) is dwarfed by the cost of a
//! silently corrupted turn.

use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStderr, ChildStdin, ChildStdout};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crossbeam_channel::{bounded, Receiver, Sender};

pub const POOL_LINE_CAPACITY: usize = 64;
pub const POOL_STDERR_CAP_BYTES: usize = 100 * 1024 * 1024;
pub const POOL_IDLE_MAX: Duration = Duration::from_secs(60 * 60);
pub(crate) const POOL_GC_INTERVAL: Duration = Duration::from_secs(5 * 60);

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct WorkerKey {
    pub model_slug: String,
    pub cwd: PathBuf,
    pub read_only: bool,
}

impl WorkerKey {
    pub fn new(model_slug: impl Into<String>, cwd: impl Into<PathBuf>, read_only: bool) -> Self {
        Self {
            model_slug: model_slug.into(),
            cwd: cwd.into(),
            read_only,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecycleReason {
    BrokenPipe,
    StreamError,
    NonZeroExit,
    AuthExpired,
    Cancel,
    IdleTimeout,
    GcAged,
    Shutdown,
}

#[derive(Debug, Clone)]
pub struct PoolLine {
    pub raw: String,
    pub captured_at: Instant,
}

pub trait WorkerSpawn: Send + Sync + 'static {
    fn spawn(&self, key: &WorkerKey, agent_id: &str) -> std::io::Result<SpawnedChild>;
}

pub struct SpawnedChild {
    pub child: Child,
    pub stdin: ChildStdin,
    pub stdout: ChildStdout,
    pub stderr: ChildStderr,
}

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

struct WorkerInner {
    key: WorkerKey,
    agent_affinity: String,
    child: Child,
    stdin: Option<ChildStdin>,
    stderr: Arc<Mutex<Vec<u8>>>,
    demux: Arc<Mutex<Option<Sender<PoolLine>>>>,
    dead: Arc<AtomicBool>,
    last_used_at: Instant,
    stdout_join: Option<JoinHandle<()>>,
    stderr_join: Option<JoinHandle<()>>,
}

impl WorkerInner {
    fn shutdown(mut self) {
        // Tear down the demux first so the reader thread sees None and
        // stops trying to forward lines into a dead receiver.
        if let Ok(mut guard) = self.demux.lock() {
            *guard = None;
        }
        drop(self.stdin.take());
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Some(join) = self.stdout_join.take() {
            let _ = join.join();
        }
        if let Some(join) = self.stderr_join.take() {
            let _ = join.join();
        }
    }
}

pub struct PoolWorker {
    inner: Option<Box<WorkerInner>>,
    receiver: Receiver<PoolLine>,
    pool: Arc<ClaudePool>,
}

impl PoolWorker {
    pub fn key(&self) -> &WorkerKey {
        &self.inner_ref().key
    }

    pub fn agent_affinity(&self) -> &str {
        self.inner_ref().agent_affinity.as_str()
    }

    pub fn child_id(&self) -> u32 {
        self.inner_ref().child.id()
    }

    pub fn is_dead(&self) -> bool {
        self.inner
            .as_ref()
            .map(|i| i.dead.load(Ordering::Acquire))
            .unwrap_or(true)
    }

    pub fn write_envelope(&mut self, payload: &[u8]) -> std::io::Result<()> {
        let inner = self.inner.as_mut().expect("worker drained");
        let stdin = inner.stdin.as_mut().ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::BrokenPipe, "claude pool stdin closed")
        })?;
        stdin.write_all(payload)?;
        stdin.flush()
    }

    pub fn recv_timeout(&self, timeout: Duration) -> Option<PoolLine> {
        self.receiver.recv_timeout(timeout).ok()
    }

    pub fn kill_child(&mut self) {
        if let Some(inner) = self.inner.as_mut() {
            let _ = inner.child.kill();
            inner.dead.store(true, Ordering::Release);
        }
    }

    pub fn stderr_snapshot(&self) -> Vec<u8> {
        self.inner
            .as_ref()
            .and_then(|i| i.stderr.lock().ok().map(|g| g.clone()))
            .unwrap_or_default()
    }

    pub fn check_in(mut self) {
        if let Some(inner) = self.inner.take() {
            Arc::clone(&self.pool).return_idle(inner);
        }
    }

    pub fn recycle(mut self, _reason: RecycleReason) {
        if let Some(inner) = self.inner.take() {
            Arc::clone(&self.pool).destroy(inner);
        }
    }

    fn inner_ref(&self) -> &WorkerInner {
        self.inner.as_ref().expect("worker drained")
    }
}

impl Drop for PoolWorker {
    fn drop(&mut self) {
        if let Some(inner) = self.inner.take() {
            Arc::clone(&self.pool).destroy(inner);
        }
    }
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
        let state = match self.state.lock() {
            Ok(s) => s,
            Err(err) => err.into_inner(),
        };
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
            if let Some(idx) = find_idle_match(&state.idle, key, agent_id) {
                let mut inner = state.idle.remove(idx).expect("idx valid");
                if inner.dead.load(Ordering::Acquire) {
                    drop(state);
                    inner.shutdown();
                    state = self.state.lock().ok()?;
                    continue;
                }
                inner.agent_affinity = agent_id.to_string();
                inner.last_used_at = Instant::now();
                state.busy = state.busy.saturating_add(1);
                return Some(self.bind_receiver(inner));
            }

            if state.busy + state.idle.len() < self.cap {
                state.busy = state.busy.saturating_add(1);
                drop(state);
                match self.spawner.spawn(key, agent_id) {
                    Ok(spawned) => {
                        let inner = self.build_worker(key.clone(), agent_id, spawned);
                        return Some(self.bind_receiver(inner));
                    }
                    Err(_err) => {
                        let mut s = self.state.lock().ok()?;
                        s.busy = s.busy.saturating_sub(1);
                        self.available.notify_all();
                        return None;
                    }
                }
            }

            let now = Instant::now();
            if now >= deadline {
                return None;
            }
            let remaining = deadline - now;
            let (s, timed_out) = self.available.wait_timeout(state, remaining).ok()?;
            state = s;
            if timed_out.timed_out() {
                return None;
            }
        }
    }

    pub fn shutdown(&self) {
        if self.shutting_down.swap(true, Ordering::AcqRel) {
            return;
        }
        let drained: Vec<Box<WorkerInner>> = {
            let mut guard = match self.state.lock() {
                Ok(g) => g,
                Err(err) => err.into_inner(),
            };
            guard.idle.drain(..).collect()
        };
        for inner in drained {
            inner.shutdown();
        }
        self.available.notify_all();
        // The GC thread observes `shutting_down` on its next tick (~1s) and
        // exits on its own. Not joined here so shutdown stays prompt.
    }

    #[cfg(test)]
    pub(crate) fn backdate_idle_for_test(&self, by: Duration) {
        let mut state = match self.state.lock() {
            Ok(g) => g,
            Err(err) => err.into_inner(),
        };
        for slot in state.idle.iter_mut() {
            slot.last_used_at = slot
                .last_used_at
                .checked_sub(by)
                .unwrap_or(slot.last_used_at);
        }
    }

    fn build_worker(
        self: &Arc<Self>,
        key: WorkerKey,
        agent_id: &str,
        spawned: SpawnedChild,
    ) -> Box<WorkerInner> {
        let SpawnedChild {
            child,
            stdin,
            stdout,
            stderr,
        } = spawned;
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let demux: Arc<Mutex<Option<Sender<PoolLine>>>> = Arc::new(Mutex::new(None));
        let dead = Arc::new(AtomicBool::new(false));
        let stderr_buf = Arc::new(Mutex::new(Vec::<u8>::new()));

        let stdout_join = spawn_stdout_reader(stdout, Arc::clone(&demux), Arc::clone(&dead), id);
        let stderr_join = spawn_stderr_reader(stderr, Arc::clone(&stderr_buf), id);

        Box::new(WorkerInner {
            key,
            agent_affinity: agent_id.to_string(),
            child,
            stdin: Some(stdin),
            stderr: stderr_buf,
            demux,
            dead,
            last_used_at: Instant::now(),
            stdout_join: Some(stdout_join),
            stderr_join: Some(stderr_join),
        })
    }

    fn bind_receiver(self: &Arc<Self>, inner: Box<WorkerInner>) -> PoolWorker {
        let (tx, rx) = bounded(POOL_LINE_CAPACITY);
        if let Ok(mut guard) = inner.demux.lock() {
            *guard = Some(tx);
        }
        PoolWorker {
            inner: Some(inner),
            receiver: rx,
            pool: Arc::clone(self),
        }
    }

    fn return_idle(self: Arc<Self>, mut inner: Box<WorkerInner>) {
        if let Ok(mut guard) = inner.demux.lock() {
            *guard = None;
        }
        let alive = matches!(inner.child.try_wait(), Ok(None));
        let healthy = alive && !inner.dead.load(Ordering::Acquire);
        if !healthy || self.shutting_down.load(Ordering::Acquire) {
            self.destroy(inner);
            return;
        }
        inner.last_used_at = Instant::now();
        let mut state = match self.state.lock() {
            Ok(s) => s,
            Err(err) => err.into_inner(),
        };
        state.busy = state.busy.saturating_sub(1);
        state.idle.push_back(inner);
        self.available.notify_one();
    }

    // `Box<WorkerInner>` matches the slot type stored in the pool's idle
    // VecDeque, so callers can hand back boxes they popped without an
    // intermediate unbox step.
    #[allow(clippy::boxed_local)]
    fn destroy(self: Arc<Self>, inner: Box<WorkerInner>) {
        {
            let mut state = match self.state.lock() {
                Ok(s) => s,
                Err(err) => err.into_inner(),
            };
            state.busy = state.busy.saturating_sub(1);
        }
        (*inner).shutdown();
        self.available.notify_all();
    }

    pub fn gc_sweep(self: &Arc<Self>) {
        let now = Instant::now();
        let aged: Vec<Box<WorkerInner>> = {
            let mut state = match self.state.lock() {
                Ok(s) => s,
                Err(err) => err.into_inner(),
            };
            let mut keep = VecDeque::with_capacity(state.idle.len());
            let mut aged = Vec::new();
            for slot in state.idle.drain(..) {
                let elapsed = now.saturating_duration_since(slot.last_used_at);
                if elapsed >= POOL_IDLE_MAX || slot.dead.load(Ordering::Acquire) {
                    aged.push(slot);
                } else {
                    keep.push_back(slot);
                }
            }
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
        let drained: Vec<Box<WorkerInner>> = match self.state.lock() {
            Ok(mut s) => s.idle.drain(..).collect(),
            Err(err) => err.into_inner().idle.drain(..).collect(),
        };
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
    for (idx, slot) in idle.iter().enumerate() {
        if &slot.key != key {
            continue;
        }
        if slot.agent_affinity == agent_id {
            return Some(idx);
        }
        if compatible.is_none() {
            compatible = Some(idx);
        }
    }
    compatible
}

fn spawn_stdout_reader(
    stdout: ChildStdout,
    demux: Arc<Mutex<Option<Sender<PoolLine>>>>,
    dead: Arc<AtomicBool>,
    id: u64,
) -> JoinHandle<()> {
    thread::Builder::new()
        .name(format!("nit-claude-pool-stdout-{id}"))
        .spawn(move || {
            let mut reader = BufReader::new(stdout);
            let mut line = String::new();
            loop {
                line.clear();
                match reader.read_line(&mut line) {
                    Ok(0) => break,
                    Ok(_) => {
                        let pool_line = PoolLine {
                            raw: line.clone(),
                            captured_at: Instant::now(),
                        };
                        let sender = match demux.lock() {
                            Ok(guard) => guard.as_ref().cloned(),
                            Err(err) => err.into_inner().as_ref().cloned(),
                        };
                        if let Some(tx) = sender {
                            let _ = tx.try_send(pool_line);
                        }
                    }
                    Err(_) => break,
                }
            }
            dead.store(true, Ordering::Release);
        })
        .expect("spawn pool stdout reader")
}

fn spawn_stderr_reader(
    mut stderr: ChildStderr,
    buf: Arc<Mutex<Vec<u8>>>,
    id: u64,
) -> JoinHandle<()> {
    thread::Builder::new()
        .name(format!("nit-claude-pool-stderr-{id}"))
        .spawn(move || {
            let mut chunk = [0u8; 4096];
            loop {
                match stderr.read(&mut chunk) {
                    Ok(0) => break,
                    Ok(n) => {
                        let mut guard = match buf.lock() {
                            Ok(g) => g,
                            Err(err) => err.into_inner(),
                        };
                        guard.extend_from_slice(&chunk[..n]);
                        if guard.len() > POOL_STDERR_CAP_BYTES {
                            let drop_to = guard.len() - POOL_STDERR_CAP_BYTES * 3 / 4;
                            guard.drain(0..drop_to);
                        }
                    }
                    Err(_) => break,
                }
            }
        })
        .expect("spawn pool stderr reader")
}

fn spawn_gc_thread(pool: Arc<ClaudePool>) {
    let weak = Arc::downgrade(&pool);
    drop(pool);
    let _ = thread::Builder::new()
        .name("nit-claude-pool-gc".into())
        .spawn(move || {
            // Tick frequently so `shutdown()` is observed within ~1s even
            // though the full sweep cadence is `POOL_GC_INTERVAL`.
            let tick = Duration::from_secs(1);
            let mut accumulated = Duration::ZERO;
            loop {
                thread::sleep(tick);
                accumulated += tick;
                let Some(pool) = weak.upgrade() else { break };
                if pool.shutting_down.load(Ordering::Acquire) {
                    break;
                }
                if accumulated >= POOL_GC_INTERVAL {
                    pool.gc_sweep();
                    accumulated = Duration::ZERO;
                }
            }
        });
}

/// Build the stream-json envelope sent over a pooled worker's stdin for a
/// single turn. Mirrors the protocol the Claude CLI accepts when launched
/// with `--input-format stream-json`.
pub fn build_stream_json_envelope(prompt: &str) -> String {
    let payload = serde_json::json!({
        "type": "user",
        "message": {
            "role": "user",
            "content": prompt,
        }
    });
    let mut envelope = payload.to_string();
    envelope.push('\n');
    envelope
}

/// True when an env-var value parses as a positive opt-in (`1` / `true` /
/// `yes`, case-insensitive). Empty / unset → false.
pub fn env_flag_enabled(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "yes" | "on"
    )
}

/// Read `NIT_CLAUDE_POOL` from the environment.
pub fn pool_enabled_from_env() -> bool {
    std::env::var("NIT_CLAUDE_POOL")
        .ok()
        .map(|v| env_flag_enabled(&v))
        .unwrap_or(false)
}

/// Read `NIT_CLAUDE_POOL_SIZE` from the environment, falling back to
/// [`crate::swarm::limits::default_claude_pool_size`] when unset or
/// unparseable.
pub fn pool_size_from_env() -> usize {
    if let Ok(raw) = std::env::var("NIT_CLAUDE_POOL_SIZE") {
        if let Ok(n) = raw.trim().parse::<usize>() {
            if n > 0 {
                return n;
            }
        }
    }
    crate::swarm::default_claude_pool_size()
}

#[cfg(test)]
#[path = "tests/claude_pool.rs"]
mod tests;
