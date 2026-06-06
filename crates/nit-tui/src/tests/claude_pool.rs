use super::*;

use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

// `limits` is a private module under `swarm`; re-exported helpers we need
// are pulled in below through the parent module's `super::*` glob, but the
// `compute_*` helper is `pub(crate)` and only reachable via the full path.
use crate::swarm::limits::compute_default_claude_pool_size;

#[derive(Default)]
struct SpawnerStub {
    spawn_calls: AtomicUsize,
    fail_after: Option<usize>,
}

impl SpawnerStub {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            spawn_calls: AtomicUsize::new(0),
            fail_after: None,
        })
    }

    fn spawn_count(&self) -> usize {
        self.spawn_calls.load(Ordering::Acquire)
    }
}

#[derive(Clone)]
struct StubSpawner {
    inner: Arc<SpawnerStub>,
}

impl StubSpawner {
    fn new(inner: Arc<SpawnerStub>) -> Self {
        Self { inner }
    }
}

impl WorkerSpawn for StubSpawner {
    fn spawn(&self, _key: &WorkerKey, _agent_id: &str) -> std::io::Result<SpawnedChild> {
        let n = self.inner.spawn_calls.fetch_add(1, Ordering::AcqRel);
        if let Some(after) = self.inner.fail_after {
            if n >= after {
                return Err(std::io::Error::other("stub: spawn limit"));
            }
        }
        spawn_sleep_child()
    }
}

fn spawn_sleep_child() -> std::io::Result<SpawnedChild> {
    // `cat` is a long-running pipe-driven helper: it reads stdin until EOF
    // and echoes it back. That gives us a real `Child` with usable stdin /
    // stdout / stderr pipes so the pool's lifecycle paths exercise actual
    // process plumbing — without depending on the `claude` CLI being on
    // the host.
    let mut child = Command::new("cat")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let stdin = child
        .stdin
        .take()
        .ok_or_else(|| std::io::Error::other("test: missing stdin"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| std::io::Error::other("test: missing stdout"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| std::io::Error::other("test: missing stderr"))?;
    Ok(SpawnedChild {
        child,
        stdin,
        stdout,
        stderr,
    })
}

fn key(model: &str, cwd: &str, read_only: bool) -> WorkerKey {
    WorkerKey::new(model.to_string(), PathBuf::from(cwd), read_only)
}

fn build_pool(cap: usize) -> (Arc<ClaudePool>, StubSpawner) {
    let stub = StubSpawner::new(SpawnerStub::new());
    let spawner: Box<dyn WorkerSpawn> = Box::new(stub.clone());
    (ClaudePool::new(cap, spawner), stub)
}

#[test]
fn default_pool_size_clamps_across_ulimit_corners() {
    // macOS default ulimit -n 256 → effective swarm 56 → pool min(56/4, 8) = 8.
    assert_eq!(compute_default_claude_pool_size(56), 8);
    // Linux 1024-fd default → effective swarm saturates at 256 → pool 64 → ceiling 8.
    assert_eq!(compute_default_claude_pool_size(256), 8);
    // Tight ulimit -n 64 → effective swarm 8 → pool 8/4 = 2.
    assert_eq!(compute_default_claude_pool_size(8), 2);
    // Degenerate (effective ceiling 1) → floor of 2.
    assert_eq!(compute_default_claude_pool_size(1), 2);
    assert_eq!(compute_default_claude_pool_size(0), 2);
    // Effective ceiling far above pool ceiling → still clamped to 8.
    assert_eq!(compute_default_claude_pool_size(1024), 8);
}

#[test]
fn worker_key_equality_and_hash() {
    use std::collections::HashSet;
    let a = key("claude-opus-4-7", "/tmp/work", false);
    let b = key("claude-opus-4-7", "/tmp/work", false);
    let c = key("claude-opus-4-7", "/tmp/work", true);
    let d = key("claude-haiku-4-5", "/tmp/work", false);
    let e = key("claude-opus-4-7", "/tmp/other", false);

    assert_eq!(a, b);
    assert_ne!(a, c);
    assert_ne!(a, d);
    assert_ne!(a, e);

    let mut set = HashSet::new();
    set.insert(a.clone());
    set.insert(b);
    set.insert(c);
    set.insert(d);
    set.insert(e);
    assert_eq!(set.len(), 4);
}

#[test]
fn env_flag_enabled_matches_canonical_truthy_values() {
    assert!(env_flag_enabled("1"));
    assert!(env_flag_enabled("true"));
    assert!(env_flag_enabled("TRUE"));
    assert!(env_flag_enabled("Yes"));
    assert!(env_flag_enabled("on"));
    assert!(!env_flag_enabled("0"));
    assert!(!env_flag_enabled(""));
    assert!(!env_flag_enabled("false"));
    assert!(!env_flag_enabled("nope"));
}

#[test]
fn build_stream_json_envelope_emits_user_message() {
    let env = build_stream_json_envelope("hello pool");
    assert!(env.ends_with('\n'));
    let trimmed = env.trim_end();
    let value: serde_json::Value = serde_json::from_str(trimmed).unwrap();
    assert_eq!(value.get("type").and_then(|v| v.as_str()), Some("user"));
    let role = value
        .get("message")
        .and_then(|m| m.get("role"))
        .and_then(|v| v.as_str());
    assert_eq!(role, Some("user"));
    let content = value
        .get("message")
        .and_then(|m| m.get("content"))
        .and_then(|v| v.as_str());
    assert_eq!(content, Some("hello pool"));
}

#[test]
fn pool_size_clamped_to_at_least_one() {
    let (pool, _) = build_pool(0);
    assert_eq!(pool.capacity(), 1);
}

#[test]
fn checkout_spawns_fresh_worker_when_idle_empty() {
    let (pool, stub) = build_pool(4);
    let k = key("claude-opus-4-7", "/tmp/work", false);
    let worker = pool
        .checkout(&k, "claude-opus-4-7", Duration::from_millis(50))
        .expect("checkout succeeds");
    assert_eq!(stub.inner.spawn_count(), 1);
    assert_eq!(worker.key(), &k);
    assert_eq!(worker.agent_affinity(), "claude-opus-4-7");
    let (idle, busy) = pool.snapshot_sizes();
    assert_eq!(idle, 0);
    assert_eq!(busy, 1);
    worker.recycle(RecycleReason::Shutdown);
    let (idle, busy) = pool.snapshot_sizes();
    assert_eq!(idle, 0);
    assert_eq!(busy, 0);
}

#[test]
fn check_in_returns_slot_to_idle_for_reuse() {
    let (pool, stub) = build_pool(4);
    let k = key("claude-opus-4-7", "/tmp/work", false);
    let worker = pool
        .checkout(&k, "claude-opus-4-7#a", Duration::from_millis(50))
        .expect("checkout 1");
    worker.check_in();
    let (idle, busy) = pool.snapshot_sizes();
    assert_eq!(idle, 1);
    assert_eq!(busy, 0);
    let _worker2 = pool
        .checkout(&k, "claude-opus-4-7#a", Duration::from_millis(50))
        .expect("checkout 2");
    assert_eq!(
        stub.inner.spawn_count(),
        1,
        "reused slot must NOT trigger a fresh spawn"
    );
}

#[test]
fn affinity_preferred_over_compatible_match() {
    let (pool, _stub) = build_pool(4);
    let k = key("claude-opus-4-7", "/tmp/work", false);
    let w_a = pool
        .checkout(&k, "agent-A", Duration::from_millis(50))
        .expect("checkout A");
    let w_b = pool
        .checkout(&k, "agent-B", Duration::from_millis(50))
        .expect("checkout B");
    let pid_a = w_a.child_id();
    let pid_b = w_b.child_id();
    w_a.check_in();
    w_b.check_in();
    let next_a = pool
        .checkout(&k, "agent-A", Duration::from_millis(50))
        .expect("checkout next-A");
    assert_eq!(
        next_a.child_id(),
        pid_a,
        "agent-A's idle slot must be preferred over agent-B's compatible slot"
    );
    drop(next_a);
    let next_compat = pool
        .checkout(&k, "agent-C", Duration::from_millis(50))
        .expect("checkout C");
    assert_eq!(
        next_compat.child_id(),
        pid_b,
        "no affinity match → any compatible slot"
    );
}

#[test]
fn incompatible_key_does_not_reuse_slot() {
    let (pool, stub) = build_pool(4);
    let opus = key("claude-opus-4-7", "/tmp/work", false);
    let haiku = key("claude-haiku-4-5", "/tmp/work", false);
    let w = pool
        .checkout(&opus, "agent-A", Duration::from_millis(50))
        .expect("checkout opus");
    w.check_in();
    let _w2 = pool
        .checkout(&haiku, "agent-A", Duration::from_millis(50))
        .expect("checkout haiku");
    assert_eq!(
        stub.inner.spawn_count(),
        2,
        "different model_slug must trigger a fresh spawn"
    );
}

#[test]
fn checkout_times_out_when_pool_at_capacity() {
    let (pool, _stub) = build_pool(2);
    let k = key("claude-opus-4-7", "/tmp/work", false);
    let _w1 = pool
        .checkout(&k, "agent-A", Duration::from_millis(50))
        .expect("checkout 1");
    let _w2 = pool
        .checkout(&k, "agent-B", Duration::from_millis(50))
        .expect("checkout 2");
    let start = Instant::now();
    let none = pool.checkout(&k, "agent-C", Duration::from_millis(120));
    let elapsed = start.elapsed();
    assert!(none.is_none(), "checkout at capacity must return None");
    assert!(
        elapsed >= Duration::from_millis(100),
        "checkout must respect the timeout (elapsed={elapsed:?})"
    );
}

#[test]
fn recycle_does_not_return_to_idle() {
    let (pool, _stub) = build_pool(4);
    let k = key("claude-opus-4-7", "/tmp/work", false);
    let w = pool
        .checkout(&k, "agent-A", Duration::from_millis(50))
        .expect("checkout");
    w.recycle(RecycleReason::Cancel);
    let (idle, busy) = pool.snapshot_sizes();
    assert_eq!(idle, 0, "recycle must drop the slot, not idle it");
    assert_eq!(busy, 0);
}

#[test]
fn drop_without_explicit_handoff_recycles_slot() {
    let (pool, _stub) = build_pool(4);
    let k = key("claude-opus-4-7", "/tmp/work", false);
    {
        let _w = pool
            .checkout(&k, "agent-A", Duration::from_millis(50))
            .expect("checkout");
    }
    let (idle, busy) = pool.snapshot_sizes();
    assert_eq!(idle, 0, "Drop without check_in must NOT return-to-pool");
    assert_eq!(busy, 0);
}

#[test]
fn spawn_failure_releases_busy_count() {
    let stub_inner = Arc::new(SpawnerStub {
        spawn_calls: AtomicUsize::new(0),
        fail_after: Some(0),
    });
    let stub = StubSpawner::new(stub_inner);
    let pool: Arc<ClaudePool> = ClaudePool::new(2, Box::new(stub.clone()));
    let k = key("claude-opus-4-7", "/tmp/work", false);
    let none = pool.checkout(&k, "agent-A", Duration::from_millis(50));
    assert!(none.is_none());
    let (idle, busy) = pool.snapshot_sizes();
    assert_eq!(
        busy, 0,
        "spawn failure must refund the busy slot, not leave it parked"
    );
    assert_eq!(idle, 0);
}

#[test]
fn gc_sweep_recycles_aged_idle_slots() {
    let (pool, _stub) = build_pool(4);
    let k = key("claude-opus-4-7", "/tmp/work", false);
    let w = pool
        .checkout(&k, "agent-A", Duration::from_millis(50))
        .expect("checkout");
    w.check_in();
    let (idle, _) = pool.snapshot_sizes();
    assert_eq!(idle, 1);
    // Step the clock forward past POOL_IDLE_MAX instead of backdating the
    // slot — forward-stepping always succeeds, whereas `checked_sub` on
    // `Instant` can underflow on platforms whose monotonic clock anchors at
    // boot and the test process is younger than POOL_IDLE_MAX (Windows CI).
    let future = std::time::Instant::now() + POOL_IDLE_MAX + Duration::from_secs(1);
    pool.gc_sweep_at(future);
    let (idle, busy) = pool.snapshot_sizes();
    assert_eq!(idle, 0, "GC must drop slots older than POOL_IDLE_MAX");
    assert_eq!(busy, 0);
}

#[test]
fn pool_shutdown_closes_idle_slots_and_blocks_new_checkout() {
    let (pool, _stub) = build_pool(4);
    let k = key("claude-opus-4-7", "/tmp/work", false);
    let w = pool
        .checkout(&k, "agent-A", Duration::from_millis(50))
        .expect("checkout");
    w.check_in();
    let (idle_pre, _) = pool.snapshot_sizes();
    assert_eq!(idle_pre, 1);

    pool.shutdown();

    let (idle, busy) = pool.snapshot_sizes();
    assert_eq!(idle, 0);
    assert_eq!(busy, 0);
    let none = pool.checkout(&k, "agent-A", Duration::from_millis(50));
    assert!(none.is_none(), "shutdown pool must refuse new checkouts");
}

#[test]
fn fd_budget_regression_under_simulated_tight_ulimit() {
    // At an effective swarm ceiling of 8 (the macOS-style tight-ulimit
    // case), default_claude_pool_size returns the floor of 2.  Verify the
    // pool itself respects that cap so 4 simulated agents × 8 turns can't
    // explode the FD count beyond `2 * FDS_PER_AGENT` permanently parked.
    let cap = compute_default_claude_pool_size(8);
    assert_eq!(cap, 2);
    let (pool, _stub) = build_pool(cap);
    let k = key("claude-opus-4-7", "/tmp/work", false);

    let w1 = pool
        .checkout(&k, "agent-A", Duration::from_millis(50))
        .expect("checkout 1");
    let w2 = pool
        .checkout(&k, "agent-B", Duration::from_millis(50))
        .expect("checkout 2");
    let denied = pool.checkout(&k, "agent-C", Duration::from_millis(80));
    assert!(
        denied.is_none(),
        "with cap=2 the third concurrent checkout must time out"
    );
    drop(w1);
    drop(w2);
    let later = pool
        .checkout(&k, "agent-D", Duration::from_millis(80))
        .expect("post-drain checkout");
    drop(later);
    let (idle, busy) = pool.snapshot_sizes();
    assert!(busy <= cap);
    assert!(idle + busy <= cap);
}

#[test]
fn pool_enabled_from_env_round_trip() {
    use std::sync::Mutex;
    static LOCK: Mutex<()> = Mutex::new(());
    let _guard = LOCK.lock().unwrap();
    let prior = std::env::var("NIT_CLAUDE_POOL").ok();
    std::env::set_var("NIT_CLAUDE_POOL", "1");
    assert!(pool_enabled_from_env());
    std::env::set_var("NIT_CLAUDE_POOL", "true");
    assert!(pool_enabled_from_env());
    std::env::set_var("NIT_CLAUDE_POOL", "0");
    assert!(!pool_enabled_from_env());
    std::env::remove_var("NIT_CLAUDE_POOL");
    assert!(!pool_enabled_from_env());
    match prior {
        Some(v) => std::env::set_var("NIT_CLAUDE_POOL", v),
        None => std::env::remove_var("NIT_CLAUDE_POOL"),
    }
}
