use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use nit_core::{AgentBusEvent, AgentTokenCount, McpConnectionState, McpStatus};

#[derive(Clone, Debug, Default)]
pub struct CodexRunnerConfig {
    pub sandbox: Option<String>,
    pub approval_policy: Option<String>,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum CodexRuntimeMode {
    Exec,
    Mcp,
}

#[derive(Clone, Debug)]
pub enum CodexCommand {
    RunTurn {
        model: String,
        cwd: PathBuf,
        mission_id: Option<String>,
        /// Resume an existing Codex session/thread id when present.
        resume_thread_id: Option<String>,
        /// When false, run the turn as `--ephemeral` (no on-disk Codex session state).
        /// When true, persist the session so future turns can resume it.
        persist_session: bool,
        reasoning_effort: Option<String>,
        prompt: String,
    },
    McpStart,
    McpStop,
    McpReconnect,
    Shutdown,
}

pub struct CodexRunner {
    cmd_tx: Sender<CodexCommand>,
    pub events: Receiver<AgentBusEvent>,
    handle: Option<JoinHandle<()>>,
}

impl CodexRunner {
    pub fn spawn(mode: CodexRuntimeMode, config: CodexRunnerConfig) -> Self {
        let (cmd_tx, cmd_rx) = mpsc::channel();
        let (event_tx, event_rx) = mpsc::channel();
        let handle = thread::Builder::new()
            .name("nit-codex".into())
            .spawn(move || runner_loop(mode, config, cmd_rx, event_tx))
            .expect("spawn codex runner");
        Self {
            cmd_tx,
            events: event_rx,
            handle: Some(handle),
        }
    }

    pub fn send(&self, command: CodexCommand) {
        let _ = self.cmd_tx.send(command);
    }

    pub fn shutdown(&mut self) {
        let _ = self.cmd_tx.send(CodexCommand::Shutdown);
        let Some(handle) = self.handle.take() else {
            return;
        };

        // Ensure quitting the TUI can't hang indefinitely if Codex is stuck. The runner loop
        // applies a short shutdown deadline; join in a helper thread with a matching timeout.
        let (done_tx, done_rx) = mpsc::channel();
        let _ = thread::Builder::new()
            .name("nit-codex-join".into())
            .spawn(move || {
                let _ = handle.join();
                let _ = done_tx.send(());
            });
        let _ = done_rx.recv_timeout(Duration::from_millis(400));
    }
}

fn runner_loop(
    mode: CodexRuntimeMode,
    config: CodexRunnerConfig,
    cmd_rx: Receiver<CodexCommand>,
    event_tx: Sender<AgentBusEvent>,
) {
    match mode {
        CodexRuntimeMode::Exec => runner_loop_exec(cmd_rx, event_tx, config),
        CodexRuntimeMode::Mcp => runner_loop_mcp(cmd_rx, event_tx, config),
    }
}

fn runner_loop_exec(
    cmd_rx: Receiver<CodexCommand>,
    event_tx: Sender<AgentBusEvent>,
    config: CodexRunnerConfig,
) {
    let mut seq = 0u64;
    let mut queue: VecDeque<CodexCommand> = VecDeque::new();
    let mut active: Option<ActiveTurn> = None;
    let mut shutting_down = false;
    let mut shutdown_deadline: Option<Instant> = None;

    loop {
        // Keep this control loop responsive so `Shutdown` can cancel an in-flight Codex process.
        let cmd = if active.is_none() && queue.is_empty() && !shutting_down {
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
                CodexCommand::RunTurn { .. } if !shutting_down => queue.push_back(cmd),
                CodexCommand::RunTurn { .. } => {}
                CodexCommand::McpStart | CodexCommand::McpStop | CodexCommand::McpReconnect => {}
                CodexCommand::Shutdown => {
                    shutting_down = true;
                    shutdown_deadline = Some(Instant::now() + Duration::from_millis(400));
                    queue.clear();
                    if let Some(active) = active.as_ref() {
                        active.cancel.store(true, Ordering::Relaxed);
                    }
                }
            }
        }

        if let Some(turn) = active.as_mut() {
            match turn.done_rx.try_recv() {
                Ok(()) | Err(mpsc::TryRecvError::Disconnected) => {
                    let turn = active.take().expect("active turn");
                    let _ = turn.handle.join();
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }

        if active.is_none() && !shutting_down {
            if let Some(cmd) = queue.pop_front() {
                if let CodexCommand::RunTurn {
                    model,
                    cwd,
                    mission_id,
                    resume_thread_id,
                    persist_session,
                    reasoning_effort,
                    prompt,
                } = cmd
                {
                    let _ = event_tx.send(AgentBusEvent::McpStatus {
                        status: McpStatus {
                            state: McpConnectionState::Connecting,
                            endpoint: codex_exec_endpoint_label(
                                &model,
                                resume_thread_id.as_deref(),
                            ),
                            latency_ms: None,
                            last_error: None,
                        },
                    });
                    let _ = event_tx.send(AgentBusEvent::TurnStarted {
                        agent_id: model.clone(),
                        mission_id: mission_id.clone(),
                        resume_thread_id: resume_thread_id.clone(),
                    });
                    seq = seq.wrapping_add(1);
                    active = Some(spawn_turn_worker(
                        &event_tx,
                        seq,
                        model,
                        cwd,
                        mission_id,
                        resume_thread_id,
                        persist_session,
                        reasoning_effort,
                        prompt,
                        config.clone(),
                    ));
                }
            }
        }

        if shutting_down {
            if let Some(active) = active.as_ref() {
                active.cancel.store(true, Ordering::Relaxed);
            }
            if active.is_none() {
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

#[derive(Debug)]
enum McpIncoming {
    Json(serde_json::Value),
    StderrLine(String),
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum McpServerDisposition {
    Keep,
    Drop,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum McpTurnAbort {
    Stop,
    Reconnect,
}

struct McpServer {
    child: std::process::Child,
    stdin: std::process::ChildStdin,
    rx: Receiver<McpIncoming>,
    _stdout_handle: JoinHandle<()>,
    _stderr_handle: JoinHandle<()>,
    next_id: u64,
    endpoint: String,
}

impl McpServer {
    fn start() -> Result<Self, String> {
        let mut child = Command::new("codex")
            .arg("mcp-server")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|err| format!("Failed to spawn `codex mcp-server`: {err}"))?;

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| "Failed to open stdin for `codex mcp-server`".to_string())?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| "Failed to open stdout for `codex mcp-server`".to_string())?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| "Failed to open stderr for `codex mcp-server`".to_string())?;

        let (tx, rx) = mpsc::channel();

        let stdout_handle = thread::Builder::new()
            .name("nit-codex-mcp-stdout".into())
            .spawn({
                let tx = tx.clone();
                move || read_mcp_stdout(stdout, tx)
            })
            .map_err(|err| format!("Failed to spawn MCP stdout reader: {err}"))?;
        let stderr_handle = thread::Builder::new()
            .name("nit-codex-mcp-stderr".into())
            .spawn(move || read_mcp_stderr(stderr, tx))
            .map_err(|err| format!("Failed to spawn MCP stderr reader: {err}"))?;

        Ok(Self {
            child,
            stdin,
            rx,
            _stdout_handle: stdout_handle,
            _stderr_handle: stderr_handle,
            next_id: 0,
            endpoint: "stdio://codex-mcp-server".into(),
        })
    }

    fn initialize(&mut self) -> Result<(), String> {
        let init_params = serde_json::json!({
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": { "name": "nit", "version": env!("CARGO_PKG_VERSION") },
        });
        let init = self.rpc_request("initialize", init_params)?;
        if let Some(endpoint) = mcp_endpoint_from_initialize_response(&init) {
            self.endpoint = endpoint;
        }
        self.rpc_notify("initialized", serde_json::json!({}))?;

        let tools = self.rpc_request("tools/list", serde_json::json!({}))?;
        ensure_codex_tools_present(&tools)?;
        Ok(())
    }

    fn rpc_notify(&mut self, method: &str, params: serde_json::Value) -> Result<(), String> {
        let value = serde_json::json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        });
        self.send_json_line(&value)
    }

    fn rpc_request(
        &mut self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        self.next_id = self.next_id.wrapping_add(1).max(1);
        let id = self.next_id;
        let value = serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });
        self.send_json_line(&value)?;
        self.wait_response(id, Duration::from_secs(4))
    }

    fn send_json_line(&mut self, value: &serde_json::Value) -> Result<(), String> {
        let text =
            serde_json::to_string(value).map_err(|err| format!("JSON encode failed: {err}"))?;
        self.stdin
            .write_all(text.as_bytes())
            .and_then(|_| self.stdin.write_all(b"\n"))
            .map_err(|err| format!("Failed to write to MCP stdin: {err}"))
    }

    fn wait_response(&mut self, id: u64, timeout: Duration) -> Result<serde_json::Value, String> {
        let deadline = Instant::now() + timeout;
        let mut last_stderr: Option<String> = None;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                let suffix = last_stderr
                    .as_deref()
                    .map(|s| format!(" (last stderr: {s})"))
                    .unwrap_or_default();
                return Err(format!("MCP request timed out waiting for id={id}{suffix}"));
            }
            match self
                .rx
                .recv_timeout(remaining.min(Duration::from_millis(200)))
            {
                Ok(McpIncoming::Json(value)) => {
                    if value.get("id").and_then(|v| v.as_u64()) != Some(id) {
                        continue;
                    }
                    if let Some(err) = value.get("error") {
                        return Err(format!("MCP error: {}", err));
                    }
                    return Ok(value);
                }
                Ok(McpIncoming::StderrLine(line)) => {
                    last_stderr = Some(line);
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err("MCP server disconnected".into());
                }
            }
        }
    }

    fn stop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn read_mcp_stdout(stdout: impl Read, tx: Sender<McpIncoming>) {
    let mut reader = BufReader::new(stdout);
    let mut line = String::new();
    loop {
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) => break,
            Ok(_) => {
                let raw = line.trim();
                if raw.is_empty() {
                    continue;
                }
                match serde_json::from_str::<serde_json::Value>(raw) {
                    Ok(value) => {
                        let _ = tx.send(McpIncoming::Json(value));
                    }
                    Err(err) => {
                        let _ = tx.send(McpIncoming::StderrLine(format!(
                            "MCP stdout JSON parse error: {err} (raw={raw})"
                        )));
                    }
                }
            }
            Err(_) => break,
        }
    }
}

fn read_mcp_stderr(stderr: impl Read, tx: Sender<McpIncoming>) {
    let mut reader = BufReader::new(stderr);
    let mut line = String::new();
    loop {
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) => break,
            Ok(_) => {
                let raw = line.trim();
                if raw.is_empty() {
                    continue;
                }
                let _ = tx.send(McpIncoming::StderrLine(raw.to_string()));
            }
            Err(_) => break,
        }
    }
}

fn mcp_endpoint_from_initialize_response(resp: &serde_json::Value) -> Option<String> {
    let info = resp.get("result")?.get("serverInfo")?;
    let name = info.get("name").and_then(|v| v.as_str()).unwrap_or("mcp");
    let version = info.get("version").and_then(|v| v.as_str());
    Some(match version {
        Some(version) if !version.trim().is_empty() => {
            format!("stdio://{name} ({version})")
        }
        _ => format!("stdio://{name}"),
    })
}

fn ensure_codex_tools_present(tools_list_resp: &serde_json::Value) -> Result<(), String> {
    let tools = tools_list_resp
        .get("result")
        .and_then(|r| r.get("tools"))
        .and_then(|v| v.as_array())
        .ok_or_else(|| "MCP tools/list response missing result.tools".to_string())?;

    let mut has_codex = false;
    let mut has_reply = false;
    for tool in tools {
        let Some(name) = tool.get("name").and_then(|v| v.as_str()) else {
            continue;
        };
        match name {
            "codex" => has_codex = true,
            "codex-reply" => has_reply = true,
            _ => {}
        }
    }
    if has_codex && has_reply {
        return Ok(());
    }
    Err("MCP server missing required tools: expected `codex` and `codex-reply`".into())
}

fn runner_loop_mcp(
    cmd_rx: Receiver<CodexCommand>,
    event_tx: Sender<AgentBusEvent>,
    config: CodexRunnerConfig,
) {
    let mut queue: VecDeque<CodexCommand> = VecDeque::new();
    let mut shutting_down = false;
    let mut mcp_enabled = true;
    let mut next_connect_attempt_at = Instant::now();
    let mut server: Option<McpServer> = None;

    loop {
        if shutting_down {
            if let Some(server) = server.as_mut() {
                server.stop();
            }
            let _ = event_tx.send(AgentBusEvent::McpStatus {
                status: McpStatus {
                    state: McpConnectionState::Disconnected,
                    endpoint: server
                        .as_ref()
                        .map(|s| s.endpoint.clone())
                        .unwrap_or_else(|| "codex mcp-server".into()),
                    latency_ms: None,
                    last_error: None,
                },
            });
            break;
        }

        match cmd_rx.recv_timeout(Duration::from_millis(50)) {
            Ok(cmd) => match cmd {
                CodexCommand::RunTurn { .. } => queue.push_back(cmd),
                CodexCommand::McpStart | CodexCommand::McpStop | CodexCommand::McpReconnect => {
                    queue.push_front(cmd)
                }
                CodexCommand::Shutdown => {
                    shutting_down = true;
                    queue.clear();
                }
            },
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                shutting_down = true;
                queue.clear();
            }
        }

        let mut server_exited: Option<(String, String)> = None;
        if let Some(srv) = server.as_mut() {
            let exited = match srv.child.try_wait() {
                Ok(Some(status)) => Some(format!("codex mcp-server exited: {status}")),
                Ok(None) => None,
                Err(err) => Some(format!("codex mcp-server wait failed: {err}")),
            };
            if let Some(err) = exited {
                server_exited = Some((srv.endpoint.clone(), err));
            }
        }
        if let Some((endpoint, err)) = server_exited.take() {
            server = None;
            let _ = event_tx.send(AgentBusEvent::McpStatus {
                status: McpStatus {
                    state: McpConnectionState::Error,
                    endpoint,
                    latency_ms: None,
                    last_error: Some(err),
                },
            });
            next_connect_attempt_at = Instant::now() + Duration::from_secs(2);
        }

        // Process control commands before attempting to (re)connect.
        if let Some(cmd) = queue.pop_front() {
            match cmd {
                CodexCommand::McpStart => {
                    mcp_enabled = true;
                    next_connect_attempt_at = Instant::now();
                    continue;
                }
                CodexCommand::McpStop => {
                    mcp_enabled = false;
                    if let Some(server) = server.as_mut() {
                        server.stop();
                    }
                    server = None;
                    let _ = event_tx.send(AgentBusEvent::McpStatus {
                        status: McpStatus {
                            state: McpConnectionState::Disconnected,
                            endpoint: "codex mcp-server".into(),
                            latency_ms: None,
                            last_error: None,
                        },
                    });
                    continue;
                }
                CodexCommand::McpReconnect => {
                    mcp_enabled = true;
                    if let Some(server) = server.as_mut() {
                        server.stop();
                    }
                    server = None;
                    next_connect_attempt_at = Instant::now();
                    continue;
                }
                CodexCommand::Shutdown => {
                    shutting_down = true;
                    queue.clear();
                    continue;
                }
                CodexCommand::RunTurn { .. } => {
                    queue.push_front(cmd);
                }
            }
        }

        if mcp_enabled && server.is_none() && Instant::now() >= next_connect_attempt_at {
            let connect_started_at = Instant::now();
            let _ = event_tx.send(AgentBusEvent::McpStatus {
                status: McpStatus {
                    state: McpConnectionState::Connecting,
                    endpoint: "codex mcp-server".into(),
                    latency_ms: None,
                    last_error: None,
                },
            });
            match McpServer::start().and_then(|mut s| {
                s.initialize()?;
                Ok(s)
            }) {
                Ok(s) => {
                    let endpoint = s.endpoint.clone();
                    let latency_ms =
                        connect_started_at.elapsed().as_millis().min(u64::MAX as u128) as u64;
                    server = Some(s);
                    let _ = event_tx.send(AgentBusEvent::McpStatus {
                        status: McpStatus {
                            state: McpConnectionState::Connected,
                            endpoint,
                            latency_ms: Some(latency_ms),
                            last_error: None,
                        },
                    });
                }
                Err(err) => {
                    let _ = event_tx.send(AgentBusEvent::McpStatus {
                        status: McpStatus {
                            state: McpConnectionState::Error,
                            endpoint: "codex mcp-server".into(),
                            latency_ms: None,
                            last_error: Some(err),
                        },
                    });
                    next_connect_attempt_at = Instant::now() + Duration::from_secs(2);
                }
            }
        }

        if matches!(queue.front(), Some(CodexCommand::RunTurn { .. })) && server.is_some() {
            let cmd = queue.pop_front().expect("queue front");
            let CodexCommand::RunTurn {
                model,
                cwd,
                mission_id,
                resume_thread_id,
                persist_session: _persist_session,
                reasoning_effort,
                prompt,
            } = cmd
            else {
                continue;
            };
            let disposition = {
                let srv = server.as_mut().expect("mcp server");
                run_turn_mcp(
                    &event_tx,
                    &cmd_rx,
                    &mut queue,
                    srv,
                    model,
                    cwd,
                    mission_id,
                    resume_thread_id,
                    reasoning_effort,
                    prompt,
                    &config,
                    &mut shutting_down,
                )
            };
            if disposition == McpServerDisposition::Drop {
                server = None;
                next_connect_attempt_at = Instant::now() + Duration::from_secs(2);
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn run_turn_mcp(
    event_tx: &Sender<AgentBusEvent>,
    cmd_rx: &Receiver<CodexCommand>,
    queue: &mut VecDeque<CodexCommand>,
    server: &mut McpServer,
    model: String,
    cwd: PathBuf,
    mission_id: Option<String>,
    resume_thread_id: Option<String>,
    reasoning_effort: Option<String>,
    prompt: String,
    config: &CodexRunnerConfig,
    shutting_down: &mut bool,
) -> McpServerDisposition {
    let _ = event_tx.send(AgentBusEvent::TurnStarted {
        agent_id: model.clone(),
        mission_id: mission_id.clone(),
        resume_thread_id: resume_thread_id.clone(),
    });

    // The MCP server is already initialized by the runner loop; keep the UI connection status
    // accurate (the turn itself may be "busy" even while the transport is connected).
    let _ = event_tx.send(AgentBusEvent::McpStatus {
        status: McpStatus {
            state: McpConnectionState::Connected,
            endpoint: server.endpoint.clone(),
            latency_ms: None,
            last_error: None,
        },
    });

    let (tool_name, arguments) = if let Some(thread_id) = resume_thread_id.as_deref() {
        (
            "codex-reply",
            serde_json::json!({ "threadId": thread_id, "prompt": prompt }),
        )
    } else {
        let mut args = serde_json::Map::new();
        args.insert("prompt".into(), serde_json::Value::String(prompt));
        args.insert("model".into(), serde_json::Value::String(model.clone()));
        args.insert(
            "cwd".into(),
            serde_json::Value::String(cwd.to_string_lossy().to_string()),
        );
        if let Some(effort) = reasoning_effort.as_deref() {
            args.insert(
                "config".into(),
                serde_json::json!({ "model_reasoning_effort": effort }),
            );
        }
        if let Some(sandbox) = config.sandbox.as_deref().map(str::trim).filter(|s| !s.is_empty())
        {
            args.insert(
                "sandbox".into(),
                serde_json::Value::String(sandbox.to_string()),
            );
        }
        if let Some(policy) = config
            .approval_policy
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            args.insert(
                "approval-policy".into(),
                serde_json::Value::String(policy.to_string()),
            );
        }
        ("codex", serde_json::Value::Object(args))
    };

    let stage = format!("tools/call({tool_name})");
    let _ = event_tx.send(AgentBusEvent::TurnStage {
        agent_id: model.clone(),
        mission_id: mission_id.clone(),
        stage,
    });

    let started_at = Instant::now();
    server.next_id = server.next_id.wrapping_add(1).max(1);
    let id = server.next_id;
    let req = serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": "tools/call",
        "params": { "name": tool_name, "arguments": arguments },
    });
    if let Err(err) = server.send_json_line(&req) {
        let _ = event_tx.send(AgentBusEvent::McpStatus {
            status: McpStatus {
                state: McpConnectionState::Error,
                endpoint: server.endpoint.clone(),
                latency_ms: None,
                last_error: Some(err.clone()),
            },
        });
        let _ = event_tx.send(AgentBusEvent::TurnFailed {
            agent_id: model,
            mission_id,
            thread_id: resume_thread_id,
            token_count: None,
            message: err,
        });
        return McpServerDisposition::Drop;
    }

    let mut last_stage: Option<String> = None;
    let mut last_stage_sent_at = Instant::now();
    let mut last_heartbeat_at = Instant::now();
    let mut last_stderr: Option<String> = None;
    let mut last_stderr_sent: Option<String> = None;
    let mut last_stderr_sent_at = Instant::now();
    let mut last_token_count: Option<AgentTokenCount> = None;
    let timeout = mcp_turn_timeout();
    loop {
        let mut abort: Option<McpTurnAbort> = None;
        while let Ok(cmd) = cmd_rx.try_recv() {
            match cmd {
                CodexCommand::RunTurn { .. } if !*shutting_down => queue.push_back(cmd),
                CodexCommand::RunTurn { .. } => {}
                CodexCommand::McpStart if !*shutting_down => queue.push_front(cmd),
                CodexCommand::McpStop if !*shutting_down => {
                    queue.push_front(cmd);
                    abort = Some(McpTurnAbort::Stop);
                    break;
                }
                CodexCommand::McpReconnect if !*shutting_down => {
                    queue.push_front(cmd);
                    abort = Some(McpTurnAbort::Reconnect);
                    break;
                }
                CodexCommand::McpStart | CodexCommand::McpStop | CodexCommand::McpReconnect => {}
                CodexCommand::Shutdown => {
                    *shutting_down = true;
                    queue.clear();
                    break;
                }
            }
        }
        if *shutting_down {
            let _ = server.child.kill();
            return McpServerDisposition::Drop;
        }
        if let Some(abort) = abort {
            let (state, msg) = match abort {
                McpTurnAbort::Stop => (McpConnectionState::Disconnected, "Cancelled (MCP stop)"),
                McpTurnAbort::Reconnect => {
                    (McpConnectionState::Connecting, "Cancelled (MCP reconnect)")
                }
            };
            server.stop();
            let _ = event_tx.send(AgentBusEvent::McpStatus {
                status: McpStatus {
                    state,
                    endpoint: server.endpoint.clone(),
                    latency_ms: None,
                    last_error: None,
                },
            });
            let _ = event_tx.send(AgentBusEvent::TurnFailed {
                agent_id: model,
                mission_id,
                thread_id: resume_thread_id,
                token_count: last_token_count.clone(),
                message: msg.to_string(),
            });
            return McpServerDisposition::Drop;
        }
        if let Some(timeout) = timeout {
            if started_at.elapsed() >= timeout {
                let suffix = last_stderr
                    .as_deref()
                    .map(|s| format!(" (last stderr: {s})"))
                    .unwrap_or_default();
                let msg = format!(
                    "MCP tool call timed out after {}s{suffix}",
                    timeout.as_secs()
                );
                server.stop();
                let _ = event_tx.send(AgentBusEvent::McpStatus {
                    status: McpStatus {
                        state: McpConnectionState::Error,
                        endpoint: server.endpoint.clone(),
                        latency_ms: None,
                        last_error: Some(msg.clone()),
                    },
                });
                let _ = event_tx.send(AgentBusEvent::TurnFailed {
                    agent_id: model,
                    mission_id,
                    thread_id: resume_thread_id,
                    token_count: last_token_count.clone(),
                    message: msg,
                });
                return McpServerDisposition::Drop;
            }
        }
        if last_heartbeat_at.elapsed() >= Duration::from_secs(2) {
            let _ = event_tx.send(AgentBusEvent::TurnHeartbeat {
                agent_id: model.clone(),
                mission_id: mission_id.clone(),
            });
            last_heartbeat_at = Instant::now();
        }

        match server.rx.recv_timeout(Duration::from_millis(100)) {
            Ok(McpIncoming::Json(value)) => {
                if value.get("id").and_then(|v| v.as_u64()) != Some(id) {
                    if handle_codex_mcp_notification(
                        event_tx,
                        &model,
                        mission_id.as_deref(),
                        id,
                        &value,
                        &mut last_stage,
                        &mut last_stage_sent_at,
                        &mut last_token_count,
                    ) {
                        continue;
                    }
                    // Ignore notifications / other responses (we only allow one in-flight request).
                    continue;
                }
                if let Some(err) = value.get("error") {
                    let msg = format!("MCP tool call failed: {err}");
                    let _ = event_tx.send(AgentBusEvent::McpStatus {
                        status: McpStatus {
                            state: McpConnectionState::Error,
                            endpoint: server.endpoint.clone(),
                            latency_ms: None,
                            last_error: Some(msg.clone()),
                        },
                    });
                    let _ = event_tx.send(AgentBusEvent::TurnFailed {
                        agent_id: model,
                        mission_id,
                        thread_id: resume_thread_id,
                        token_count: last_token_count.clone(),
                        message: msg,
                    });
                    return McpServerDisposition::Keep;
                }
                let Some(result) = value.get("result") else {
                    let msg = "MCP tool call response missing result".to_string();
                    let _ = event_tx.send(AgentBusEvent::TurnFailed {
                        agent_id: model,
                        mission_id,
                        thread_id: resume_thread_id,
                        token_count: last_token_count.clone(),
                        message: msg,
                    });
                    return McpServerDisposition::Keep;
                };
                match extract_codex_mcp_output(result) {
                    Some((thread_id, content)) => {
                        let latency_ms = started_at.elapsed().as_millis().min(u64::MAX as u128) as u64;
                        let _ = event_tx.send(AgentBusEvent::McpStatus {
                            status: McpStatus {
                                state: McpConnectionState::Connected,
                                endpoint: server.endpoint.clone(),
                                latency_ms: Some(latency_ms),
                                last_error: None,
                            },
                        });
                        let _ = event_tx.send(AgentBusEvent::TurnCompleted {
                            agent_id: model,
                            mission_id,
                            thread_id: Some(thread_id),
                            token_count: last_token_count.clone(),
                            message: content.trim_end().to_string(),
                        });
                    }
                    None => {
                        let suffix = last_stderr
                            .as_deref()
                            .map(|s| format!(" (last stderr: {s})"))
                            .unwrap_or_default();
                        let msg = format!("Unexpected MCP tool result shape{suffix}");
                        let _ = event_tx.send(AgentBusEvent::TurnFailed {
                            agent_id: model,
                            mission_id,
                            thread_id: resume_thread_id,
                            token_count: last_token_count.clone(),
                            message: msg,
                        });
                    }
                }
                return McpServerDisposition::Keep;
            }
            Ok(McpIncoming::StderrLine(line)) => {
                let lowered = line.to_ascii_lowercase();
                let important = lowered.contains("error")
                    || lowered.contains("failed")
                    || lowered.contains("bad gateway")
                    || lowered.contains("unauthorized")
                    || lowered.contains("forbidden")
                    || lowered.contains("timeout")
                    || lowered.contains("dns")
                    || lowered.contains("connection");
                if important
                    && (last_stderr_sent.as_deref() != Some(line.as_str())
                        || last_stderr_sent_at.elapsed() >= Duration::from_secs(1))
                {
                    last_stderr_sent = Some(line.clone());
                    last_stderr_sent_at = Instant::now();
                    let _ = event_tx.send(AgentBusEvent::TurnLog {
                        agent_id: model.clone(),
                        message: line.clone(),
                    });
                }
                last_stderr = Some(line);
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                let msg = "MCP server disconnected".to_string();
                let _ = event_tx.send(AgentBusEvent::McpStatus {
                    status: McpStatus {
                        state: McpConnectionState::Error,
                        endpoint: server.endpoint.clone(),
                        latency_ms: None,
                        last_error: Some(msg.clone()),
                    },
                });
                let _ = event_tx.send(AgentBusEvent::TurnFailed {
                    agent_id: model,
                    mission_id,
                    thread_id: resume_thread_id,
                    token_count: last_token_count.clone(),
                    message: msg,
                });
                return McpServerDisposition::Drop;
            }
        }
    }
}

fn mcp_turn_timeout() -> Option<Duration> {
    const DEFAULT_TIMEOUT: Duration = Duration::from_secs(600);
    match std::env::var("NIT_MCP_TURN_TIMEOUT_SECS") {
        Ok(raw) => {
            let raw = raw.trim();
            if raw.is_empty() {
                return Some(DEFAULT_TIMEOUT);
            }
            let Ok(secs) = raw.parse::<u64>() else {
                return Some(DEFAULT_TIMEOUT);
            };
            if secs == 0 {
                None
            } else {
                Some(Duration::from_secs(secs))
            }
        }
        Err(_) => Some(DEFAULT_TIMEOUT),
    }
}

fn handle_codex_mcp_notification(
    event_tx: &Sender<AgentBusEvent>,
    agent_id: &str,
    mission_id: Option<&str>,
    request_id: u64,
    value: &serde_json::Value,
    last_stage: &mut Option<String>,
    last_stage_sent_at: &mut Instant,
    last_token_count: &mut Option<AgentTokenCount>,
) -> bool {
    let Some(method) = value.get("method").and_then(|v| v.as_str()) else {
        return false;
    };
    if method != "codex/event" {
        return false;
    }

    let Some(params) = value.get("params") else {
        return true;
    };
    if let Some(meta_id) = params
        .get("_meta")
        .and_then(|meta| meta.get("requestId"))
        .and_then(|v| v.as_u64())
    {
        if meta_id != request_id {
            return true;
        }
    }
    let Some(msg) = params.get("msg") else {
        return true;
    };
    let Some(kind) = msg.get("type").and_then(|v| v.as_str()) else {
        return true;
    };

    let stage = codex_mcp_stage_label(kind, msg);
    let is_interesting = kind.starts_with("thread_")
        || kind.starts_with("turn_")
        || kind.starts_with("task_")
        || kind.starts_with("item_")
        || kind.starts_with("tool_")
        || kind.ends_with("_error")
        || kind == "warning"
        || kind == "error"
        || kind == "token_count"
        || kind == "stream_error";
    if is_interesting
        && (last_stage.as_deref() != Some(stage.as_str())
            || last_stage_sent_at.elapsed() >= Duration::from_secs(1))
    {
        *last_stage = Some(stage.clone());
        *last_stage_sent_at = Instant::now();
        let _ = event_tx.send(AgentBusEvent::TurnStage {
            agent_id: agent_id.to_string(),
            mission_id: mission_id.map(str::to_string),
            stage,
        });
    }

    if kind == "token_count" {
        if let Some(token_count) = token_count_from_mcp_msg(msg) {
            *last_token_count = Some(token_count.clone());
            let _ = event_tx.send(AgentBusEvent::TokenCount {
                agent_id: agent_id.to_string(),
                mission_id: mission_id.map(str::to_string),
                token_count,
            });
        }
    }

    if kind == "warning" || kind == "error" || kind.ends_with("_error") || kind == "stream_error" {
        if let Some(message) = msg.get("message").and_then(|v| v.as_str()) {
            let _ = event_tx.send(AgentBusEvent::TurnLog {
                agent_id: agent_id.to_string(),
                message: message.to_string(),
            });
        }
    }

    true
}

fn token_count_from_mcp_msg(msg: &serde_json::Value) -> Option<AgentTokenCount> {
    let info = msg.get("info").unwrap_or(msg);
    let total_tokens = extract_total_tokens(info)?;
    if total_tokens == 0 || total_tokens > u32::MAX as u64 {
        return None;
    }
    let context_window = extract_context_window(info).unwrap_or(0);
    if context_window > u32::MAX as u64 {
        return None;
    }
    Some(AgentTokenCount {
        total_tokens: total_tokens as u32,
        context_window: context_window as u32,
    })
}

fn codex_mcp_stage_label(kind: &str, msg: &serde_json::Value) -> String {
    if matches!(kind, "item_started" | "item_completed") {
        if let Some(item_kind) = msg
            .get("item")
            .and_then(|item| item.get("type"))
            .and_then(|v| v.as_str())
        {
            return format!("{kind}({item_kind})");
        }
    }
    kind.to_string()
}

fn extract_codex_mcp_output(result: &serde_json::Value) -> Option<(String, String)> {
    // Common case: structured output matches outputSchema directly.
    if let (Some(thread_id), Some(content)) = (
        result.get("threadId").and_then(|v| v.as_str()),
        result.get("content").and_then(|v| v.as_str()),
    ) {
        return Some((thread_id.to_string(), content.to_string()));
    }

    // Some MCP servers wrap structured output.
    if let Some(structured) = result.get("structuredContent") {
        if let (Some(thread_id), Some(content)) = (
            structured.get("threadId").and_then(|v| v.as_str()),
            structured.get("content").and_then(|v| v.as_str()),
        ) {
            return Some((thread_id.to_string(), content.to_string()));
        }
    }

    // Fallback: try to recover text content from `content: [{type:text,text:...}, ...]`.
    let content_array = result.get("content").and_then(|v| v.as_array())?;
    let mut text = String::new();
    for item in content_array {
        if item.get("type").and_then(|v| v.as_str()) == Some("text") {
            if let Some(chunk) = item.get("text").and_then(|v| v.as_str()) {
                text.push_str(chunk);
            }
        }
    }
    if text.trim().is_empty() {
        return None;
    }
    // If we only have text, we still need a thread id.
    let thread_id = result
        .get("threadId")
        .or_else(|| result.get("thread_id"))
        .and_then(|v| v.as_str())?;
    Some((thread_id.to_string(), text))
}

struct ActiveTurn {
    cancel: Arc<AtomicBool>,
    done_rx: Receiver<()>,
    handle: JoinHandle<()>,
}

struct StdoutCapture {
    stdout: Vec<u8>,
    json_errors: Vec<String>,
}

fn spawn_turn_worker(
    event_tx: &Sender<AgentBusEvent>,
    seq: u64,
    model: String,
    cwd: PathBuf,
    mission_id: Option<String>,
    resume_thread_id: Option<String>,
    persist_session: bool,
    reasoning_effort: Option<String>,
    prompt: String,
    config: CodexRunnerConfig,
) -> ActiveTurn {
    let cancel = Arc::new(AtomicBool::new(false));
    let (done_tx, done_rx) = mpsc::channel();
    let event_tx = event_tx.clone();
    let cancel_worker = Arc::clone(&cancel);
    let handle = thread::Builder::new()
        .name(format!("nit-codex-turn-{seq}"))
        .spawn(move || {
            run_turn(
                &event_tx,
                seq,
                model,
                cwd,
                mission_id,
                resume_thread_id,
                persist_session,
                reasoning_effort,
                prompt,
                config,
                cancel_worker,
            );
            let _ = done_tx.send(());
        })
        .expect("spawn codex turn worker");
    ActiveTurn {
        cancel,
        done_rx,
        handle,
    }
}

fn run_turn(
    event_tx: &Sender<AgentBusEvent>,
    seq: u64,
    model: String,
    cwd: PathBuf,
    mission_id: Option<String>,
    resume_thread_id: Option<String>,
    persist_session: bool,
    reasoning_effort: Option<String>,
    prompt: String,
    config: CodexRunnerConfig,
    cancel: Arc<AtomicBool>,
) {
    let started_at = Instant::now();
    let out_file = std::env::temp_dir().join(format!("nit-codex-last-message-{seq}.txt"));

    let mut cmd = Command::new("codex");
    if let Some(policy) = config
        .approval_policy
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        cmd.arg("-a").arg(policy);
    }
    if let Some(sandbox) = config
        .sandbox
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        cmd.arg("-s").arg(sandbox);
    }
    if let Some(_thread_id) = resume_thread_id.as_deref() {
        cmd.arg("exec")
            .arg("resume")
            .arg("--json")
            .arg("-m")
            .arg(&model);
    } else {
        cmd.arg("exec").arg("--json").arg("--color").arg("never");
        if !persist_session {
            cmd.arg("--ephemeral");
        }
        cmd.arg("-m").arg(&model).arg("-C").arg(&cwd);
    }
    if let Some(effort) = reasoning_effort.as_deref() {
        // Override any global config (e.g. `xhigh`) that some models don't support.
        cmd.arg("-c")
            .arg(format!("model_reasoning_effort={:?}", effort.trim()));
    }
    cmd.arg("-o").arg(&out_file);
    if let Some(thread_id) = resume_thread_id.as_deref() {
        // Positional SESSION_ID comes after options for `codex exec resume`.
        cmd.arg(thread_id);
    }
    cmd
        // Read prompt from stdin so multi-line input works without shell escaping.
        .arg("-")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(err) => {
            let _ = event_tx.send(AgentBusEvent::McpStatus {
                status: McpStatus {
                    state: McpConnectionState::Error,
                    endpoint: codex_exec_endpoint_label(&model, resume_thread_id.as_deref()),
                    latency_ms: None,
                    last_error: Some(format!("Failed to spawn codex: {err}")),
                },
            });
            let _ = event_tx.send(AgentBusEvent::TurnFailed {
                agent_id: model,
                mission_id,
                thread_id: resume_thread_id.clone(),
                token_count: None,
                message: format!("Failed to spawn codex: {err}"),
            });
            return;
        }
    };

    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(prompt.as_bytes());
        let _ = stdin.write_all(b"\n");
    }

    let stdout_handle = child.stdout.take().map(|stdout| {
        let event_tx = event_tx.clone();
        let model = model.clone();
        let mission_id = mission_id.clone();
        thread::Builder::new()
            .name(format!("nit-codex-stdout-{seq}"))
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
                            buf.extend_from_slice(line.as_bytes());
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

                            if let Some(token_count) = token_count_from_value(&value) {
                                let _ = event_tx.send(AgentBusEvent::TokenCount {
                                    agent_id: model.clone(),
                                    mission_id: mission_id.clone(),
                                    token_count,
                                });
                            }

                            let payload = value.get("payload").unwrap_or(&value);
                            let kind = payload.get("type").and_then(|v| v.as_str());
                            if let Some(kind) = kind {
                                // Emit a compact "stage" update so the UI can show progress even
                                // when Codex doesn't stream intermediate messages.
                                let stage = if matches!(kind, "item.started" | "item.completed") {
                                    payload
                                        .get("item")
                                        .and_then(|item| item.get("type"))
                                        .and_then(|v| v.as_str())
                                        .map(|item_kind| format!("{kind}({item_kind})"))
                                        .unwrap_or_else(|| kind.to_string())
                                } else {
                                    kind.to_string()
                                };
                                let is_interesting = kind.starts_with("thread.")
                                    || kind.starts_with("turn.")
                                    || kind.starts_with("item.")
                                    || kind.starts_with("tool.")
                                    || kind == "token_count"
                                    || kind == "error";
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
                                let msg =
                                    payload.get("message").and_then(|v| v.as_str()).or_else(|| {
                                        payload
                                            .get("error")
                                            .and_then(|err| err.get("message"))
                                            .and_then(|v| v.as_str())
                                    });
                                if let Some(msg) = msg {
                                    json_errors.push(msg.to_string());
                                    let _ = event_tx.send(AgentBusEvent::TurnLog {
                                        agent_id: model.clone(),
                                        message: msg.to_string(),
                                    });
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
            .expect("spawn codex stdout reader")
    });
    let stderr_handle = child.stderr.take().map(|mut stderr| {
        thread::Builder::new()
            .name(format!("nit-codex-stderr-{seq}"))
            .spawn(move || {
                let mut buf = Vec::new();
                let _ = stderr.read_to_end(&mut buf);
                buf
            })
            .expect("spawn codex stderr reader")
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
                    thread_id: resume_thread_id.clone(),
                    token_count: None,
                    message: format!("Codex wait failed: {err}"),
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

    // Stderr can contain plain-text warnings even when `--json` is used.
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
        let message = if !json_errors.is_empty() {
            json_errors.join(" | ")
        } else if !stderr_text.trim().is_empty() {
            stderr_text.trim().to_string()
        } else {
            format!("Codex exited with {status}")
        };
        let latency_ms = started_at.elapsed().as_millis().min(u64::MAX as u128) as u64;
        let _ = event_tx.send(AgentBusEvent::McpStatus {
            status: McpStatus {
                state: McpConnectionState::Error,
                endpoint: format!("codex exec -m {model}"),
                latency_ms: Some(latency_ms),
                last_error: Some(message.clone()),
            },
        });
        let thread_id = extract_thread_id_from_jsonl(&stdout);
        let token_count = extract_token_count_from_jsonl(&stdout);
        let _ = event_tx.send(AgentBusEvent::TurnFailed {
            agent_id: model,
            mission_id,
            thread_id,
            token_count,
            message,
        });
        let _ = std::fs::remove_file(&out_file);
        return;
    }

    let message = std::fs::read_to_string(&out_file).unwrap_or_default();
    let _ = std::fs::remove_file(&out_file);
    let message = message.trim_end().to_string();
    if message.is_empty() {
        let thread_id = extract_thread_id_from_jsonl(&stdout);
        let token_count = extract_token_count_from_jsonl(&stdout);
        let _ = event_tx.send(AgentBusEvent::TurnFailed {
            agent_id: model,
            mission_id,
            thread_id,
            token_count,
            message: "Codex finished but produced an empty last message.".into(),
        });
        return;
    }

    let thread_id = extract_thread_id_from_jsonl(&stdout);
    let token_count = extract_token_count_from_jsonl(&stdout);
    let latency_ms = started_at.elapsed().as_millis().min(u64::MAX as u128) as u64;
    let _ = event_tx.send(AgentBusEvent::McpStatus {
        status: McpStatus {
            state: McpConnectionState::Connected,
            endpoint: format!("codex exec -m {model}"),
            latency_ms: Some(latency_ms),
            last_error: None,
        },
    });
    let _ = event_tx.send(AgentBusEvent::TurnCompleted {
        agent_id: model,
        mission_id,
        thread_id,
        token_count,
        message,
    });
}

fn codex_exec_endpoint_label(agent_id: &str, resume_thread_id: Option<&str>) -> String {
    if let Some(thread_id) = resume_thread_id {
        format!(
            "codex exec resume {} -m {agent_id}",
            shorten_thread_id(thread_id)
        )
    } else {
        format!("codex exec -m {agent_id}")
    }
}

fn shorten_thread_id(thread_id: &str) -> String {
    let id = thread_id.trim();
    const MAX_CHARS: usize = 8;
    let Some((idx, _)) = id.char_indices().nth(MAX_CHARS) else {
        return id.to_string();
    };
    format!("{}…", &id[..idx])
}

fn extract_thread_id_from_jsonl(stdout: &[u8]) -> Option<String> {
    // `codex exec --json` emits a "thread.started" event with a `thread_id` field.
    let text = String::from_utf8_lossy(stdout);
    for raw in text.lines() {
        let raw = raw.trim();
        if raw.is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<serde_json::Value>(raw) else {
            continue;
        };
        let Some(thread_id) = value.get("thread_id").and_then(|v| v.as_str()) else {
            continue;
        };
        if thread_id.trim().is_empty() {
            continue;
        }
        // Prefer the canonical thread lifecycle events, but accept any event containing thread_id.
        if let Some(kind) = value.get("type").and_then(|v| v.as_str()) {
            if kind.starts_with("thread.") {
                return Some(thread_id.to_string());
            }
        }
        return Some(thread_id.to_string());
    }
    None
}

fn extract_token_count_from_jsonl(stdout: &[u8]) -> Option<AgentTokenCount> {
    // Codex streams "token_count" events that include total token usage + context window.
    // We accept both exec-mode JSONL and session-style wrapped events.
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

        if let Some(token_count) = token_count_from_value(&value) {
            last = Some(token_count);
        }
    }
    last
}

fn token_count_from_value(value: &serde_json::Value) -> Option<AgentTokenCount> {
    let payload = value.get("payload").unwrap_or(value);
    let Some(kind) = payload.get("type").and_then(|v| v.as_str()) else {
        return None;
    };
    if kind == "token_count" {
        let Some(info) = payload.get("info") else {
            return None;
        };
        let context_window = extract_context_window(info)?;
        let total_tokens = extract_total_tokens(info)?;
        if context_window > u32::MAX as u64 || total_tokens > u32::MAX as u64 {
            return None;
        }
        return Some(AgentTokenCount {
            total_tokens: total_tokens as u32,
            context_window: context_window as u32,
        });
    }

    // Fallback: some Codex CLI versions only report per-turn token usage at `turn.completed`.
    // Those payloads often omit the context window size; the UI can stitch that in from the
    // models cache. We use `context_window=0` to mean "unknown".
    let Some(usage) = payload.get("usage") else {
        return None;
    };
    let input = usage
        .get("input_tokens")
        .or_else(|| usage.get("prompt_tokens"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let output = usage
        .get("output_tokens")
        .or_else(|| usage.get("completion_tokens"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let total = input.saturating_add(output);
    if total == 0 || total > u32::MAX as u64 {
        return None;
    }
    Some(AgentTokenCount {
        total_tokens: total as u32,
        context_window: 0,
    })
}

fn extract_context_window(info: &serde_json::Value) -> Option<u64> {
    info.get("model_context_window")
        .or_else(|| info.get("context_window"))
        .or_else(|| info.get("context_window_tokens"))
        .or_else(|| info.get("model_context_window_tokens"))
        .and_then(|v| v.as_u64())
        .filter(|v| *v > 0)
}

fn extract_total_tokens(info: &serde_json::Value) -> Option<u64> {
    // Prefer the *last* model-visible token usage over lifetime totals.
    //
    // Codex can auto-compact context when it nears the model context window. When that happens,
    // lifetime token usage (total_token_usage) keeps increasing, but the model-visible history
    // size can decrease. `last_token_usage` reflects that post-compaction size.
    info.get("last_token_usage")
        .and_then(|u| u.get("total_tokens"))
        .and_then(|v| v.as_u64())
        .or_else(|| {
            info.get("total_token_usage")
                .and_then(|u| u.get("total_tokens"))
                .and_then(|v| v.as_u64())
        })
        .or_else(|| info.get("total_tokens").and_then(|v| v.as_u64()))
        .or_else(|| info.get("used_tokens").and_then(|v| v.as_u64()))
}

#[cfg(test)]
mod tests {
    use super::handle_codex_mcp_notification;
    use super::extract_thread_id_from_jsonl;
    use super::extract_token_count_from_jsonl;
    use nit_core::AgentTokenCount;
    use nit_core::AgentBusEvent;
    use std::sync::mpsc;
    use std::time::Instant;

    #[test]
    fn extracts_thread_id_from_event_stream() {
        let jsonl =
            br#"{"type":"thread.started","thread_id":"019ca7c5-536f-7f81-82a7-7a38fa483cb2"}
{"type":"turn.started"}
{"type":"turn.completed"}"#;
        assert_eq!(
            extract_thread_id_from_jsonl(jsonl).as_deref(),
            Some("019ca7c5-536f-7f81-82a7-7a38fa483cb2")
        );
    }

    #[test]
    fn ignores_empty_thread_id() {
        let jsonl = br#"{"type":"thread.started","thread_id":"  "}
{"type":"turn.started"}"#;
        assert!(extract_thread_id_from_jsonl(jsonl).is_none());
    }

    #[test]
    fn extracts_last_token_count_from_wrapped_events() {
        let jsonl = br#"{"timestamp":"t","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"total_tokens":100},"model_context_window":1000}}}
{"timestamp":"t","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"total_tokens":250},"model_context_window":1000}}}"#;
        assert_eq!(
            extract_token_count_from_jsonl(jsonl),
            Some(AgentTokenCount {
                total_tokens: 250,
                context_window: 1000
            })
        );
    }

    #[test]
    fn token_count_prefers_last_token_usage_over_lifetime_totals() {
        let jsonl = br#"{"type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"total_tokens":999999},"last_token_usage":{"total_tokens":1234},"model_context_window":10000}}}"#;
        assert_eq!(
            extract_token_count_from_jsonl(jsonl),
            Some(AgentTokenCount {
                total_tokens: 1234,
                context_window: 10000
            })
        );
    }

    #[test]
    fn extracts_token_count_from_turn_completed_usage() {
        let jsonl = br#"{"type":"thread.started","thread_id":"thread-123"}
{"type":"turn.started"}
{"type":"turn.completed","usage":{"input_tokens":10916,"cached_input_tokens":9984,"output_tokens":72}}"#;
        assert_eq!(
            extract_token_count_from_jsonl(jsonl),
            Some(AgentTokenCount {
                total_tokens: 10988,
                context_window: 0
            })
        );
    }

    #[test]
    fn mcp_token_count_notifications_emit_agent_bus_token_count() {
        let (tx, rx) = mpsc::channel::<AgentBusEvent>();
        let value = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "codex/event",
            "params": {
                "_meta": { "requestId": 42 },
                "msg": {
                    "type": "token_count",
                    "info": {
                        "total_token_usage": { "total_tokens": 123 },
                        "model_context_window": 1000
                    }
                }
            }
        });

        let mut last_stage = None;
        let mut last_stage_sent_at = Instant::now();
        let mut last_token_count = None;
        assert!(handle_codex_mcp_notification(
            &tx,
            "gpt-test",
            None,
            42,
            &value,
            &mut last_stage,
            &mut last_stage_sent_at,
            &mut last_token_count,
        ));
        assert_eq!(
            last_token_count,
            Some(AgentTokenCount {
                total_tokens: 123,
                context_window: 1000
            })
        );

        let mut saw_token_count = false;
        while let Ok(event) = rx.try_recv() {
            if let AgentBusEvent::TokenCount { token_count, .. } = event {
                assert_eq!(
                    token_count,
                    AgentTokenCount {
                        total_tokens: 123,
                        context_window: 1000
                    }
                );
                saw_token_count = true;
                break;
            }
        }
        assert!(saw_token_count);
    }
}
