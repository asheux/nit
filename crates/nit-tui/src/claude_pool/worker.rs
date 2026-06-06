//! Worker slots for the warm `claude -p` pool: the per-slot child handle,
//! the checked-out `PoolWorker` facade, and the stdout/stderr reader
//! threads. The pool lifecycle and checkout logic live in `super::pool`.

use std::io::{BufRead, BufReader, Read, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStderr, ChildStdin, ChildStdout};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crossbeam_channel::{bounded, Receiver, Sender};

use super::pool::{lock_recover, ClaudePool};
use super::{POOL_LINE_CAPACITY, POOL_STDERR_CAP_BYTES};

// Late-bound channel into the active checkout: `None` between turns, so the
// stdout reader silently drops lines until a `PoolWorker` binds a receiver.
type DemuxSender = Arc<Mutex<Option<Sender<PoolLine>>>>;
type StderrTail = Arc<Mutex<Vec<u8>>>;

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

pub(crate) struct WorkerInner {
    pub(crate) key: WorkerKey,
    pub(crate) agent_affinity: String,
    pub(crate) child: Child,
    stdin: Option<ChildStdin>,
    stderr: StderrTail,
    demux: DemuxSender,
    pub(crate) dead: Arc<AtomicBool>,
    pub(crate) last_used_at: Instant,
    stdout_join: Option<JoinHandle<()>>,
    stderr_join: Option<JoinHandle<()>>,
}

impl WorkerInner {
    pub(crate) fn from_spawned(
        key: WorkerKey,
        agent_id: &str,
        spawned: SpawnedChild,
        id: u64,
    ) -> Box<WorkerInner> {
        let SpawnedChild {
            child,
            stdin,
            stdout,
            stderr,
        } = spawned;
        let demux: DemuxSender = Arc::new(Mutex::new(None));
        let dead = Arc::new(AtomicBool::new(false));
        let stderr_buf: StderrTail = Arc::new(Mutex::new(Vec::new()));

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

    pub(crate) fn detach_receiver(&self) {
        if let Ok(mut guard) = self.demux.lock() {
            *guard = None;
        }
    }

    pub(crate) fn shutdown(mut self) {
        // Drop the sender before killing the child so the stdout reader sees
        // `None` and stops forwarding into a receiver that is going away.
        self.detach_receiver();
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
    pub(crate) fn new(inner: Box<WorkerInner>, pool: Arc<ClaudePool>) -> PoolWorker {
        let (tx, rx) = bounded(POOL_LINE_CAPACITY);
        if let Ok(mut guard) = inner.demux.lock() {
            *guard = Some(tx);
        }
        PoolWorker {
            inner: Some(inner),
            receiver: rx,
            pool,
        }
    }

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

fn spawn_stdout_reader(
    stdout: ChildStdout,
    demux: DemuxSender,
    dead: Arc<AtomicBool>,
    id: u64,
) -> JoinHandle<()> {
    thread::Builder::new()
        .name(format!("nit-claude-pool-stdout-{id}"))
        .spawn(move || pump_stdout(stdout, demux, dead))
        .expect("spawn pool stdout reader")
}

fn spawn_stderr_reader(stderr: ChildStderr, buf: StderrTail, id: u64) -> JoinHandle<()> {
    thread::Builder::new()
        .name(format!("nit-claude-pool-stderr-{id}"))
        .spawn(move || pump_stderr(stderr, buf))
        .expect("spawn pool stderr reader")
}

fn pump_stdout(stdout: ChildStdout, demux: DemuxSender, dead: Arc<AtomicBool>) {
    let mut reader = BufReader::new(stdout);
    let mut line = String::new();
    loop {
        line.clear();
        let n = match reader.read_line(&mut line) {
            Ok(n) => n,
            Err(_) => break,
        };
        if n == 0 {
            break;
        }
        // Stamp capture time before taking the demux lock so a slow consumer
        // can't skew the observed read time of this line.
        let pool_line = PoolLine {
            raw: line.clone(),
            captured_at: Instant::now(),
        };
        let sender = lock_recover(&demux).as_ref().cloned();
        if let Some(tx) = sender {
            let _ = tx.try_send(pool_line);
        }
    }
    dead.store(true, Ordering::Release);
}

fn pump_stderr(mut stderr: ChildStderr, buf: StderrTail) {
    let mut chunk = [0u8; 4096];
    while let Ok(n) = stderr.read(&mut chunk) {
        if n == 0 {
            break;
        }
        let mut guard = lock_recover(&buf);
        guard.extend_from_slice(&chunk[..n]);
        if guard.len() > POOL_STDERR_CAP_BYTES {
            let drop_to = guard.len() - POOL_STDERR_CAP_BYTES * 3 / 4;
            guard.drain(0..drop_to);
        }
    }
}
