use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use nit_core::{AgentBusEvent, AgentTokenCount, McpConnectionState, McpStatus};

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
    pub fn spawn(config: ClaudeRunnerConfig) -> Self {
        let (cmd_tx, cmd_rx) = mpsc::channel();
        let (event_tx, event_rx) = mpsc::channel();
        let shutdown = Arc::new(AtomicBool::new(false));
        let shutdown_worker = Arc::clone(&shutdown);
        let handle = thread::Builder::new()
            .name("nit-claude".into())
            .spawn(move || runner_loop(config, shutdown_worker, cmd_rx, event_tx))
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

fn runner_loop(
    config: ClaudeRunnerConfig,
    _shutdown: Arc<AtomicBool>,
    cmd_rx: Receiver<ClaudeCommand>,
    event_tx: Sender<AgentBusEvent>,
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
}

struct ActiveTurn {
    agent_id: String,
    cancel: Arc<AtomicBool>,
    done_rx: Receiver<()>,
    handle: JoinHandle<()>,
}

struct StdoutCapture {
    stdout: Vec<u8>,
    json_errors: Vec<String>,
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
) -> ActiveTurn {
    let agent_id = model.clone();
    let cancel = Arc::new(AtomicBool::new(false));
    let (done_tx, done_rx) = mpsc::channel();
    let event_tx = event_tx.clone();
    let cancel_worker = Arc::clone(&cancel);
    let handle = thread::Builder::new()
        .name(format!("nit-claude-turn-{seq}"))
        .spawn(move || {
            run_turn(
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
    let started_at = Instant::now();
    let out_file = std::env::temp_dir().join(format!("nit-claude-last-message-{seq}.txt"));

    let mut cmd = Command::new("claude");
    cmd.args(build_claude_args(
        model.as_str(),
        cwd.as_path(),
        persist_session,
        effort.as_deref(),
        out_file.as_path(),
        resume_session_id.as_deref(),
        read_only,
        max_turns,
        &config,
    ))
    .stdin(Stdio::piped())
    .stdout(Stdio::piped())
    .stderr(Stdio::piped());

    let mut child = match cmd.spawn() {
        Ok(child) => child,
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

    let stdout_handle = child.stdout.take().map(|stdout| {
        let event_tx = event_tx.clone();
        let model = model.clone();
        let mission_id = mission_id.clone();
        thread::Builder::new()
            .name(format!("nit-claude-stdout-{seq}"))
            .spawn(move || {
                let mut buf = Vec::new();
                let mut json_errors = Vec::new();
                let mut last_stage: Option<String> = None;
                let mut last_stage_sent_at = Instant::now();
                let mut reader = BufReader::new(stdout);
                let mut line = String::new();
                loop {
                    line.clear();
                    match reader.read_line(&mut line) {
                        Ok(0) => break,
                        Ok(_) => {
                            append_stdout_line_capped(&mut buf, line.as_bytes());
                            let raw = line.trim();
                            if raw.is_empty() {
                                continue;
                            }
                            let Ok(value) = serde_json::from_str::<serde_json::Value>(raw) else {
                                let _ = event_tx.send(AgentBusEvent::TurnLog {
                                    agent_id: model.clone(),
                                    message: raw.to_string(),
                                });
                                continue;
                            };

                            // Extract token usage from Claude stream-json events.
                            if let Some(token_count) = claude_token_count_from_value(&value) {
                                let _ = event_tx.send(AgentBusEvent::TokenCount {
                                    agent_id: model.clone(),
                                    mission_id: mission_id.clone(),
                                    token_count,
                                });
                            }

                            // Claude stream-json uses "type" at top level.
                            let kind = value.get("type").and_then(|v| v.as_str());
                            if let Some(kind) = kind {
                                let stage = match kind {
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
                                    _ => kind.to_string(),
                                };
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
                                if is_interesting
                                    && (last_stage.as_deref() != Some(stage.as_str())
                                        || last_stage_sent_at.elapsed() >= Duration::from_secs(1))
                                {
                                    last_stage = Some(stage.clone());
                                    last_stage_sent_at = Instant::now();
                                    let _ = event_tx.send(AgentBusEvent::TurnStage {
                                        agent_id: model.clone(),
                                        mission_id: mission_id.clone(),
                                        stage,
                                    });
                                }
                            }
                            if kind == Some("error") {
                                let msg = value
                                    .get("error")
                                    .and_then(|v| v.as_str())
                                    .or_else(|| value.get("message").and_then(|v| v.as_str()));
                                if let Some(msg) = msg {
                                    push_json_error_capped(&mut json_errors, msg.to_string());
                                    let _ = event_tx.send(AgentBusEvent::TurnLog {
                                        agent_id: model.clone(),
                                        message: msg.to_string(),
                                    });
                                }
                            }
                            // Detect file writes for per-agent genome attribution
                            // AND the substrate claim lattice. Claude CLI
                            // stream-json nests tool_use blocks inside
                            // `assistant.message.content[]` — there is no
                            // top-level `content_block_start` event. Filter to
                            // write-capable tools so Read/Glob/Grep don't
                            // spuriously emit FileWrite. Do NOT gate on
                            // path.exists() — Write creating a new file
                            // legitimately targets a path that doesn't exist yet.
                            if kind == Some("assistant") {
                                if let Some(content) = value
                                    .get("message")
                                    .and_then(|m| m.get("content"))
                                    .and_then(|c| c.as_array())
                                {
                                    for block in content {
                                        let block_type = block.get("type").and_then(|v| v.as_str());
                                        if block_type != Some("tool_use") {
                                            continue;
                                        }
                                        let tool_name = block.get("name").and_then(|v| v.as_str());
                                        let is_write_tool = matches!(
                                            tool_name,
                                            Some("Write")
                                                | Some("Edit")
                                                | Some("MultiEdit")
                                                | Some("NotebookEdit")
                                        );
                                        if !is_write_tool {
                                            continue;
                                        }
                                        for key in ["file_path", "path", "file"] {
                                            if let Some(p) = block
                                                .get("input")
                                                .and_then(|v| v.get(key))
                                                .and_then(|v| v.as_str())
                                            {
                                                let path = if std::path::Path::new(p).is_absolute()
                                                {
                                                    std::path::PathBuf::from(p)
                                                } else {
                                                    cwd.join(p)
                                                };
                                                let _ = event_tx.send(AgentBusEvent::FileWrite {
                                                    agent_id: model.clone(),
                                                    mission_id: mission_id.clone(),
                                                    path,
                                                });
                                                break;
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        Err(_) => break,
                    }
                }
                StdoutCapture {
                    stdout: buf,
                    json_errors,
                }
            })
            .expect("spawn claude stdout reader")
    });
    let stderr_handle = child.stderr.take().map(|mut stderr| {
        thread::Builder::new()
            .name(format!("nit-claude-stderr-{seq}"))
            .spawn(move || {
                let mut buf = Vec::new();
                let _ = stderr.read_to_end(&mut buf);
                buf
            })
            .expect("spawn claude stderr reader")
    });

    let mut killed = false;
    let mut last_heartbeat_at = Instant::now();
    let status = loop {
        if !killed && last_heartbeat_at.elapsed() >= Duration::from_secs(2) {
            let _ = event_tx.send(AgentBusEvent::TurnHeartbeat {
                agent_id: model.clone(),
                mission_id: mission_id.clone(),
            });
            last_heartbeat_at = Instant::now();
        }
        if cancel.load(Ordering::Relaxed) && !killed {
            let _ = child.kill();
            killed = true;
        }
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => thread::sleep(Duration::from_millis(50)),
            Err(err) => {
                let _ = event_tx.send(AgentBusEvent::TurnFailed {
                    agent_id: model,
                    mission_id,
                    thread_id: resume_session_id.clone(),
                    token_count: None,
                    message: format!("Claude wait failed: {err}"),
                });
                let _ = std::fs::remove_file(&out_file);
                return;
            }
        }
    };

    let (stdout, json_errors) = match stdout_handle.and_then(|handle| handle.join().ok()) {
        Some(capture) => (capture.stdout, capture.json_errors),
        None => (Vec::new(), Vec::new()),
    };
    let stderr = stderr_handle
        .and_then(|handle| handle.join().ok())
        .unwrap_or_default();

    if killed {
        let _ = std::fs::remove_file(&out_file);
        return;
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

    if !status.success() {
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
        let _ = std::fs::remove_file(&out_file);
        return;
    }

    // Claude -p writes the final result to stdout as part of stream-json. We also attempt to
    // read the out_file (from the -o flag fallback, if applicable), and fall back to extracting
    // the result text from the JSONL stream.
    let message = std::fs::read_to_string(&out_file)
        .ok()
        .map(|s| s.trim_end().to_string())
        .filter(|s| !s.is_empty())
        .or_else(|| extract_result_text_from_jsonl(&stdout))
        .unwrap_or_default();
    let _ = std::fs::remove_file(&out_file);

    if message.is_empty() {
        let session_id = extract_session_id_from_jsonl(&stdout);
        let token_count = extract_token_count_from_jsonl(&stdout);
        let _ = event_tx.send(AgentBusEvent::TurnFailed {
            agent_id: model,
            mission_id,
            thread_id: session_id,
            token_count,
            message: "Claude finished but produced an empty last message.".into(),
        });
        return;
    }

    let session_id = extract_session_id_from_jsonl(&stdout);
    let token_count = extract_token_count_from_jsonl(&stdout);
    let latency_ms = started_at.elapsed().as_millis().min(u64::MAX as u128) as u64;
    let endpoint = claude_endpoint_label(&model, resume_session_id.as_deref());
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
    agent_id
        .split_once("#swarm-")
        .or_else(|| agent_id.split_once("#chat-clone-"))
        .or_else(|| agent_id.split_once("#shadow-"))
        .map(|(base, _)| {
            if base.trim().is_empty() {
                agent_id
            } else {
                base
            }
        })
        .unwrap_or(agent_id)
}

#[allow(clippy::too_many_arguments)]
fn build_claude_args(
    agent_id: &str,
    cwd: &Path,
    persist_session: bool,
    effort: Option<&str>,
    _out_file: &Path,
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

    // Permission handling: auto-allow common tools for headless operation.
    // Shadow/advisory turns restrict tools to read-only so the subprocess
    // can't edit files or run commands regardless of what the prompt says.
    if read_only {
        args.push("--allowedTools".into());
        args.push("Read,Glob,Grep".into());
    } else if let Some(mode) = config
        .permission_mode
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        args.push("--permission-mode".into());
        args.push(mode.to_string());
    } else {
        // Default: allow standard tools for autonomous operation.
        args.push("--allowedTools".into());
        args.push("Read,Edit,Write,Bash,Glob,Grep,WebSearch,WebFetch".into());
    }

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

#[cfg(test)]
#[path = "tests/claude_runner.rs"]
mod tests;
