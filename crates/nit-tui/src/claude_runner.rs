use std::collections::VecDeque;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use nit_core::{AgentBusEvent, AgentTokenCount, McpConnectionState, McpStatus};

use crate::claude_pool::{
    build_stream_json_envelope, pool_enabled_from_env, pool_size_from_env, ClaudePool, PoolWorker,
    RecycleReason, SpawnedChild, WorkerKey, WorkerSpawn,
};
use crate::swarm::is_provider_quota_exhausted_in_result;

// Worst-case time a pool checkout is allowed to wait before falling back
// to a cold-spawn. The pool is a perf optimisation; if no worker is free in
// 250ms it's cheaper to spawn a fresh process than block the dispatch loop.
const POOL_CHECKOUT_TIMEOUT: Duration = Duration::from_millis(250);

#[derive(Clone, Debug)]
pub struct ClaudeRunnerConfig {
    pub max_parallel_turns: usize,
    /// Claude CLI `--permission-mode` value (e.g. `"dangerously-skip-permissions"` for headless).
    pub permission_mode: Option<String>,
}

impl Default for ClaudeRunnerConfig {
    fn default() -> Self {
        Self {
            max_parallel_turns: usize::MAX,
            permission_mode: None,
        }
    }
}

#[derive(Clone, Debug)]
pub enum ClaudeCommand {
    RunTurn {
        model: String,
        cwd: PathBuf,
        mission_id: Option<String>,
        /// Resume an existing Claude session id when present.
        resume_session_id: Option<String>,
        /// When false, run the turn with `--no-session-persistence`.
        /// When true, persist the session so future turns can resume it.
        persist_session: bool,
        effort: Option<String>,
        prompt: String,
        /// Restrict the turn to read-only tools (no Write/Edit/Bash). Used for
        /// shadow advisory agents that must not modify the workspace.
        read_only: bool,
        /// Override for `--max-turns`. When `None` the runner uses
        /// `DEFAULT_MAX_TURNS`. Integrator tasks run real verify loops
        /// (clippy → test → fmt → fix → re-check) and routinely need more
        /// than the default budget.
        max_turns: Option<u32>,
    },
    Shutdown,
    /// Cancel any in-flight turn for `agent_id` (kills the subprocess via
    /// the per-turn `cancel` AtomicBool) and drop matching queued turns.
    /// Idempotent — no-op if the agent has no in-flight or queued work.
    CancelTurn {
        agent_id: String,
    },
    /// Cancel every in-flight turn and drop the entire queue.
    CancelAll,
}

pub const DEFAULT_MAX_TURNS: u32 = 50;
/// Budget for swarm support roles (proposer, judge, review, test, research).
/// These roles are read-only but perform deep recon — reading many files,
/// greping for symbols, collecting evidence — before producing a substantial
/// proposal/critique. The plain-chat default of 50 gets exhausted quickly on
/// non-trivial scopes (e.g. proposing a refactor plan for a 13k-line module
/// routinely needs 100+ read/grep turns).
pub const SWARM_SUPPORT_MAX_TURNS: u32 = 200;
/// Budget for the integrator — the single writer. Runs verify loops
/// (clippy → test → fmt → fix → re-check) on top of the actual edits, so the
/// envelope has to be large enough for the whole write/verify cycle.
pub const INTEGRATOR_MAX_TURNS: u32 = 500;

pub struct ClaudeRunner {
    cmd_tx: Sender<ClaudeCommand>,
    pub events: Receiver<AgentBusEvent>,
    shutdown: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl ClaudeRunner {
    #[cfg(test)]
    pub(crate) fn test_channel() -> (Self, Receiver<ClaudeCommand>) {
        let (cmd_tx, commands) = mpsc::channel();
        let (_, events) = mpsc::channel();
        (
            Self {
                cmd_tx,
                events,
                shutdown: Arc::new(AtomicBool::new(false)),
                handle: None,
            },
            commands,
        )
    }

    pub fn spawn(config: ClaudeRunnerConfig) -> Self {
        let pool = if pool_enabled_from_env() {
            let cap = pool_size_from_env();
            let spawner: Box<dyn WorkerSpawn> = Box::new(ClaudeCliSpawner::new(config.clone()));
            Some(ClaudePool::new(cap, spawner))
        } else {
            None
        };
        Self::spawn_with_pool(config, pool)
    }

    pub(crate) fn spawn_with_pool(
        config: ClaudeRunnerConfig,
        pool: Option<Arc<ClaudePool>>,
    ) -> Self {
        let (cmd_tx, cmd_rx) = mpsc::channel();
        let (event_tx, event_rx) = mpsc::channel();
        let shutdown = Arc::new(AtomicBool::new(false));
        let shutdown_worker = Arc::clone(&shutdown);
        let handle = thread::Builder::new()
            .name("nit-claude".into())
            .spawn(move || runner_loop(config, shutdown_worker, cmd_rx, event_tx, pool))
            .expect("spawn claude runner");
        Self {
            cmd_tx,
            events: event_rx,
            shutdown,
            handle: Some(handle),
        }
    }

    // Returns `false` when the runner channel is disconnected (shut down or crashed).
    pub fn send(&self, command: ClaudeCommand) -> bool {
        self.cmd_tx.send(command).is_ok()
    }

    pub fn shutdown(&mut self) {
        self.shutdown.store(true, Ordering::Relaxed);
        let _ = self.cmd_tx.send(ClaudeCommand::Shutdown);
        let Some(handle) = self.handle.take() else {
            return;
        };

        let (done_tx, done_rx) = mpsc::channel();
        let _ = thread::Builder::new()
            .name("nit-claude-join".into())
            .spawn(move || {
                let _ = handle.join();
                let _ = done_tx.send(());
            });
        let _ = done_rx.recv_timeout(Duration::from_millis(400));
    }
}

impl Drop for ClaudeRunner {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn cancel_queued_turns(
    queue: &mut VecDeque<ClaudeCommand>,
    event_tx: &Sender<AgentBusEvent>,
    agent: Option<&str>,
) {
    queue.retain(|command| {
        let ClaudeCommand::RunTurn {
            model, mission_id, ..
        } = command
        else {
            return true;
        };
        if agent.is_some_and(|id| id != model) {
            return true;
        }
        let _ = event_tx.send(AgentBusEvent::TurnFailed {
            agent_id: model.clone(),
            mission_id: mission_id.clone(),
            thread_id: None,
            token_count: None,
            message: nit_core::OPERATOR_CANCEL_TURN_MESSAGE.into(),
        });
        false
    });
}

fn runner_loop(
    config: ClaudeRunnerConfig,
    _shutdown: Arc<AtomicBool>,
    cmd_rx: Receiver<ClaudeCommand>,
    event_tx: Sender<AgentBusEvent>,
    pool: Option<Arc<ClaudePool>>,
) {
    let mut seq = 0u64;
    let mut queue: VecDeque<ClaudeCommand> = VecDeque::new();
    let mut active: Vec<ActiveTurn> = Vec::new();
    let mut shutting_down = false;
    let mut shutdown_deadline: Option<Instant> = None;
    let max_parallel = config.max_parallel_turns.max(1);

    loop {
        let cmd = if active.is_empty() && queue.is_empty() && !shutting_down {
            cmd_rx.recv().ok()
        } else {
            match cmd_rx.recv_timeout(Duration::from_millis(50)) {
                Ok(cmd) => Some(cmd),
                Err(mpsc::RecvTimeoutError::Timeout) => None,
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    shutting_down = true;
                    shutdown_deadline = Some(Instant::now() + Duration::from_millis(400));
                    None
                }
            }
        };

        if let Some(cmd) = cmd {
            match cmd {
                ClaudeCommand::RunTurn { .. } if !shutting_down => queue.push_back(cmd),
                ClaudeCommand::RunTurn { .. } => {}
                ClaudeCommand::Shutdown => {
                    shutting_down = true;
                    shutdown_deadline = Some(Instant::now() + Duration::from_millis(400));
                    queue.clear();
                    for turn in active.iter() {
                        turn.cancel.store(true, Ordering::Relaxed);
                    }
                }
                ClaudeCommand::CancelTurn { agent_id } => {
                    // Kill in-flight turn for this agent; worker checks
                    // `cancel` between try_wait polls and child.kill()s
                    // within ~50ms.
                    for turn in active.iter() {
                        if turn.agent_id == agent_id {
                            turn.cancel.store(true, Ordering::Relaxed);
                        }
                    }
                    cancel_queued_turns(&mut queue, &event_tx, Some(&agent_id));
                }
                ClaudeCommand::CancelAll => {
                    cancel_queued_turns(&mut queue, &event_tx, None);
                    for turn in active.iter() {
                        turn.cancel.store(true, Ordering::Relaxed);
                    }
                }
            }
        }

        let mut idx = 0usize;
        while idx < active.len() {
            let done = match active[idx].done_rx.try_recv() {
                Ok(()) | Err(mpsc::TryRecvError::Disconnected) => true,
                Err(mpsc::TryRecvError::Empty) => false,
            };
            if done {
                let turn = active.remove(idx);
                let _ = turn.handle.join();
            } else {
                idx += 1;
            }
        }

        if !shutting_down {
            while active.len() < max_parallel {
                let idx = queue.iter().position(|cmd| match cmd {
                    ClaudeCommand::RunTurn { model, .. } => !active
                        .iter()
                        .any(|turn| turn.agent_id.as_str() == model.as_str()),
                    _ => false,
                });
                let Some(idx) = idx else {
                    break;
                };
                let Some(cmd) = queue.remove(idx) else {
                    break;
                };
                let ClaudeCommand::RunTurn {
                    model,
                    cwd,
                    mission_id,
                    resume_session_id,
                    persist_session,
                    effort,
                    prompt,
                    read_only,
                    max_turns,
                } = cmd
                else {
                    continue;
                };
                let _ = event_tx.send(AgentBusEvent::McpStatus {
                    status: McpStatus {
                        state: McpConnectionState::Connecting,
                        endpoint: claude_endpoint_label(&model, resume_session_id.as_deref()),
                        latency_ms: None,
                        last_error: None,
                    },
                });
                let _ = event_tx.send(AgentBusEvent::TurnStarted {
                    agent_id: model.clone(),
                    mission_id: mission_id.clone(),
                    resume_thread_id: resume_session_id.clone(),
                });
                crate::swarm::report_provider_followup_input(
                    &event_tx,
                    &model,
                    mission_id.as_deref(),
                    &prompt,
                );
                seq = seq.wrapping_add(1);
                active.push(spawn_turn_worker(
                    &event_tx,
                    seq,
                    model,
                    cwd,
                    mission_id,
                    resume_session_id,
                    persist_session,
                    effort,
                    prompt,
                    read_only,
                    max_turns,
                    config.clone(),
                    pool.clone(),
                ));
            }
        }

        if shutting_down {
            for turn in active.iter() {
                turn.cancel.store(true, Ordering::Relaxed);
            }
            if active.is_empty() {
                break;
            }
            if let Some(deadline) = shutdown_deadline {
                if Instant::now() >= deadline {
                    break;
                }
            }
        }
    }

    if let Some(pool) = pool {
        pool.shutdown();
    }
}

struct ActiveTurn {
    agent_id: String,
    cancel: Arc<AtomicBool>,
    done_rx: Receiver<()>,
    handle: JoinHandle<()>,
}

/// See `codex_runner::STDOUT_TAIL_CAP_BYTES` — same rationale. 100 MB tail
/// window, drop from front at newline boundary on overflow.
const STDOUT_TAIL_CAP_BYTES: usize = 100 * 1024 * 1024;
const JSON_ERRORS_CAP: usize = 256;

fn append_stdout_line_capped(buf: &mut Vec<u8>, line: &[u8]) {
    buf.extend_from_slice(line);
    if buf.len() <= STDOUT_TAIL_CAP_BYTES {
        return;
    }
    let want_drop = buf.len() - STDOUT_TAIL_CAP_BYTES * 3 / 4;
    let drop_to = buf[want_drop..]
        .iter()
        .position(|&b| b == b'\n')
        .map(|i| want_drop + i + 1)
        .unwrap_or(buf.len());
    buf.drain(0..drop_to);
}

fn push_json_error_capped(errors: &mut Vec<String>, msg: String) {
    if errors.len() >= JSON_ERRORS_CAP {
        errors.drain(0..JSON_ERRORS_CAP / 2);
    }
    errors.push(msg);
}

/// Decide whether a `tool_use` block represents productive write-like work
/// for the idle-reaper writer-exemption. Recognises the four built-in
/// editors, write-like Bash commands (redirection, mv/cp/tee, common
/// build/test runners), and `mcp__<server>__<verb>` tools whose verb
/// signals creation/update/upload/edit. Conservative on Bash — false
/// positives keep an unproductive turn alive past the timeout, but
/// false negatives kill genuinely productive work mid-run.
fn tool_invokes_writes(name: Option<&str>, input: Option<&serde_json::Value>) -> bool {
    let Some(name) = name else { return false };
    if matches!(name, "Write" | "Edit" | "MultiEdit" | "NotebookEdit") {
        return true;
    }
    if name == "Bash" {
        let cmd = input
            .and_then(|v| v.get("command"))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        return bash_command_writes(cmd);
    }
    if let Some(rest) = name.strip_prefix("mcp__") {
        if let Some((_, action)) = rest.rsplit_once("__") {
            let head = action.split('_').next().unwrap_or(action);
            return matches!(
                head,
                "create" | "update" | "write" | "upload" | "edit" | "put"
            );
        }
    }
    false
}

fn bash_command_writes(cmd: &str) -> bool {
    if cmd.contains('>') {
        return true;
    }
    cmd.split([';', '|', '&']).any(|segment| {
        let mut tokens = segment.split_whitespace();
        match tokens.next() {
            Some("tee" | "mv" | "cp" | "rm" | "mkdir" | "touch") => true,
            Some("cargo") => matches!(
                tokens.next(),
                Some("build" | "test" | "fix" | "fmt" | "clippy" | "check")
            ),
            _ => false,
        }
    })
}

#[allow(clippy::too_many_arguments)]
fn spawn_turn_worker(
    event_tx: &Sender<AgentBusEvent>,
    seq: u64,
    model: String,
    cwd: PathBuf,
    mission_id: Option<String>,
    resume_session_id: Option<String>,
    persist_session: bool,
    effort: Option<String>,
    prompt: String,
    read_only: bool,
    max_turns: Option<u32>,
    config: ClaudeRunnerConfig,
    pool: Option<Arc<ClaudePool>>,
) -> ActiveTurn {
    let agent_id = model.clone();
    let cancel = Arc::new(AtomicBool::new(false));
    let (done_tx, done_rx) = mpsc::channel();
    let event_tx = event_tx.clone();
    let cancel_worker = Arc::clone(&cancel);
    let handle = thread::Builder::new()
        .name(format!("nit-claude-turn-{seq}"))
        .spawn(move || {
            dispatch_turn(
                &event_tx,
                seq,
                model,
                cwd,
                mission_id,
                resume_session_id,
                persist_session,
                effort,
                prompt,
                read_only,
                max_turns,
                config,
                pool,
                cancel_worker,
            );
            let _ = done_tx.send(());
        })
        .expect("spawn claude turn worker");
    ActiveTurn {
        agent_id,
        cancel,
        done_rx,
        handle,
    }
}

#[allow(clippy::too_many_arguments)]
fn dispatch_turn(
    event_tx: &Sender<AgentBusEvent>,
    seq: u64,
    model: String,
    cwd: PathBuf,
    mission_id: Option<String>,
    resume_session_id: Option<String>,
    persist_session: bool,
    effort: Option<String>,
    prompt: String,
    read_only: bool,
    max_turns: Option<u32>,
    config: ClaudeRunnerConfig,
    pool: Option<Arc<ClaudePool>>,
    cancel: Arc<AtomicBool>,
) {
    if let Some(pool) = pool.as_ref() {
        if turn_supports_pool(&resume_session_id, persist_session, &effort, max_turns) {
            let key = WorkerKey::new(
                claude_model_slug_for_agent_id(&model).to_string(),
                cwd.clone(),
                read_only,
            );
            if let Some(worker) = pool.checkout(&key, model.as_str(), POOL_CHECKOUT_TIMEOUT) {
                run_turn_pooled(
                    event_tx,
                    worker,
                    seq,
                    model,
                    cwd,
                    mission_id,
                    resume_session_id,
                    prompt,
                    cancel,
                );
                return;
            }
        }
    }
    run_turn(
        event_tx,
        seq,
        model,
        cwd,
        mission_id,
        resume_session_id,
        persist_session,
        effort,
        prompt,
        read_only,
        max_turns,
        config,
        cancel,
    );
}

fn turn_supports_pool(
    resume_session_id: &Option<String>,
    persist_session: bool,
    effort: &Option<String>,
    max_turns: Option<u32>,
) -> bool {
    resume_session_id.is_none() && persist_session && effort.is_none() && max_turns.is_none()
}

#[allow(clippy::too_many_arguments)]
fn run_turn(
    event_tx: &Sender<AgentBusEvent>,
    seq: u64,
    model: String,
    cwd: PathBuf,
    mission_id: Option<String>,
    resume_session_id: Option<String>,
    persist_session: bool,
    effort: Option<String>,
    prompt: String,
    read_only: bool,
    max_turns: Option<u32>,
    config: ClaudeRunnerConfig,
    cancel: Arc<AtomicBool>,
) {
    let cmd = prepare_claude_command(
        model.as_str(),
        cwd.as_path(),
        persist_session,
        effort.as_deref(),
        resume_session_id.as_deref(),
        read_only,
        max_turns,
        &config,
    );
    run_cold_process(
        cmd,
        event_tx,
        seq,
        model,
        cwd,
        mission_id,
        resume_session_id,
        prompt,
        cancel,
    );
}

#[allow(clippy::too_many_arguments)]
fn run_cold_process(
    mut cmd: Command,
    event_tx: &Sender<AgentBusEvent>,
    seq: u64,
    model: String,
    cwd: PathBuf,
    mission_id: Option<String>,
    resume_session_id: Option<String>,
    prompt: String,
    cancel: Arc<AtomicBool>,
) {
    let started_at = Instant::now();
    // Independent file positions make input EOF a property of this process's
    // current output, even when a descendant inherited its standard streams.
    let spawned = (|| -> std::io::Result<_> {
        let (stdout, out_writer) = crate::runner_output::private_output_stream()?;
        let (stderr, err_writer) = crate::runner_output::private_output_stream()?;
        let child = cmd
            .stdin(Stdio::piped())
            .stdout(out_writer)
            .stderr(err_writer)
            .spawn()?;
        Ok((child, stdout, stderr))
    })();
    let (mut child, mut output, stderr_file) = match spawned {
        Ok(pair) => pair,
        Err(err) => {
            let _ = event_tx.send(AgentBusEvent::McpStatus {
                status: McpStatus {
                    state: McpConnectionState::Error,
                    endpoint: claude_endpoint_label(&model, resume_session_id.as_deref()),
                    latency_ms: None,
                    last_error: Some(format!("Failed to spawn claude: {err}")),
                },
            });
            let _ = event_tx.send(AgentBusEvent::TurnFailed {
                agent_id: model,
                mission_id,
                thread_id: resume_session_id.clone(),
                token_count: None,
                message: format!("Failed to spawn claude: {err}"),
            });
            return;
        }
    };

    // Mark as connected immediately so the vitals system doesn't flag CRIT
    // while the turn is in progress.
    let _ = event_tx.send(AgentBusEvent::McpStatus {
        status: McpStatus {
            state: McpConnectionState::Connected,
            endpoint: claude_endpoint_label(&model, resume_session_id.as_deref()),
            latency_ms: None,
            last_error: None,
        },
    });

    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(prompt.as_bytes());
        let _ = stdin.write_all(b"\n");
    }

    let mut stream = TurnStream::new();
    let mut pending = Vec::new();
    let mut buf = [0u8; 32 * 1024];
    let mut position = 0u64;
    let mut last_stdout_at = Instant::now();
    let mut last_heartbeat_at = Instant::now();
    let idle_timeout = claude_turn_idle_timeout();
    let mut kill_reason = None;
    let mut exited = None;
    let status = loop {
        if cancel.load(Ordering::Acquire) {
            kill_reason = Some(TurnKillReason::OperatorCancel);
        } else if stream.result_seen {
            kill_reason = Some(TurnKillReason::ResultSeen);
        } else if !stream.turn_did_writes
            && idle_timeout.is_some_and(|t| last_stdout_at.elapsed() >= t)
        {
            kill_reason = Some(TurnKillReason::IdleTimeout);
        }

        if exited.is_none() {
            if kill_reason.is_some() {
                let _ = child.kill();
            }
            let status = if kill_reason.is_some() {
                child.wait().map(Some)
            } else {
                child.try_wait()
            };
            match status {
                Ok(Some(status)) => {
                    // Ignore further writes from descendants of a finished CLI.
                    exited = Some((status, output.metadata().map_or(position, |m| m.len())));
                }
                Ok(None) => {}
                Err(err) => {
                    let _ = event_tx.send(AgentBusEvent::TurnFailed {
                        agent_id: model,
                        mission_id,
                        thread_id: resume_session_id,
                        token_count: None,
                        message: format!("Claude wait failed: {err}"),
                    });
                    return;
                }
            }
        }
        if let Some((status, end)) = exited {
            if kill_reason.is_some() || position >= end {
                if !pending.is_empty() && kill_reason.is_none() {
                    stream.observe(
                        &String::from_utf8_lossy(&pending),
                        event_tx,
                        &model,
                        &mission_id,
                        &cwd,
                    );
                }
                break status;
            }
        }
        if last_heartbeat_at.elapsed() >= Duration::from_secs(2) {
            let _ = event_tx.send(AgentBusEvent::TurnHeartbeat {
                agent_id: model.clone(),
                mission_id: mission_id.clone(),
            });
            last_heartbeat_at = Instant::now();
        }
        let limit = exited.map_or(buf.len(), |(_, end)| {
            (end - position).min(buf.len() as u64) as usize
        });
        let n = match output.read(&mut buf[..limit]) {
            Ok(n) => n,
            Err(err) if err.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(err) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = event_tx.send(AgentBusEvent::TurnFailed {
                    agent_id: model,
                    mission_id,
                    thread_id: resume_session_id,
                    token_count: None,
                    message: format!("Claude output read failed: {err}"),
                });
                return;
            }
        };
        if n == 0 {
            thread::sleep(Duration::from_millis(15));
            continue;
        }
        last_stdout_at = Instant::now();
        position += n as u64;
        for part in buf[..n].split_inclusive(|b| *b == b'\n') {
            pending.extend_from_slice(part);
            if part.ends_with(b"\n") {
                stream.observe(
                    &String::from_utf8_lossy(&pending),
                    event_tx,
                    &model,
                    &mission_id,
                    &cwd,
                );
                pending.clear();
                if stream.result_seen {
                    break;
                }
            }
        }
    };
    let stdout = stream.stdout_buf;
    let json_errors = stream.json_errors;
    let mut stderr = Vec::new();
    let len = stderr_file.metadata().map_or(0, |meta| meta.len());
    let _ = stderr_file
        .take(len.min(1024 * 1024))
        .read_to_end(&mut stderr);

    if kill_reason == Some(TurnKillReason::OperatorCancel) {
        // Emit a TurnFailed so AppState releases this active turn. The
        // bus handler detects the OPERATOR_CANCEL_TURN_MESSAGE sentinel
        // and routes this down a "soft" path (Idle status, Info diag)
        // instead of the Error path used by genuine subprocess failures.
        let _ = event_tx.send(AgentBusEvent::TurnFailed {
            agent_id: model,
            mission_id,
            thread_id: resume_session_id.clone(),
            token_count: None,
            message: nit_core::OPERATOR_CANCEL_TURN_MESSAGE.into(),
        });
        return;
    }

    // Diagnostic for runner-driven kills so the operator can see why a
    // turn ended early in the agent ops log.
    if let Some(reason) = kill_reason {
        let elapsed_secs = started_at.elapsed().as_secs();
        let msg = match reason {
            TurnKillReason::ResultSeen => format!(
                "Claude turn closed after `result` event (Claude CLI was still alive after {elapsed_secs}s; killed to release the swarm)"
            ),
            TurnKillReason::IdleTimeout => format!(
                "Claude turn killed by idle timeout after {}s of stream silence; attempting to recover the final message from buffered stream-json",
                idle_timeout.map(|d| d.as_secs()).unwrap_or_default(),
            ),
            TurnKillReason::OperatorCancel => unreachable!(),
        };
        let _ = event_tx.send(AgentBusEvent::TurnLog {
            agent_id: model.clone(),
            message: msg,
        });
    }

    let stderr_text = String::from_utf8_lossy(&stderr);
    for line in stderr_text.lines() {
        let line = line.trim();
        if !line.is_empty() {
            let _ = event_tx.send(AgentBusEvent::TurnLog {
                agent_id: model.clone(),
                message: line.to_string(),
            });
        }
    }

    if report_terminal_error(
        &stdout,
        event_tx,
        &model,
        &mission_id,
        resume_session_id.as_deref(),
        started_at,
    ) {
        return;
    }

    // Skip the subprocess-error path when WE killed the child — the
    // non-zero status is from our `child.kill()`, not from Claude
    // genuinely crashing.  The buffered stream-json still holds the
    // model's final assistant message and can be extracted below.
    if !stream.result_seen && kill_reason.is_none() && !status.success() {
        // Persist raw stdout + stderr to a debug log so the failure can be
        // inspected even when the subprocess dies without emitting any JSON
        // error events or stderr text (e.g. fast-exit rate limit / resource
        // failures). Include the log path in the TurnFailed message.
        let log_path = std::env::temp_dir().join(format!(
            "nit-claude-crash-{}-{seq}.log",
            mission_id.as_deref().unwrap_or("no-mission")
        ));
        let log_written = {
            let header = format!(
                "exit: {status}\nagent: {model}\nmission: {}\nelapsed_ms: {}\n--- stdout ---\n",
                mission_id.as_deref().unwrap_or("(none)"),
                started_at.elapsed().as_millis(),
            );
            let mut body = header.into_bytes();
            body.extend_from_slice(&stdout);
            body.extend_from_slice(b"\n--- stderr ---\n");
            body.extend_from_slice(&stderr);
            std::fs::write(&log_path, &body).is_ok()
        };
        let log_suffix = if log_written {
            format!(" (log: {})", log_path.display())
        } else {
            String::new()
        };
        let base = if !json_errors.is_empty() {
            json_errors.join(" | ")
        } else if !stderr_text.trim().is_empty() {
            stderr_text.trim().to_string()
        } else {
            format!("Claude exited with {status}")
        };
        let message = format!("{base}{log_suffix}");
        let latency_ms = started_at.elapsed().as_millis().min(u64::MAX as u128) as u64;
        let _ = event_tx.send(AgentBusEvent::McpStatus {
            status: McpStatus {
                state: McpConnectionState::Error,
                endpoint: claude_endpoint_label(&model, resume_session_id.as_deref()),
                latency_ms: Some(latency_ms),
                last_error: Some(message.clone()),
            },
        });
        let session_id = extract_session_id_from_jsonl(&stdout);
        let token_count = extract_token_count_from_jsonl(&stdout);
        let _ = event_tx.send(AgentBusEvent::TurnFailed {
            agent_id: model,
            mission_id,
            thread_id: session_id,
            token_count,
            message,
        });
        return;
    }

    let message = extract_result_text_from_jsonl(&stdout).unwrap_or_default();

    if message.is_empty() {
        let session_id = extract_session_id_from_jsonl(&stdout);
        let token_count = extract_token_count_from_jsonl(&stdout);
        let failure_message = match kill_reason {
            Some(TurnKillReason::IdleTimeout) => format!(
                "Claude turn killed by idle timeout after {}s of stream silence; no extractable result.",
                idle_timeout.map(|d| d.as_secs()).unwrap_or_default(),
            ),
            _ => "Claude finished but produced an empty last message.".into(),
        };
        let _ = event_tx.send(AgentBusEvent::TurnFailed {
            agent_id: model,
            mission_id,
            thread_id: session_id,
            token_count,
            message: failure_message,
        });
        return;
    }

    let session_id = extract_session_id_from_jsonl(&stdout);
    let token_count = extract_token_count_from_jsonl(&stdout);
    let latency_ms = started_at.elapsed().as_millis().min(u64::MAX as u128) as u64;
    let endpoint = claude_endpoint_label(&model, resume_session_id.as_deref());

    // Claude CLI exits 0 even when the account hit its quota — the limit
    // banner ("You've hit your limit · resets ...") gets glued onto the
    // result text instead of surfacing as a subprocess error. Treating that
    // as a successful turn poisoned the swarm: it triggered genome retries
    // against a dead quota, and silently completed `Synthesizing` with the
    // banner as the report. Detect the banner and route to TurnFailed so
    // the existing rate-limit-aware handlers fire.
    if is_provider_quota_exhausted_in_result(&message) {
        let _ = event_tx.send(AgentBusEvent::McpStatus {
            status: McpStatus {
                state: McpConnectionState::Error,
                endpoint,
                latency_ms: Some(latency_ms),
                last_error: Some(message.clone()),
            },
        });
        let _ = event_tx.send(AgentBusEvent::TurnFailed {
            agent_id: model,
            mission_id,
            thread_id: session_id,
            token_count,
            message,
        });
        return;
    }

    let _ = event_tx.send(AgentBusEvent::McpStatus {
        status: McpStatus {
            state: McpConnectionState::Connected,
            endpoint,
            latency_ms: Some(latency_ms),
            last_error: None,
        },
    });
    let _ = event_tx.send(AgentBusEvent::TurnCompleted {
        agent_id: model,
        mission_id,
        thread_id: session_id,
        token_count,
        message,
    });
}

fn claude_endpoint_label(agent_id: &str, resume_session_id: Option<&str>) -> String {
    let model_slug = claude_model_slug_for_agent_id(agent_id);
    let suffix = if model_slug == agent_id {
        String::new()
    } else {
        format!(" (agent {agent_id})")
    };
    if let Some(session_id) = resume_session_id {
        format!(
            "claude -p resume {} --model {model_slug}{suffix}",
            shorten_id(session_id),
        )
    } else {
        format!("claude -p --model {model_slug}{suffix}")
    }
}

pub fn claude_model_slug_for_agent_id(agent_id: &str) -> &str {
    // Same approach as `codex_runner::codex_model_slug_for_agent_id`:
    // every clone/lane suffix (`#swarm-…`, `#chat-clone-…`, `#shadow-…`,
    // `#mp-pane-…`) starts with `#`, and base model slugs never contain
    // one. Splitting on the FIRST `#` handles nested suffixes like
    // `claude-opus-4-7#mp-pane-01#swarm-mis-001-clone-01` correctly —
    // the multipane layer prepends its suffix and swarm later appends
    // its own.
    match agent_id.split_once('#') {
        Some((base, _)) if !base.trim().is_empty() => base,
        _ => agent_id,
    }
}

/// Build the `claude` subprocess `Command` with both the argv list AND the
/// per-pane working directory bound. `--add-dir` only mutates Claude's
/// allow-list — without `current_dir(cwd)` the child inherits nit's parent
/// cwd, so multipane prompts always ran in the workspace root regardless of
/// which directory the operator picked. This helper exists so the spawn-site
/// invariant (`cmd.get_current_dir() == Some(cwd)`) is testable.
#[allow(clippy::too_many_arguments)]
fn prepare_claude_command(
    agent_id: &str,
    cwd: &Path,
    persist_session: bool,
    effort: Option<&str>,
    resume_session_id: Option<&str>,
    read_only: bool,
    max_turns: Option<u32>,
    config: &ClaudeRunnerConfig,
) -> Command {
    let mut cmd = Command::new("claude");
    cmd.current_dir(cwd).args(build_claude_args(
        agent_id,
        cwd,
        persist_session,
        effort,
        resume_session_id,
        read_only,
        max_turns,
        config,
    ));
    cmd
}

fn add_skill_references(args: &mut Vec<String>) {
    if let Some(dir) = nit_core::skills::reference_dir() {
        args.extend(["--add-dir".into(), dir.display().to_string()]);
    }
    args.extend([
        "--append-system-prompt".into(),
        nit_core::skills::reference_hint(),
    ]);
}

fn add_tool_permissions(args: &mut Vec<String>, read_only: bool, config: &ClaudeRunnerConfig) {
    if read_only {
        args.extend([
            "--tools".into(),
            "Read,Glob,Grep".into(),
            "--allowedTools".into(),
            "Read,Glob,Grep".into(),
            "--strict-mcp-config".into(),
        ]);
    } else if let Some(mode) = config
        .permission_mode
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        args.push("--permission-mode".into());
        args.push(mode.to_string());
    } else {
        args.push("--allowedTools".into());
        args.push("Read,Edit,Write,Bash,Glob,Grep,WebSearch,WebFetch".into());
    }
}

#[allow(clippy::too_many_arguments)]
fn build_claude_args(
    agent_id: &str,
    cwd: &Path,
    persist_session: bool,
    effort: Option<&str>,
    resume_session_id: Option<&str>,
    read_only: bool,
    max_turns: Option<u32>,
    config: &ClaudeRunnerConfig,
) -> Vec<String> {
    let model_slug = claude_model_slug_for_agent_id(agent_id);
    let mut args = vec![
        "-p".into(),
        "--verbose".into(),
        "--output-format".into(),
        "stream-json".into(),
        "--model".into(),
        model_slug.to_string(),
    ];

    add_skill_references(&mut args);

    // Effort level.
    if let Some(effort) = effort.map(str::trim).filter(|s| !s.is_empty()) {
        args.push("--effort".into());
        args.push(effort.to_string());
    }

    // Working directory. Claude CLI uses --add-dir for additional directories, but the primary
    // CWD is set via the child process working directory. We pass it explicitly for clarity.
    args.push("--add-dir".into());
    args.push(cwd.to_string_lossy().to_string());

    // Session management.
    if let Some(session_id) = resume_session_id {
        args.push("--resume".into());
        args.push(session_id.to_string());
    }
    if !persist_session {
        args.push("--no-session-persistence".into());
    }

    add_tool_permissions(&mut args, read_only, config);

    // Max turns to prevent runaway sessions. Role-aware: integrators
    // routinely need more budget because they run real verify loops
    // (clippy → test → fmt → fix → re-check) and the default 50 is too
    // tight for those (observed: task completed but hit --max-turns during
    // cosmetic `cargo fmt` cleanup).
    args.push("--max-turns".into());
    args.push(max_turns.unwrap_or(DEFAULT_MAX_TURNS).to_string());

    // Read prompt from stdin (trailing `-`).
    args.push("-".into());

    args
}

fn shorten_id(id: &str) -> String {
    let id = id.trim();
    const MAX_CHARS: usize = 8;
    let Some((idx, _)) = id.char_indices().nth(MAX_CHARS) else {
        return id.to_string();
    };
    format!("{}…", &id[..idx])
}

// Claude emits `{"type":"system","subtype":"init","session_id":"..."}` at the
// start of each turn. Any event carrying a non-empty `session_id` works as a
// fallback in case the init line was dropped by a truncated stream.
fn extract_session_id_from_jsonl(stdout: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(stdout);
    for raw in text.lines() {
        let raw = raw.trim();
        if raw.is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<serde_json::Value>(raw) else {
            continue;
        };
        // Look for session_id in any event (prefer "system" / "init" events).
        if let Some(session_id) = value.get("session_id").and_then(|v| v.as_str()) {
            if !session_id.trim().is_empty() {
                return Some(session_id.to_string());
            }
        }
    }
    None
}

// When Claude uses tools during a turn, the stream contains multiple
// `assistant` events (text before tool use, text after). We collect every one
// so intermediate output (e.g. test results) isn't lost, and replace the last
// assistant chunk with the authoritative `result` event when both are present
// — the two normally duplicate each other.
fn extract_result_text_from_jsonl(stdout: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(stdout);
    let mut assistant_texts: Vec<String> = Vec::new();
    let mut result_text: Option<String> = None;
    for raw in text.lines() {
        let raw = raw.trim();
        if raw.is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<serde_json::Value>(raw) else {
            continue;
        };
        let kind = value.get("type").and_then(|v| v.as_str());
        if kind == Some("result") {
            if let Some(text) = value.get("result").and_then(|v| v.as_str()) {
                let trimmed = text.trim();
                if !trimmed.is_empty() {
                    result_text = Some(trimmed.to_string());
                }
            }
        }
        // Capture assistant text messages — there may be multiple when
        // Claude interleaves text with tool use.
        if kind == Some("assistant") {
            if let Some(message) = value.get("message").and_then(|v| {
                // The message may be a content block array or a plain string.
                if let Some(s) = v.as_str() {
                    Some(s.to_string())
                } else if let Some(content) = v.get("content").and_then(|c| c.as_array()) {
                    let texts: Vec<&str> = content
                        .iter()
                        .filter_map(|block| {
                            if block.get("type").and_then(|t| t.as_str()) == Some("text") {
                                block.get("text").and_then(|t| t.as_str())
                            } else {
                                None
                            }
                        })
                        .collect();
                    if texts.is_empty() {
                        None
                    } else {
                        Some(texts.join("\n"))
                    }
                } else {
                    None
                }
            }) {
                if !message.trim().is_empty() {
                    assistant_texts.push(message);
                }
            }
        }
    }

    // The result event typically duplicates the last assistant text.
    // Replace it so we don't double-up, then join all parts.
    if let Some(result) = result_text {
        if assistant_texts.is_empty() {
            return Some(result);
        }
        // Replace last assistant text with the (authoritative) result text.
        *assistant_texts.last_mut().unwrap() = result;
        Some(assistant_texts.join("\n\n"))
    } else if !assistant_texts.is_empty() {
        Some(assistant_texts.join("\n\n"))
    } else {
        None
    }
}

fn extract_token_count_from_jsonl(stdout: &[u8]) -> Option<AgentTokenCount> {
    let text = String::from_utf8_lossy(stdout);
    let mut last: Option<AgentTokenCount> = None;
    for raw in text.lines() {
        let raw = raw.trim();
        if raw.is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<serde_json::Value>(raw) else {
            continue;
        };
        if let Some(token_count) = claude_token_count_from_value(&value) {
            last = Some(token_count);
        }
    }
    last
}

// Token usage lives in different places depending on event kind:
// - `assistant` events → `value.message.usage`
// - `result` events    → `value.usage`, plus `value.modelUsage.<model>.contextWindow`
fn claude_token_count_from_value(value: &serde_json::Value) -> Option<AgentTokenCount> {
    let kind = value.get("type").and_then(|v| v.as_str());

    // Try multiple locations for usage data:
    // - "result" events: top-level "usage"
    // - "assistant" events: nested under "message.usage"
    let usage = value
        .get("usage")
        .or_else(|| value.get("message").and_then(|m| m.get("usage")))?;

    let input = usage
        .get("input_tokens")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let output = usage
        .get("output_tokens")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    // Cache tokens count towards context usage.
    let cache_creation = usage
        .get("cache_creation_input_tokens")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let cache_read = usage
        .get("cache_read_input_tokens")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);

    let total = input
        .saturating_add(output)
        .saturating_add(cache_creation)
        .saturating_add(cache_read);
    if total == 0 || total > u32::MAX as u64 {
        return None;
    }

    // Extract context window from modelUsage in "result" events:
    // {"modelUsage":{"claude-opus-4-6":{"contextWindow":200000}}}
    let mut context_window = 0u64;
    if kind == Some("result") {
        if let Some(model_usage) = value.get("modelUsage").and_then(|v| v.as_object()) {
            for (_model, info) in model_usage {
                if let Some(cw) = info.get("contextWindow").and_then(|v| v.as_u64()) {
                    context_window = cw;
                    break;
                }
            }
        }
    }

    Some(AgentTokenCount {
        total_tokens: total as u32,
        context_window: context_window as u32,
    })
}

/// Why the runner killed the Claude subprocess before it exited on its own.
/// Drives post-loop dispatch: operator cancel goes down the soft-cancel path,
/// while runner-driven kills (`ResultSeen` / `IdleTimeout`) try to recover the
/// final message from the buffered stream-json so the swarm can still proceed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TurnKillReason {
    OperatorCancel,
    ResultSeen,
    IdleTimeout,
}

/// Idle-output reaper for stuck Claude turns. Mirrors `mcp_turn_idle_timeout`
/// in `codex_runner.rs` but defaults to **on** at 15 minutes — Claude CLI can
/// linger indefinitely waiting on backgrounded tool subshells even after the
/// model has emitted its final `result` event, and there is no upstream
/// reaper to catch that.
///
/// The reaper only fires on read-only / verifier-style turns: any turn that
/// invokes a write-capable tool (Write/Edit/MultiEdit/NotebookEdit) is
/// flagged as a writer and exempted, on the assumption that writers are
/// productive even during long stream silences. Operators retain `/abort`
/// for the rare case where a writer turn genuinely wedges.
///
/// - Unset env var → `Some(900s)` (default-on safety net).
/// - Empty / unparseable value → `Some(900s)`.
/// - `"0"` → `None` (explicit disable).
/// - Positive integer → `Some(N seconds)`.
fn claude_turn_idle_timeout() -> Option<Duration> {
    const DEFAULT_IDLE_TIMEOUT_SECS: u64 = 15 * 60;
    let default = Duration::from_secs(DEFAULT_IDLE_TIMEOUT_SECS);
    match std::env::var("NIT_CLAUDE_TURN_IDLE_TIMEOUT_SECS") {
        Ok(raw) => {
            let raw = raw.trim();
            if raw.is_empty() {
                return Some(default);
            }
            match raw.parse::<u64>() {
                Ok(0) => None,
                Ok(secs) => Some(Duration::from_secs(secs)),
                Err(_) => Some(default),
            }
        }
        Err(_) => Some(default),
    }
}

/// `true` when `value` is a Claude stream-json `{"type":"result", ...}`
/// envelope — the canonical end-of-turn marker.
fn is_stream_result_event(value: &serde_json::Value) -> bool {
    value["type"] == "result"
}

/// Production [`WorkerSpawn`] that boots a real `claude` CLI process with the
/// long-lived `--input-format stream-json --output-format stream-json` pair.
///
/// Pool slots are baked at default `--max-turns` / no `--effort` /
/// `--no-session-persistence` — only "vanilla" turns are eligible (see
/// [`turn_supports_pool`]). Specialised turns (integrators with custom
/// `--max-turns`, resumed sessions, custom effort levels) bypass the pool
/// entirely and take the cold-spawn path.
pub(crate) struct ClaudeCliSpawner {
    config: ClaudeRunnerConfig,
}

impl ClaudeCliSpawner {
    pub(crate) fn new(config: ClaudeRunnerConfig) -> Self {
        Self { config }
    }
}

impl WorkerSpawn for ClaudeCliSpawner {
    fn spawn(&self, key: &WorkerKey, _agent_id: &str) -> std::io::Result<SpawnedChild> {
        let mut cmd = Command::new("claude");
        cmd.current_dir(&key.cwd)
            .args(build_pool_worker_args(key, &self.config))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = cmd.spawn()?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| std::io::Error::other("pool: missing stdin"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| std::io::Error::other("pool: missing stdout"))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| std::io::Error::other("pool: missing stderr"))?;
        Ok(SpawnedChild {
            child,
            stdin,
            stdout,
            stderr,
        })
    }
}

fn build_pool_worker_args(key: &WorkerKey, config: &ClaudeRunnerConfig) -> Vec<String> {
    let mut args = vec![
        "-p".into(),
        "--verbose".into(),
        "--input-format".into(),
        "stream-json".into(),
        "--output-format".into(),
        "stream-json".into(),
        "--model".into(),
        key.model_slug.clone(),
        "--add-dir".into(),
        key.cwd.to_string_lossy().to_string(),
        "--no-session-persistence".into(),
    ];
    add_skill_references(&mut args);
    add_tool_permissions(&mut args, key.read_only, config);
    args.push("--max-turns".into());
    args.push(DEFAULT_MAX_TURNS.to_string());
    args
}

#[allow(clippy::too_many_arguments)]
fn run_turn_pooled(
    event_tx: &Sender<AgentBusEvent>,
    mut worker: PoolWorker,
    _seq: u64,
    model: String,
    cwd: PathBuf,
    mission_id: Option<String>,
    resume_session_id: Option<String>,
    prompt: String,
    cancel: Arc<AtomicBool>,
) {
    let started_at = Instant::now();
    let endpoint = claude_endpoint_label(&model, resume_session_id.as_deref());
    let envelope = build_stream_json_envelope(&prompt);

    if let Err(err) = worker.write_envelope(envelope.as_bytes()) {
        worker.recycle(RecycleReason::BrokenPipe);
        let _ = event_tx.send(AgentBusEvent::McpStatus {
            status: McpStatus {
                state: McpConnectionState::Error,
                endpoint,
                latency_ms: None,
                last_error: Some(format!("pool envelope write failed: {err}")),
            },
        });
        let _ = event_tx.send(AgentBusEvent::TurnFailed {
            agent_id: model,
            mission_id,
            thread_id: resume_session_id,
            token_count: None,
            message: format!("Claude pool: envelope write failed: {err}"),
        });
        return;
    }

    let _ = event_tx.send(AgentBusEvent::McpStatus {
        status: McpStatus {
            state: McpConnectionState::Connected,
            endpoint: endpoint.clone(),
            latency_ms: None,
            last_error: None,
        },
    });

    let mut state = TurnStream::new();
    let idle_timeout = claude_turn_idle_timeout();
    let mut kill_reason: Option<TurnKillReason> = None;
    let mut last_heartbeat_at = Instant::now();
    let mut last_stdout_at = Instant::now();

    loop {
        if kill_reason.is_none() && last_heartbeat_at.elapsed() >= Duration::from_secs(2) {
            let _ = event_tx.send(AgentBusEvent::TurnHeartbeat {
                agent_id: model.clone(),
                mission_id: mission_id.clone(),
            });
            last_heartbeat_at = Instant::now();
        }

        if kill_reason.is_none() {
            if cancel.load(Ordering::Relaxed) {
                worker.kill_child();
                kill_reason = Some(TurnKillReason::OperatorCancel);
            } else if state.result_seen {
                break;
            } else if worker.is_dead() {
                kill_reason = Some(TurnKillReason::ResultSeen);
                break;
            } else if let Some(timeout) = idle_timeout {
                if !state.turn_did_writes && last_stdout_at.elapsed() >= timeout {
                    worker.kill_child();
                    kill_reason = Some(TurnKillReason::IdleTimeout);
                }
            }
        }

        if kill_reason.is_some() {
            break;
        }

        let Some(line) = worker.recv_timeout(Duration::from_millis(50)) else {
            continue;
        };
        last_stdout_at = Instant::now();
        state.observe(&line.raw, event_tx, &model, &mission_id, &cwd);
    }

    let stdout = std::mem::take(&mut state.stdout_buf);
    let json_errors = std::mem::take(&mut state.json_errors);
    let stderr = worker.stderr_snapshot();

    let unhealthy = matches!(
        kill_reason,
        Some(TurnKillReason::IdleTimeout) | Some(TurnKillReason::OperatorCancel)
    ) || worker.is_dead();
    let result_only = matches!(kill_reason, Some(TurnKillReason::ResultSeen)) && !worker.is_dead();
    if unhealthy {
        worker.recycle(match kill_reason {
            Some(TurnKillReason::OperatorCancel) => RecycleReason::Cancel,
            Some(TurnKillReason::IdleTimeout) => RecycleReason::IdleTimeout,
            _ => RecycleReason::NonZeroExit,
        });
    } else if result_only {
        // Worker still alive after `result` — return slot for reuse. CLI
        // protocol may or may not actually serve another turn; the next
        // checkout validates via try_wait and recycles on failure.
        worker.check_in();
    } else {
        // Reached the break via `state.result_seen`, worker alive.
        worker.check_in();
    }

    if kill_reason == Some(TurnKillReason::OperatorCancel) {
        let _ = event_tx.send(AgentBusEvent::TurnFailed {
            agent_id: model,
            mission_id,
            thread_id: resume_session_id,
            token_count: None,
            message: nit_core::OPERATOR_CANCEL_TURN_MESSAGE.into(),
        });
        return;
    }

    if let Some(reason) = kill_reason {
        let elapsed_secs = started_at.elapsed().as_secs();
        let msg = match reason {
            TurnKillReason::ResultSeen => format!(
                "Claude pool turn closed after `result` event ({elapsed_secs}s)"
            ),
            TurnKillReason::IdleTimeout => format!(
                "Claude pool turn killed by idle timeout after {}s of stream silence; attempting to recover the final message from buffered stream-json",
                idle_timeout.map(|d| d.as_secs()).unwrap_or_default(),
            ),
            TurnKillReason::OperatorCancel => unreachable!(),
        };
        let _ = event_tx.send(AgentBusEvent::TurnLog {
            agent_id: model.clone(),
            message: msg,
        });
    }

    let stderr_text = String::from_utf8_lossy(&stderr);
    for line in stderr_text.lines() {
        let line = line.trim();
        if !line.is_empty() {
            let _ = event_tx.send(AgentBusEvent::TurnLog {
                agent_id: model.clone(),
                message: line.to_string(),
            });
        }
    }

    if report_terminal_error(
        &stdout,
        event_tx,
        &model,
        &mission_id,
        resume_session_id.as_deref(),
        started_at,
    ) {
        return;
    }
    let message = extract_result_text_from_jsonl(&stdout).unwrap_or_default();
    if message.is_empty() {
        let session_id = extract_session_id_from_jsonl(&stdout);
        let token_count = extract_token_count_from_jsonl(&stdout);
        let failure_message = match kill_reason {
            Some(TurnKillReason::IdleTimeout) => format!(
                "Claude pool turn killed by idle timeout after {}s of stream silence; no extractable result.",
                idle_timeout.map(|d| d.as_secs()).unwrap_or_default(),
            ),
            _ => {
                if !json_errors.is_empty() {
                    json_errors.join(" | ")
                } else if !stderr_text.trim().is_empty() {
                    stderr_text.trim().to_string()
                } else {
                    "Claude pool turn finished but produced an empty last message.".into()
                }
            }
        };
        let _ = event_tx.send(AgentBusEvent::McpStatus {
            status: McpStatus {
                state: McpConnectionState::Error,
                endpoint,
                latency_ms: Some(elapsed_ms(started_at)),
                last_error: Some(failure_message.clone()),
            },
        });
        let _ = event_tx.send(AgentBusEvent::TurnFailed {
            agent_id: model,
            mission_id,
            thread_id: session_id,
            token_count,
            message: failure_message,
        });
        return;
    }

    let session_id = extract_session_id_from_jsonl(&stdout);
    let token_count = extract_token_count_from_jsonl(&stdout);

    if is_provider_quota_exhausted_in_result(&message) {
        let _ = event_tx.send(AgentBusEvent::McpStatus {
            status: McpStatus {
                state: McpConnectionState::Error,
                endpoint,
                latency_ms: Some(elapsed_ms(started_at)),
                last_error: Some(message.clone()),
            },
        });
        let _ = event_tx.send(AgentBusEvent::TurnFailed {
            agent_id: model,
            mission_id,
            thread_id: session_id,
            token_count,
            message,
        });
        return;
    }

    let _ = event_tx.send(AgentBusEvent::McpStatus {
        status: McpStatus {
            state: McpConnectionState::Connected,
            endpoint,
            latency_ms: Some(elapsed_ms(started_at)),
            last_error: None,
        },
    });
    let _ = event_tx.send(AgentBusEvent::TurnCompleted {
        agent_id: model,
        mission_id,
        thread_id: session_id,
        token_count,
        message,
    });
}

fn terminal_result_error(stdout: &[u8]) -> Option<String> {
    let result = stdout
        .rsplit(|byte| *byte == b'\n')
        .filter_map(|line| serde_json::from_slice::<serde_json::Value>(line).ok())
        .find(is_stream_result_event)?;
    let subtype = result["subtype"].as_str().unwrap_or_default();
    if result["is_error"].as_bool() != Some(true) && !subtype.starts_with("error_") {
        return if result["result"]
            .as_str()
            .is_some_and(|s| !s.trim().is_empty())
        {
            None
        } else {
            Some("Claude emitted an incomplete result.".into())
        };
    }
    if let Some(message) = result["result"]
        .as_str()
        .filter(|msg| !msg.trim().is_empty())
    {
        return Some(message.to_string());
    }
    let errors: Vec<_> = result["errors"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|value| value.as_str())
        .collect();
    Some(if errors.is_empty() {
        format!("Claude reported an unsuccessful turn ({subtype}).")
    } else {
        errors.join(" | ")
    })
}

fn report_terminal_error(
    stdout: &[u8],
    event_tx: &Sender<AgentBusEvent>,
    model: &str,
    mission_id: &Option<String>,
    resume_session: Option<&str>,
    started_at: Instant,
) -> bool {
    let Some(message) = terminal_result_error(stdout) else {
        return false;
    };
    let _ = event_tx.send(AgentBusEvent::McpStatus {
        status: McpStatus {
            state: McpConnectionState::Error,
            endpoint: claude_endpoint_label(model, resume_session),
            latency_ms: Some(elapsed_ms(started_at)),
            last_error: Some(message.clone()),
        },
    });
    let _ = event_tx.send(AgentBusEvent::TurnFailed {
        agent_id: model.to_string(),
        mission_id: mission_id.clone(),
        thread_id: extract_session_id_from_jsonl(stdout),
        token_count: extract_token_count_from_jsonl(stdout),
        message,
    });
    true
}

fn elapsed_ms(started_at: Instant) -> u64 {
    started_at.elapsed().as_millis().min(u64::MAX as u128) as u64
}

struct TurnStream {
    result_seen: bool,
    turn_did_writes: bool,
    stdout_buf: Vec<u8>,
    json_errors: Vec<String>,
    last_stage: Option<String>,
    last_stage_sent_at: Instant,
}

impl TurnStream {
    fn observe(
        &mut self,
        line: &str,
        event_tx: &Sender<AgentBusEvent>,
        model: &str,
        mission_id: &Option<String>,
        cwd: &Path,
    ) {
        let obs = process_stream_json_line(
            line,
            &mut self.stdout_buf,
            &mut self.json_errors,
            &mut self.last_stage,
            &mut self.last_stage_sent_at,
            event_tx,
            model,
            mission_id,
            cwd,
        );
        self.result_seen |= obs.result_seen;
        self.turn_did_writes |= obs.turn_did_writes;
    }

    fn new() -> Self {
        Self {
            result_seen: false,
            turn_did_writes: false,
            stdout_buf: Vec::new(),
            json_errors: Vec::new(),
            last_stage: None,
            last_stage_sent_at: Instant::now(),
        }
    }
}

#[derive(Default)]
struct StreamObservations {
    result_seen: bool,
    turn_did_writes: bool,
}

// Shared stream-json event parser for cold and pooled turns.
#[allow(clippy::too_many_arguments)]
fn process_stream_json_line(
    raw_line: &str,
    stdout_buf: &mut Vec<u8>,
    json_errors: &mut Vec<String>,
    last_stage: &mut Option<String>,
    last_stage_sent_at: &mut Instant,
    event_tx: &Sender<AgentBusEvent>,
    model: &str,
    mission_id: &Option<String>,
    cwd: &Path,
) -> StreamObservations {
    let mut obs = StreamObservations::default();
    append_stdout_line_capped(stdout_buf, raw_line.as_bytes());
    let trimmed = raw_line.trim();
    if trimmed.is_empty() {
        return obs;
    }
    let Ok(value) = serde_json::from_str::<serde_json::Value>(trimmed) else {
        let _ = event_tx.send(AgentBusEvent::TurnLog {
            agent_id: model.to_string(),
            message: trimmed.to_string(),
        });
        return obs;
    };

    if is_stream_result_event(&value) {
        obs.result_seen = true;
    }

    if let Some(token_count) = claude_token_count_from_value(&value) {
        let _ = event_tx.send(AgentBusEvent::TokenCount {
            agent_id: model.to_string(),
            mission_id: mission_id.clone(),
            token_count,
        });
    }

    let kind = value.get("type").and_then(|v| v.as_str());
    if let Some(kind) = kind {
        emit_stage_event_if_changed(
            kind,
            &value,
            last_stage,
            last_stage_sent_at,
            event_tx,
            model,
            mission_id,
        );
    }

    if kind == Some("error") {
        if let Some(msg) = value
            .get("error")
            .and_then(|v| v.as_str())
            .or_else(|| value.get("message").and_then(|v| v.as_str()))
        {
            push_json_error_capped(json_errors, msg.to_string());
            let _ = event_tx.send(AgentBusEvent::TurnLog {
                agent_id: model.to_string(),
                message: msg.to_string(),
            });
        }
    }

    if kind == Some("assistant") {
        scan_assistant_tool_uses(&value, &mut obs, event_tx, model, mission_id, cwd);
    }

    obs
}

fn emit_stage_event_if_changed(
    kind: &str,
    value: &serde_json::Value,
    last_stage: &mut Option<String>,
    last_stage_sent_at: &mut Instant,
    event_tx: &Sender<AgentBusEvent>,
    model: &str,
    mission_id: &Option<String>,
) {
    let is_interesting = matches!(
        kind,
        "system"
            | "assistant"
            | "content_block_start"
            | "tool_use"
            | "tool_result"
            | "result"
            | "error"
    );
    if !is_interesting {
        return;
    }
    let stage = derive_stage_label(kind, value);
    let stage_changed = last_stage.as_deref() != Some(stage.as_str());
    if !stage_changed && last_stage_sent_at.elapsed() < Duration::from_secs(1) {
        return;
    }
    *last_stage = Some(stage.clone());
    *last_stage_sent_at = Instant::now();
    let _ = event_tx.send(AgentBusEvent::TurnStage {
        agent_id: model.to_string(),
        mission_id: mission_id.clone(),
        stage,
    });
}

fn scan_assistant_tool_uses(
    value: &serde_json::Value,
    obs: &mut StreamObservations,
    event_tx: &Sender<AgentBusEvent>,
    model: &str,
    mission_id: &Option<String>,
    cwd: &Path,
) {
    let Some(content) = value
        .get("message")
        .and_then(|m| m.get("content"))
        .and_then(|c| c.as_array())
    else {
        return;
    };
    for block in content {
        if block.get("type").and_then(|v| v.as_str()) != Some("tool_use") {
            continue;
        }
        let tool_name = block.get("name").and_then(|v| v.as_str());
        let input = block.get("input");
        if !tool_invokes_writes(tool_name, input) {
            continue;
        }
        obs.turn_did_writes = true;
        emit_file_write_if_path_present(input, event_tx, model, mission_id, cwd);
    }
}

fn emit_file_write_if_path_present(
    input: Option<&serde_json::Value>,
    event_tx: &Sender<AgentBusEvent>,
    model: &str,
    mission_id: &Option<String>,
    cwd: &Path,
) {
    for key in ["file_path", "path", "file"] {
        let Some(raw) = input.and_then(|v| v.get(key)).and_then(|v| v.as_str()) else {
            continue;
        };
        let path = if std::path::Path::new(raw).is_absolute() {
            std::path::PathBuf::from(raw)
        } else {
            cwd.join(raw)
        };
        let _ = event_tx.send(AgentBusEvent::FileWrite {
            agent_id: model.to_string(),
            mission_id: mission_id.clone(),
            path,
        });
        return;
    }
}

fn derive_stage_label(kind: &str, value: &serde_json::Value) -> String {
    match kind {
        "assistant" => {
            let subtype = value
                .get("subtype")
                .and_then(|v| v.as_str())
                .unwrap_or("text");
            format!("assistant({subtype})")
        }
        "content_block_start" => {
            let block_type = value
                .get("content_block")
                .and_then(|b| b.get("type"))
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            match block_type {
                "tool_use" => {
                    let name = value
                        .get("content_block")
                        .and_then(|b| b.get("name"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("tool");
                    format!("tool_use({name})")
                }
                _ => format!("content({block_type})"),
            }
        }
        "tool_use" | "tool_result" => {
            let tool_name = value
                .get("tool")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            format!("{kind}({tool_name})")
        }
        other => other.to_string(),
    }
}

#[cfg(test)]
#[path = "tests/claude_runner.rs"]
mod tests;
