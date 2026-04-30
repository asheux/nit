use std::collections::{HashMap, VecDeque};
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
pub struct CodexRunnerConfig {
    pub sandbox: Option<String>,
    pub approval_policy: Option<String>,
    /// Maximum number of Codex turns to run concurrently.
    ///
    /// - Exec runtime: caps concurrent `codex exec` child processes.
    /// - MCP runtime: caps in-flight `tools/call` requests multiplexed over the persistent server.
    pub max_parallel_turns: usize,
    /// When set, Codex is launched with `-c mcp_servers.nit=...` so the model
    /// can invoke the nit substrate tools (emit_signal / assert_claim /
    /// assert_assumption) served by `nit-mcp-server`.  The string is the UDS
    /// socket path the back-channel listener bound in nit-tui.
    pub mcp_backchannel_socket: Option<String>,
}

impl Default for CodexRunnerConfig {
    fn default() -> Self {
        Self {
            sandbox: None,
            approval_policy: Some("never".into()),
            max_parallel_turns: usize::MAX,
            mcp_backchannel_socket: None,
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum CodexRuntimeMode {
    /// Spawn a fresh `codex exec` child process per turn.
    Exec,
    /// Keep a persistent `codex mcp-server` process and multiplex turns as `tools/call` requests.
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
        /// Restrict the turn to a read-only sandbox (no workspace writes, no
        /// shell). Used for shadow advisory agents.
        read_only: bool,
    },
    McpStart,
    McpStop,
    McpReconnect,
    Shutdown,
    /// Cancel any in-flight turn for `agent_id` (kills the subprocess via
    /// the per-turn `cancel` AtomicBool) and drop matching queued turns
    /// from this runner's pending queue. Idempotent — no-op if the agent
    /// has no in-flight or queued work.
    CancelTurn {
        agent_id: String,
    },
    /// Cancel every in-flight turn and drop the entire queue. Used by the
    /// global `/abort all` path. Cheaper than enumerating agents when the
    /// caller wants to clear the runner wholesale.
    CancelAll,
}

pub struct CodexRunner {
    cmd_tx: Sender<CodexCommand>,
    pub events: Receiver<AgentBusEvent>,
    shutdown: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl CodexRunner {
    pub fn spawn(
        mode: CodexRuntimeMode,
        config: CodexRunnerConfig,
        mcp_backchannel_socket: Option<String>,
    ) -> Self {
        let (cmd_tx, cmd_rx) = mpsc::channel();
        let (event_tx, event_rx) = mpsc::channel();
        let shutdown = Arc::new(AtomicBool::new(false));
        let shutdown_worker = Arc::clone(&shutdown);
        let config = CodexRunnerConfig {
            mcp_backchannel_socket,
            ..config
        };
        let handle = thread::Builder::new()
            .name("nit-codex".into())
            .spawn(move || runner_loop(mode, config, shutdown_worker, cmd_rx, event_tx))
            .expect("spawn codex runner");
        Self {
            cmd_tx,
            events: event_rx,
            shutdown,
            handle: Some(handle),
        }
    }

    /// Send a command to the runner. Returns `true` if the command was accepted,
    /// `false` if the runner's channel is disconnected (runner shut down or crashed).
    pub fn send(&self, command: CodexCommand) -> bool {
        self.cmd_tx.send(command).is_ok()
    }

    pub fn shutdown(&mut self) {
        self.shutdown.store(true, Ordering::Relaxed);
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

impl Drop for CodexRunner {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn runner_loop(
    mode: CodexRuntimeMode,
    config: CodexRunnerConfig,
    shutdown: Arc<AtomicBool>,
    cmd_rx: Receiver<CodexCommand>,
    event_tx: Sender<AgentBusEvent>,
) {
    match mode {
        CodexRuntimeMode::Exec => runner_loop_exec(cmd_rx, event_tx, config),
        CodexRuntimeMode::Mcp => runner_loop_mcp(cmd_rx, event_tx, config, shutdown),
    }
}

fn runner_loop_exec(
    cmd_rx: Receiver<CodexCommand>,
    event_tx: Sender<AgentBusEvent>,
    config: CodexRunnerConfig,
) {
    let mut seq = 0u64;
    let mut queue: VecDeque<CodexCommand> = VecDeque::new();
    let mut active: Vec<ActiveTurn> = Vec::new();
    let mut shutting_down = false;
    let mut shutdown_deadline: Option<Instant> = None;
    let max_parallel = config.max_parallel_turns.max(1);

    loop {
        // Keep this control loop responsive so `Shutdown` can cancel an in-flight Codex process.
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
                CodexCommand::RunTurn { .. } if !shutting_down => queue.push_back(cmd),
                CodexCommand::RunTurn { .. } => {}
                CodexCommand::McpStart | CodexCommand::McpStop | CodexCommand::McpReconnect => {}
                CodexCommand::Shutdown => {
                    shutting_down = true;
                    shutdown_deadline = Some(Instant::now() + Duration::from_millis(400));
                    queue.clear();
                    for turn in active.iter() {
                        turn.cancel.store(true, Ordering::Relaxed);
                    }
                }
                CodexCommand::CancelTurn { agent_id } => {
                    // Kill any in-flight turn for this agent: the worker
                    // checks `cancel` between try_wait polls and calls
                    // child.kill() within ~50ms.
                    for turn in active.iter() {
                        if turn.agent_id == agent_id {
                            turn.cancel.store(true, Ordering::Relaxed);
                        }
                    }
                    // Drop pending RunTurn commands for the same agent so
                    // a queued turn doesn't fire after the user aborted.
                    queue.retain(|cmd| match cmd {
                        CodexCommand::RunTurn { model, .. } => model.as_str() != agent_id,
                        _ => true,
                    });
                }
                CodexCommand::CancelAll => {
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
                    CodexCommand::RunTurn { model, .. } => !active
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
                let CodexCommand::RunTurn {
                    model,
                    cwd,
                    mission_id,
                    resume_thread_id,
                    persist_session,
                    reasoning_effort,
                    prompt,
                    read_only,
                } = cmd
                else {
                    continue;
                };
                let _ = event_tx.send(AgentBusEvent::McpStatus {
                    status: McpStatus {
                        state: McpConnectionState::Connecting,
                        endpoint: codex_exec_endpoint_label(&model, resume_thread_id.as_deref()),
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
                active.push(spawn_turn_worker(
                    &event_tx,
                    seq,
                    model,
                    cwd,
                    mission_id,
                    resume_thread_id,
                    persist_session,
                    reasoning_effort,
                    prompt,
                    read_only,
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

#[derive(Debug)]
enum McpIncoming {
    Json(serde_json::Value),
    StderrLine(String),
}

struct McpServer {
    child: std::process::Child,
    stdin: std::process::ChildStdin,
    rx: Receiver<McpIncoming>,
    _stdout_handle: JoinHandle<()>,
    _stderr_handle: JoinHandle<()>,
    next_id: u64,
    endpoint: String,
    shutdown: Arc<AtomicBool>,
}

impl McpServer {
    fn start(shutdown: Arc<AtomicBool>, config: &CodexRunnerConfig) -> Result<Self, String> {
        let mut cmd = Command::new("codex");
        // nit-mcp override: if the back-channel is live, tell `codex mcp-server`
        // about the `nit` tool server so the model can discover our tools.
        // Re-uses the same helper as per-turn `codex exec` for consistency.
        let mut mcp_args: Vec<String> = Vec::new();
        push_nit_mcp_config_args(&mut mcp_args, config, "codex-mcp-session");
        for arg in &mcp_args {
            cmd.arg(arg);
        }
        let mut child = cmd
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
            shutdown,
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
            if self.shutdown.load(Ordering::Relaxed) {
                return Err("MCP request cancelled (shutdown)".into());
            }
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
                        return Err(format!("MCP error: {err}"));
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

impl Drop for McpServer {
    fn drop(&mut self) {
        self.stop();
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

struct InFlightMcpTurn {
    agent_id: String,
    mission_id: Option<String>,
    resume_thread_id: Option<String>,
    cwd: PathBuf,
    started_at: Instant,
    last_activity_at: Instant,
    last_heartbeat_sent_at: Instant,
    last_stage: Option<String>,
    last_stage_sent_at: Instant,
    last_token_count: Option<AgentTokenCount>,
}

fn runner_loop_mcp(
    cmd_rx: Receiver<CodexCommand>,
    event_tx: Sender<AgentBusEvent>,
    config: CodexRunnerConfig,
    shutdown: Arc<AtomicBool>,
) {
    let mut queue: VecDeque<CodexCommand> = VecDeque::new();
    let mut shutting_down = false;
    let mut mcp_enabled = true;
    let mut next_connect_attempt_at = Instant::now();
    let mut server: Option<McpServer> = None;
    let mut in_flight: HashMap<u64, InFlightMcpTurn> = HashMap::new();
    let mut in_flight_by_agent: HashMap<String, u64> = HashMap::new();
    let max_parallel = config.max_parallel_turns.max(1);
    let timeout = mcp_turn_timeout();
    let idle_timeout = mcp_turn_idle_timeout();

    // Global stderr throttling; stderr is not request-id tagged so we avoid spamming all lanes.
    let mut last_stderr_sent: Option<String> = None;
    let mut last_stderr_sent_at = Instant::now();

    loop {
        if shutdown.load(Ordering::Relaxed) {
            shutting_down = true;
            queue.clear();
        }
        if shutting_down {
            if let Some(server) = server.as_mut() {
                server.stop();
            }
            in_flight.clear();
            in_flight_by_agent.clear();
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

        match cmd_rx.recv_timeout(Duration::from_millis(30)) {
            Ok(cmd) => match cmd {
                CodexCommand::RunTurn { .. } => queue.push_back(cmd),
                CodexCommand::McpStart | CodexCommand::McpStop | CodexCommand::McpReconnect => {
                    queue.push_front(cmd)
                }
                CodexCommand::Shutdown => {
                    shutting_down = true;
                    queue.clear();
                }
                CodexCommand::CancelTurn { agent_id } => {
                    // MCP turns multiplex through the shared mcp-server
                    // process; cancellation is per-RPC by request id.
                    // Find every in-flight turn for this agent, fail it,
                    // and free its slot.
                    let ids_to_cancel: Vec<u64> = in_flight
                        .iter()
                        .filter(|(_, t)| t.agent_id == agent_id)
                        .map(|(id, _)| *id)
                        .collect();
                    for id in ids_to_cancel {
                        if let Some(turn) = in_flight.remove(&id) {
                            in_flight_by_agent.remove(&turn.agent_id);
                            let _ = event_tx.send(AgentBusEvent::TurnFailed {
                                agent_id: turn.agent_id,
                                mission_id: turn.mission_id,
                                thread_id: turn.resume_thread_id,
                                token_count: turn.last_token_count,
                                message: nit_core::OPERATOR_CANCEL_TURN_MESSAGE.into(),
                            });
                        }
                    }
                    queue.retain(|cmd| match cmd {
                        CodexCommand::RunTurn { model, .. } => model.as_str() != agent_id,
                        _ => true,
                    });
                }
                CodexCommand::CancelAll => {
                    queue.clear();
                    for (_id, turn) in in_flight.drain() {
                        let _ = event_tx.send(AgentBusEvent::TurnFailed {
                            agent_id: turn.agent_id,
                            mission_id: turn.mission_id,
                            thread_id: turn.resume_thread_id,
                            token_count: turn.last_token_count,
                            message: nit_core::OPERATOR_CANCEL_TURN_MESSAGE.into(),
                        });
                    }
                    in_flight_by_agent.clear();
                }
            },
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                shutting_down = true;
                queue.clear();
            }
        }

        // Detect unexpected server exit.
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
            for (_id, turn) in in_flight.drain() {
                let _ = event_tx.send(AgentBusEvent::TurnFailed {
                    agent_id: turn.agent_id,
                    mission_id: turn.mission_id,
                    thread_id: turn.resume_thread_id,
                    token_count: turn.last_token_count,
                    message: "MCP server exited".into(),
                });
            }
            in_flight_by_agent.clear();
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

        // Drain front-of-queue control commands (start/stop/reconnect/shutdown).
        loop {
            let Some(cmd) = queue.pop_front() else {
                break;
            };
            match cmd {
                CodexCommand::McpStart => {
                    mcp_enabled = true;
                    next_connect_attempt_at = Instant::now();
                }
                CodexCommand::McpStop => {
                    mcp_enabled = false;
                    if let Some(server) = server.as_mut() {
                        server.stop();
                    }
                    server = None;
                    // Cancel in-flight turns and any turns already queued in this runner.
                    for (_id, turn) in in_flight.drain() {
                        let _ = event_tx.send(AgentBusEvent::TurnFailed {
                            agent_id: turn.agent_id,
                            mission_id: turn.mission_id,
                            thread_id: None,
                            token_count: turn.last_token_count,
                            message: "Cancelled (MCP stop)".into(),
                        });
                    }
                    in_flight_by_agent.clear();
                    while let Some(cmd) = queue.pop_front() {
                        if let CodexCommand::RunTurn {
                            model, mission_id, ..
                        } = cmd
                        {
                            let _ = event_tx.send(AgentBusEvent::TurnFailed {
                                agent_id: model,
                                mission_id,
                                thread_id: None,
                                token_count: None,
                                message: "Cancelled (MCP stop)".into(),
                            });
                        }
                    }
                    let _ = event_tx.send(AgentBusEvent::McpStatus {
                        status: McpStatus {
                            state: McpConnectionState::Disconnected,
                            endpoint: "codex mcp-server".into(),
                            latency_ms: None,
                            last_error: None,
                        },
                    });
                }
                CodexCommand::McpReconnect => {
                    mcp_enabled = true;
                    if let Some(server) = server.as_mut() {
                        server.stop();
                    }
                    server = None;
                    // Match previous behavior: reconnect cancels in-flight turns but keeps queued turns.
                    for (_id, turn) in in_flight.drain() {
                        let _ = event_tx.send(AgentBusEvent::TurnFailed {
                            agent_id: turn.agent_id,
                            mission_id: turn.mission_id,
                            thread_id: None,
                            token_count: turn.last_token_count,
                            message: "Cancelled (MCP reconnect)".into(),
                        });
                    }
                    in_flight_by_agent.clear();
                    next_connect_attempt_at = Instant::now();
                    let _ = event_tx.send(AgentBusEvent::McpStatus {
                        status: McpStatus {
                            state: McpConnectionState::Connecting,
                            endpoint: "codex mcp-server".into(),
                            latency_ms: None,
                            last_error: None,
                        },
                    });
                }
                CodexCommand::Shutdown => {
                    shutting_down = true;
                    queue.clear();
                }
                other => {
                    queue.push_front(other);
                    break;
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
            match McpServer::start(Arc::clone(&shutdown), &config).and_then(|mut s| {
                s.initialize()?;
                Ok(s)
            }) {
                Ok(s) => {
                    let endpoint = s.endpoint.clone();
                    let latency_ms = connect_started_at
                        .elapsed()
                        .as_millis()
                        .min(u64::MAX as u128) as u64;
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

        // Per-turn timeouts: if any turn exceeds the timeout, restart the server and fail all turns.
        if let Some(timeout) = timeout {
            let now = Instant::now();
            let timed_out = in_flight
                .values()
                .any(|turn| now.duration_since(turn.started_at) >= timeout);
            if timed_out {
                let msg = format!("MCP tool call timed out after {}s", timeout.as_secs());
                if let Some(mut srv) = server.take() {
                    srv.stop();
                }
                for (_id, turn) in in_flight.drain() {
                    let _ = event_tx.send(AgentBusEvent::TurnFailed {
                        agent_id: turn.agent_id,
                        mission_id: turn.mission_id,
                        thread_id: turn.resume_thread_id,
                        token_count: turn.last_token_count,
                        message: msg.clone(),
                    });
                }
                in_flight_by_agent.clear();
                let _ = event_tx.send(AgentBusEvent::McpStatus {
                    status: McpStatus {
                        state: McpConnectionState::Error,
                        endpoint: "codex mcp-server".into(),
                        latency_ms: None,
                        last_error: Some(msg),
                    },
                });
                next_connect_attempt_at = Instant::now() + Duration::from_secs(2);
                continue;
            }
        }

        // Per-turn idle timeouts: a single tool call that stops producing
        // events is a model-side stall, not a server-side failure. Fail
        // only the stalled turn(s) and keep the multiplexed server alive
        // for the other in-flight turns.
        if let Some(idle_timeout) = idle_timeout {
            let now = Instant::now();
            let stalled_ids: Vec<u64> = in_flight
                .iter()
                .filter(|(_, turn)| now.duration_since(turn.last_activity_at) >= idle_timeout)
                .map(|(id, _)| *id)
                .collect();
            if !stalled_ids.is_empty() {
                let msg = format!(
                    "MCP tool call stalled after {}s without events",
                    idle_timeout.as_secs()
                );
                for id in stalled_ids {
                    if let Some(turn) = in_flight.remove(&id) {
                        in_flight_by_agent.remove(&turn.agent_id);
                        let _ = event_tx.send(AgentBusEvent::TurnFailed {
                            agent_id: turn.agent_id,
                            mission_id: turn.mission_id,
                            thread_id: turn.resume_thread_id,
                            token_count: turn.last_token_count,
                            message: msg.clone(),
                        });
                    }
                }
            }
        }

        // Heartbeats for all in-flight turns.
        let now = Instant::now();
        for turn in in_flight.values_mut() {
            if now.duration_since(turn.last_heartbeat_sent_at) >= Duration::from_secs(2) {
                let _ = event_tx.send(AgentBusEvent::TurnHeartbeat {
                    agent_id: turn.agent_id.clone(),
                    mission_id: turn.mission_id.clone(),
                });
                turn.last_heartbeat_sent_at = now;
            }
        }

        // Consume any pending MCP messages (notifications + responses).
        let mut drop_server_due_to_disconnect = false;
        if let Some(srv) = server.as_mut() {
            for _ in 0..50 {
                match srv.rx.try_recv() {
                    Ok(McpIncoming::Json(value)) => {
                        if let Some(id) = value.get("id").and_then(|v| v.as_u64()) {
                            let Some(turn) = in_flight.remove(&id) else {
                                continue;
                            };
                            in_flight_by_agent.remove(&turn.agent_id);

                            if let Some(err) = value.get("error") {
                                let msg = format!("MCP tool call failed: {err}");
                                let _ = event_tx.send(AgentBusEvent::McpStatus {
                                    status: McpStatus {
                                        state: McpConnectionState::Error,
                                        endpoint: srv.endpoint.clone(),
                                        latency_ms: None,
                                        last_error: Some(msg.clone()),
                                    },
                                });
                                let _ = event_tx.send(AgentBusEvent::TurnFailed {
                                    agent_id: turn.agent_id,
                                    mission_id: turn.mission_id,
                                    thread_id: turn.resume_thread_id,
                                    token_count: turn.last_token_count,
                                    message: msg,
                                });
                                continue;
                            }

                            let Some(result) = value.get("result") else {
                                let _ = event_tx.send(AgentBusEvent::TurnFailed {
                                    agent_id: turn.agent_id,
                                    mission_id: turn.mission_id,
                                    thread_id: turn.resume_thread_id,
                                    token_count: turn.last_token_count,
                                    message: "MCP tool call response missing result".into(),
                                });
                                continue;
                            };

                            match extract_codex_mcp_output(result) {
                                Some((thread_id, content)) => {
                                    let latency_ms =
                                        turn.started_at.elapsed().as_millis().min(u64::MAX as u128)
                                            as u64;
                                    let _ = event_tx.send(AgentBusEvent::McpStatus {
                                        status: McpStatus {
                                            state: McpConnectionState::Connected,
                                            endpoint: srv.endpoint.clone(),
                                            latency_ms: Some(latency_ms),
                                            last_error: None,
                                        },
                                    });
                                    let _ = event_tx.send(AgentBusEvent::TurnCompleted {
                                        agent_id: turn.agent_id,
                                        mission_id: turn.mission_id,
                                        thread_id: Some(thread_id),
                                        token_count: turn.last_token_count,
                                        message: content.trim_end().to_string(),
                                    });
                                }
                                None => {
                                    let _ = event_tx.send(AgentBusEvent::TurnFailed {
                                        agent_id: turn.agent_id,
                                        mission_id: turn.mission_id,
                                        thread_id: turn.resume_thread_id,
                                        token_count: turn.last_token_count,
                                        message: "Unexpected MCP tool result shape".into(),
                                    });
                                }
                            }
                            continue;
                        }

                        // Notifications (codex/event) update stage/token counts.
                        let request_id = value
                            .get("params")
                            .and_then(|p| p.get("_meta"))
                            .and_then(|m| m.get("requestId"))
                            .and_then(|v| v.as_u64());
                        if let Some(request_id) = request_id {
                            if let Some(turn) = in_flight.get_mut(&request_id) {
                                turn.last_activity_at = Instant::now();
                                let _ = handle_codex_mcp_notification(
                                    &event_tx,
                                    &turn.agent_id,
                                    turn.mission_id.as_deref(),
                                    request_id,
                                    &turn.cwd,
                                    &value,
                                    &mut turn.last_stage,
                                    &mut turn.last_stage_sent_at,
                                    &mut turn.last_token_count,
                                );
                            }
                        } else if in_flight.len() == 1 {
                            if let Some((&only_id, turn)) = in_flight.iter_mut().next() {
                                turn.last_activity_at = Instant::now();
                                let _ = handle_codex_mcp_notification(
                                    &event_tx,
                                    &turn.agent_id,
                                    turn.mission_id.as_deref(),
                                    only_id,
                                    &turn.cwd,
                                    &value,
                                    &mut turn.last_stage,
                                    &mut turn.last_stage_sent_at,
                                    &mut turn.last_token_count,
                                );
                            }
                        }
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
                            for turn in in_flight.values() {
                                let _ = event_tx.send(AgentBusEvent::TurnLog {
                                    agent_id: turn.agent_id.clone(),
                                    message: line.clone(),
                                });
                            }
                        }
                    }
                    Err(mpsc::TryRecvError::Empty) => break,
                    Err(mpsc::TryRecvError::Disconnected) => {
                        let msg = "MCP server disconnected".to_string();
                        let _ = event_tx.send(AgentBusEvent::McpStatus {
                            status: McpStatus {
                                state: McpConnectionState::Error,
                                endpoint: srv.endpoint.clone(),
                                latency_ms: None,
                                last_error: Some(msg.clone()),
                            },
                        });
                        for (_id, turn) in in_flight.drain() {
                            let _ = event_tx.send(AgentBusEvent::TurnFailed {
                                agent_id: turn.agent_id,
                                mission_id: turn.mission_id,
                                thread_id: turn.resume_thread_id,
                                token_count: turn.last_token_count,
                                message: msg.clone(),
                            });
                        }
                        in_flight_by_agent.clear();
                        drop_server_due_to_disconnect = true;
                        break;
                    }
                }
            }
        }
        if drop_server_due_to_disconnect {
            server = None;
            next_connect_attempt_at = Instant::now() + Duration::from_secs(2);
        }

        // Dispatch queued turns while under the parallel cap (one in-flight turn per agent id).
        if mcp_enabled && server.is_some() && in_flight.len() < max_parallel {
            let mut drop_server = false;
            while in_flight.len() < max_parallel {
                let idx = queue.iter().position(|cmd| match cmd {
                    CodexCommand::RunTurn { model, .. } => !in_flight_by_agent.contains_key(model),
                    _ => false,
                });
                let Some(idx) = idx else {
                    break;
                };
                let Some(cmd) = queue.remove(idx) else {
                    break;
                };
                let CodexCommand::RunTurn {
                    model,
                    cwd,
                    mission_id,
                    resume_thread_id,
                    persist_session: _persist_session,
                    reasoning_effort,
                    prompt,
                    read_only,
                } = cmd
                else {
                    continue;
                };

                let Some(srv) = server.as_mut() else {
                    break;
                };

                let _ = event_tx.send(AgentBusEvent::TurnStarted {
                    agent_id: model.clone(),
                    mission_id: mission_id.clone(),
                    resume_thread_id: resume_thread_id.clone(),
                });
                let _ = event_tx.send(AgentBusEvent::McpStatus {
                    status: McpStatus {
                        state: McpConnectionState::Connected,
                        endpoint: srv.endpoint.clone(),
                        latency_ms: None,
                        last_error: None,
                    },
                });

                let (tool_name, arguments) = build_codex_mcp_tool_call(
                    model.as_str(),
                    prompt.as_str(),
                    cwd.as_path(),
                    reasoning_effort.as_deref(),
                    &config,
                    resume_thread_id.as_deref(),
                    read_only,
                );

                let stage = format!("tools/call({tool_name})");
                let _ = event_tx.send(AgentBusEvent::TurnStage {
                    agent_id: model.clone(),
                    mission_id: mission_id.clone(),
                    stage,
                });

                srv.next_id = srv.next_id.wrapping_add(1).max(1);
                let id = srv.next_id;
                let req = serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "method": "tools/call",
                    "params": { "name": tool_name, "arguments": arguments },
                });
                if let Err(err) = srv.send_json_line(&req) {
                    let _ = event_tx.send(AgentBusEvent::McpStatus {
                        status: McpStatus {
                            state: McpConnectionState::Error,
                            endpoint: srv.endpoint.clone(),
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
                    drop_server = true;
                    break;
                }

                let now = Instant::now();
                in_flight.insert(
                    id,
                    InFlightMcpTurn {
                        agent_id: model.clone(),
                        mission_id,
                        resume_thread_id,
                        cwd: cwd.clone(),
                        started_at: now,
                        last_activity_at: now,
                        last_heartbeat_sent_at: now,
                        last_stage: None,
                        last_stage_sent_at: now,
                        last_token_count: None,
                    },
                );
                in_flight_by_agent.insert(model, id);
            }

            if drop_server {
                if let Some(server) = server.as_mut() {
                    server.stop();
                }
                server = None;
                next_connect_attempt_at = Instant::now() + Duration::from_secs(2);
            }
        }
    }
}

fn mcp_turn_timeout() -> Option<Duration> {
    match std::env::var("NIT_MCP_TURN_TIMEOUT_SECS") {
        Ok(raw) => {
            let raw = raw.trim();
            if raw.is_empty() {
                return None;
            }
            let secs = raw.parse::<u64>().ok()?;
            if secs == 0 {
                None
            } else {
                Some(Duration::from_secs(secs))
            }
        }
        Err(_) => None,
    }
}

fn mcp_turn_idle_timeout() -> Option<Duration> {
    const DEFAULT_IDLE_TIMEOUT_SECS: u64 = 10 * 60;
    match std::env::var("NIT_MCP_TURN_IDLE_TIMEOUT_SECS") {
        Ok(raw) => {
            let raw = raw.trim();
            if raw.is_empty() {
                return Some(Duration::from_secs(DEFAULT_IDLE_TIMEOUT_SECS));
            }
            match raw.parse::<u64>() {
                Ok(0) => None,
                Ok(secs) => Some(Duration::from_secs(secs)),
                Err(_) => Some(Duration::from_secs(DEFAULT_IDLE_TIMEOUT_SECS)),
            }
        }
        Err(_) => None,
    }
}

#[allow(clippy::too_many_arguments)]
fn handle_codex_mcp_notification(
    event_tx: &Sender<AgentBusEvent>,
    agent_id: &str,
    mission_id: Option<&str>,
    request_id: u64,
    cwd: &std::path::Path,
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

    // Detect file writes from tool_use events for per-agent genome attribution.
    // Agents write files via tools (edit, write, str_replace_editor, bash).
    // Extract file paths from item events so the genome system knows which
    // agent modified which file — no filesystem-level guessing.
    if kind == "item_completed" || kind == "item.completed" {
        if let Some(item) = msg.get("item").or_else(|| msg.get("info")) {
            extract_file_write_paths(item, cwd)
                .into_iter()
                .for_each(|path| {
                    let _ = event_tx.send(AgentBusEvent::FileWrite {
                        agent_id: agent_id.to_string(),
                        mission_id: mission_id.map(str::to_string),
                        path,
                    });
                });
        }
    }

    true
}

// Scan a tool_use/item event for filesystem write targets. Handles Codex,
// Claude, and generic agent formats by checking several nested locations and
// common path field aliases, deduplicating overlapping matches.
fn extract_file_write_paths(
    item: &serde_json::Value,
    cwd: &std::path::Path,
) -> Vec<std::path::PathBuf> {
    let mut paths = Vec::new();
    let mut seen = std::collections::HashSet::new();

    let path_keys = [
        "file_path",
        "path",
        "file",
        "filename",
        "file_name",
        "target_file",
        "old_str_file",
        "new_file_path",
    ];

    // Recursively search for path-like string values in the JSON tree.
    fn extract_paths_recursive(
        value: &serde_json::Value,
        keys: &[&str],
        cwd: &std::path::Path,
        paths: &mut Vec<std::path::PathBuf>,
        seen: &mut std::collections::HashSet<std::path::PathBuf>,
        depth: usize,
    ) {
        if depth > 5 {
            return; // Avoid unbounded recursion.
        }
        match value {
            serde_json::Value::Object(map) => {
                for (k, v) in map {
                    if keys.contains(&k.as_str()) {
                        if let Some(p) = v.as_str() {
                            if !p.is_empty() && !p.contains('\n') {
                                let path = if std::path::Path::new(p).is_absolute() {
                                    std::path::PathBuf::from(p)
                                } else {
                                    cwd.join(p)
                                };
                                if path.exists() && seen.insert(path.clone()) {
                                    paths.push(path);
                                }
                            }
                        }
                    }
                    extract_paths_recursive(v, keys, cwd, paths, seen, depth + 1);
                }
            }
            serde_json::Value::Array(arr) => {
                for v in arr {
                    extract_paths_recursive(v, keys, cwd, paths, seen, depth + 1);
                }
            }
            _ => {}
        }
    }

    extract_paths_recursive(item, &path_keys, cwd, &mut paths, &mut seen, 0);
    paths
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
    agent_id: String,
    cancel: Arc<AtomicBool>,
    done_rx: Receiver<()>,
    handle: JoinHandle<()>,
}

struct StdoutCapture {
    stdout: Vec<u8>,
    json_errors: Vec<String>,
}

/// Hard ceiling on the per-turn stdout buffer. The captured `Vec<u8>` feeds
/// `extract_thread_id_from_jsonl` / `extract_token_count_from_jsonl` after the
/// turn ends, and those terminal events live at the tail of the stream — so on
/// overflow we drop from the FRONT (at a newline boundary, to keep JSONL
/// parseable). 100 MB is roughly 10× a worst-case legitimate Codex turn (large
/// reasoning + many tool calls); anything larger is a runaway and would OOM at
/// `MAX_SWARM_SIZE` concurrency without this cap.
const STDOUT_TAIL_CAP_BYTES: usize = 100 * 1024 * 1024;
/// Max retained JSON error messages per turn. Bounded because a malformed
/// stream can spam errors line-after-line. We keep the latest by dropping the
/// oldest half on overflow (amortised O(1) per push).
const JSON_ERRORS_CAP: usize = 256;

fn append_stdout_line_capped(buf: &mut Vec<u8>, line: &[u8]) {
    buf.extend_from_slice(line);
    if buf.len() <= STDOUT_TAIL_CAP_BYTES {
        return;
    }
    // Aim to keep ~75% of the cap so we don't churn on every line near the
    // boundary. Drain from the front through the next newline to preserve
    // record alignment for downstream JSONL parsers.
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
    resume_thread_id: Option<String>,
    persist_session: bool,
    reasoning_effort: Option<String>,
    prompt: String,
    read_only: bool,
    config: CodexRunnerConfig,
) -> ActiveTurn {
    let agent_id = model.clone();
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
                read_only,
                config,
                cancel_worker,
            );
            let _ = done_tx.send(());
        })
        .expect("spawn codex turn worker");
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
    resume_thread_id: Option<String>,
    persist_session: bool,
    reasoning_effort: Option<String>,
    prompt: String,
    read_only: bool,
    config: CodexRunnerConfig,
    cancel: Arc<AtomicBool>,
) {
    let started_at = Instant::now();
    let out_file = std::env::temp_dir().join(format!("nit-codex-last-message-{seq}.txt"));

    let mut cmd = Command::new("codex");
    cmd.args(build_codex_exec_args(
        model.as_str(),
        cwd.as_path(),
        persist_session,
        reasoning_effort.as_deref(),
        out_file.as_path(),
        resume_thread_id.as_deref(),
        read_only,
        &config,
    ))
    // Read prompt from stdin so multi-line input works without shell escaping.
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
                                    push_json_error_capped(&mut json_errors, msg.to_string());
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
        // Emit a TurnFailed so AppState releases this active turn and the
        // breather/UI stop showing the agent as Running. The bus handler
        // detects the OPERATOR_CANCEL_TURN_MESSAGE sentinel and routes
        // this down a "soft" path (Idle status, Info diag) rather than
        // the "Error" path that genuine subprocess failures take.
        let _ = event_tx.send(AgentBusEvent::TurnFailed {
            agent_id: model,
            mission_id,
            thread_id: resume_thread_id.clone(),
            token_count: None,
            message: nit_core::OPERATOR_CANCEL_TURN_MESSAGE.into(),
        });
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
                endpoint: codex_exec_endpoint_label(&model, resume_thread_id.as_deref()),
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
    let endpoint = codex_exec_endpoint_label(&model, resume_thread_id.as_deref());
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
        thread_id,
        token_count,
        message,
    });
}

fn codex_exec_endpoint_label(agent_id: &str, resume_thread_id: Option<&str>) -> String {
    let model_slug = codex_model_slug_for_agent_id(agent_id);
    let suffix = if model_slug == agent_id {
        String::new()
    } else {
        format!(" (agent {agent_id})")
    };
    if let Some(thread_id) = resume_thread_id {
        format!(
            "codex exec resume {} -m {model_slug}{suffix}",
            shorten_thread_id(thread_id),
        )
    } else {
        format!("codex exec -m {model_slug}{suffix}")
    }
}

fn codex_model_slug_for_agent_id(agent_id: &str) -> &str {
    // Strip every clone-style suffix in one go: split on the first '#'.
    // All known suffix conventions (`#swarm-…`, `#chat-clone-…`,
    // `#shadow-…`, `#mp-pane-…`) start with `#`, and base model slugs
    // never contain `#`. This also handles nested suffixes like
    // `claude-opus-4-7#mp-pane-01#swarm-mis-001-clone-01` — the FIRST
    // `#` always separates the model slug from the lane decoration.
    match agent_id.split_once('#') {
        Some((base, _)) if !base.trim().is_empty() => base,
        _ => agent_id,
    }
}

fn build_codex_mcp_tool_call(
    agent_id: &str,
    prompt: &str,
    cwd: &Path,
    reasoning_effort: Option<&str>,
    config: &CodexRunnerConfig,
    resume_thread_id: Option<&str>,
    read_only: bool,
) -> (&'static str, serde_json::Value) {
    if let Some(thread_id) = resume_thread_id {
        return (
            "codex-reply",
            serde_json::json!({ "threadId": thread_id, "prompt": prompt }),
        );
    }

    let mut args = serde_json::Map::new();
    args.insert(
        "prompt".into(),
        serde_json::Value::String(prompt.to_string()),
    );
    args.insert(
        "model".into(),
        serde_json::Value::String(codex_model_slug_for_agent_id(agent_id).to_string()),
    );
    args.insert(
        "cwd".into(),
        serde_json::Value::String(cwd.to_string_lossy().to_string()),
    );
    if let Some(effort) = reasoning_effort.map(str::trim).filter(|s| !s.is_empty()) {
        args.insert(
            "config".into(),
            serde_json::json!({ "model_reasoning_effort": effort }),
        );
    }
    let sandbox_override = if read_only { Some("read-only") } else { None };
    let sandbox_value = sandbox_override.or_else(|| {
        config
            .sandbox
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
    });
    if let Some(sandbox) = sandbox_value {
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
}

#[allow(clippy::too_many_arguments)]
fn build_codex_exec_args(
    agent_id: &str,
    cwd: &Path,
    persist_session: bool,
    reasoning_effort: Option<&str>,
    out_file: &Path,
    resume_thread_id: Option<&str>,
    read_only: bool,
    config: &CodexRunnerConfig,
) -> Vec<String> {
    let mut args = Vec::new();
    if let Some(policy) = config
        .approval_policy
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        args.push("-a".into());
        args.push(policy.to_string());
    }
    let sandbox_override = if read_only { Some("read-only") } else { None };
    let sandbox_value = sandbox_override.or_else(|| {
        config
            .sandbox
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
    });
    if let Some(sandbox) = sandbox_value {
        args.push("-s".into());
        args.push(sandbox.to_string());
    }

    let model_slug = codex_model_slug_for_agent_id(agent_id);
    if let Some(thread_id) = resume_thread_id {
        args.push("exec".into());
        args.push("resume".into());
        args.push("--json".into());
        args.push("-m".into());
        args.push(model_slug.to_string());
        if let Some(effort) = reasoning_effort.map(str::trim).filter(|s| !s.is_empty()) {
            // Override any global config (e.g. `xhigh`) that some models don't support.
            args.push("-c".into());
            args.push(format!("model_reasoning_effort={effort:?}"));
        }
        // nit-mcp override (when a back-channel socket is set): register
        // `nit-mcp-server` as a Codex-discoverable tool server.
        push_nit_mcp_config_args(&mut args, config, agent_id);
        args.push("-o".into());
        args.push(out_file.to_string_lossy().to_string());
        // Positional SESSION_ID comes after options for `codex exec resume`.
        args.push(thread_id.to_string());
        args.push("-".into());
        return args;
    }

    args.push("exec".into());
    args.push("--json".into());
    args.push("--color".into());
    args.push("never".into());
    if !persist_session {
        args.push("--ephemeral".into());
    }
    args.push("-m".into());
    args.push(model_slug.to_string());
    args.push("-C".into());
    args.push(cwd.to_string_lossy().to_string());
    if let Some(effort) = reasoning_effort.map(str::trim).filter(|s| !s.is_empty()) {
        // Override any global config (e.g. `xhigh`) that some models don't support.
        args.push("-c".into());
        args.push(format!("model_reasoning_effort={effort:?}"));
    }
    push_nit_mcp_config_args(&mut args, config, agent_id);
    args.push("-o".into());
    args.push(out_file.to_string_lossy().to_string());
    args.push("-".into());
    args
}

// Push `-c mcp_servers.nit=...` overrides so the child Codex process can
// discover the back-channel MCP server via a TOML inline table. The agent id
// propagates via env so signals/claims carry the right `posted_by`.
//
// Note: Codex's exact TOML inline-table escaping for `-c` overrides hasn't
// been empirically verified — if Codex rejects this override at runtime the
// in-process nit-mcp side still works; only the Codex-discoverable tool
// bridge is affected.
fn push_nit_mcp_config_args(args: &mut Vec<String>, config: &CodexRunnerConfig, agent_id: &str) {
    let Some(socket_path) = config
        .mcp_backchannel_socket
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    else {
        return;
    };
    let Some(bin_path) = nit_mcp_server_binary_path() else {
        return;
    };
    // Escape backslashes and double quotes so the TOML-string literals remain
    // well-formed no matter what lives in $PATH.
    let bin_esc = escape_toml_string(&bin_path);
    let sock_esc = escape_toml_string(socket_path);
    let agent_esc = escape_toml_string(agent_id);
    let value = format!(
        "{{ command = \"{bin_esc}\", args = [], env = {{ NIT_MCP_BACKCHANNEL_SOCKET = \"{sock_esc}\", NIT_MCP_AGENT_ID = \"{agent_esc}\" }} }}"
    );
    args.push("-c".into());
    args.push(format!("mcp_servers.nit={value}"));
}

// Locate `nit-mcp-server` next to the running binary. `cargo install` lays it
// down alongside `nit`; development builds land it in the same `target/debug`.
// Returns `None` when discovery fails so callers can skip the `-c` injection.
fn nit_mcp_server_binary_path() -> Option<String> {
    let self_exe = std::env::current_exe().ok()?;
    let dir = self_exe.parent()?;
    let candidate = dir.join("nit-mcp-server");
    Some(candidate.to_string_lossy().into_owned())
}

fn escape_toml_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            _ => out.push(ch),
        }
    }
    out
}

fn shorten_thread_id(thread_id: &str) -> String {
    const MAX_CHARS: usize = 8;
    let id = thread_id.trim();
    match id.char_indices().nth(MAX_CHARS) {
        Some((idx, _)) => format!("{}…", &id[..idx]),
        None => id.to_string(),
    }
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
    let kind = payload.get("type").and_then(|v| v.as_str())?;
    if kind == "token_count" {
        let info = payload.get("info")?;
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
    let usage = payload.get("usage")?;
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

// Evaluate-genome tool — injected into every agent prompt so agents know nit
// measures their output automatically. They don't call the tool; the runner
// watches for the marker in case an agent explicitly requests a report.
pub const EVALUATE_GENOME_TOOL_DESCRIPTION: &str = r#"
[nit tool: evaluate_genome]
nit evaluates genome quality automatically in real time as you write files.
You do NOT need to call [evaluate_genome] — nit measures quality externally
after your changes are written to disk. If quality degrades, nit will retry
your turn automatically with specific per-encoder feedback.

Focus on writing good code using the encoder guide and recommendations above.
nit handles the measurement.
[/nit tool]
"#;

// Looks for `[evaluate_genome:<path>]` in agent output and returns a formatted
// genome report when the path resolves to a readable file.
pub fn handle_evaluate_genome_request(workspace_root: &Path, message: &str) -> Option<String> {
    let marker = "[evaluate_genome:";
    let start = message.find(marker)?;
    let rest = &message[start + marker.len()..];
    let end = rest.find(']')?;
    let raw_path = rest[..end].trim();
    if raw_path.is_empty() {
        return None;
    }
    let file_path = if std::path::Path::new(raw_path).is_absolute() {
        std::path::PathBuf::from(raw_path)
    } else {
        workspace_root.join(raw_path)
    };
    let text = std::fs::read_to_string(&file_path).ok()?;
    let report = nit_core::compute_genome_report(&text, &file_path);
    Some(nit_core::format_genome_report(&report))
}

#[cfg(test)]
#[path = "tests/codex_runner.rs"]
mod tests;
