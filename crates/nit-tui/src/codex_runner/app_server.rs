//! Opt-in app-server transport. Each running lane owns its stdio connection,
//! working directory, tool configuration and acknowledged provider turn ID.
use super::{args, CodexRunnerConfig, ExecOutput, ExecStop};
use crate::steering::SteerOutcome;
use nit_core::{AgentBusEvent, AgentTokenCount};
use serde_json::{json, Value};
use std::{
    collections::{HashMap, HashSet},
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        atomic::Ordering,
        mpsc::{self, Receiver, Sender, SyncSender},
        Arc, Mutex,
    },
    thread,
    time::{Duration, Instant},
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CodexLiveTurn {
    pub thread_id: String,
    pub turn_id: String,
    pub agent_id: String,
    pub mission_id: Option<String>,
    pub cwd: PathBuf,
    origin: Arc<str>,
}
impl CodexLiveTurn {
    pub fn matches_task_turn(&self, nonce: &str) -> bool {
        !nonce.is_empty()
            && nonce.bytes().all(|b| b.is_ascii_hexdigit() || b == b'-')
            && self.origin.lines().any(|line| {
                line.strip_prefix("[nit-task-turn ")
                    .and_then(|s| s.strip_suffix(']'))
                    == Some(nonce)
            })
    }
    pub fn matches_chat_prompt(&self, text: &str) -> bool {
        crate::steering::matches_chat_prompt(&self.origin, text)
    }
}
struct Request {
    id: String,
    text: String,
    reply: Sender<SteerOutcome>,
}
type Route = (CodexLiveTurn, SyncSender<Request>);
#[derive(Clone, Default)]
pub(super) struct LiveRoutes(Arc<Mutex<HashMap<String, Route>>>);
impl LiveRoutes {
    pub(super) fn get(
        &self,
        agent: &str,
        mission: Option<&str>,
        cwd: &Path,
    ) -> Option<CodexLiveTurn> {
        let map = self.0.lock().ok()?;
        let (token, _) = map.get(agent)?;
        (token.mission_id.as_deref() == mission && token.cwd == cwd).then(|| token.clone())
    }
    pub(super) fn steer(
        &self,
        target: CodexLiveTurn,
        id: String,
        text: String,
    ) -> Receiver<SteerOutcome> {
        let (tx, rx) = mpsc::channel();
        let result = if id.is_empty() || id.len() > 128 || text.trim().is_empty() {
            Some(SteerOutcome::Unavailable(
                "Missing message or valid receipt ID".into(),
            ))
        } else {
            let map = self.0.lock().expect("Codex routes");
            match map.get(&target.agent_id) {
                Some((current, route)) if *current == target => route
                    .try_send(Request {
                        id,
                        text,
                        reply: tx.clone(),
                    })
                    .err()
                    .map(|e| match e {
                        mpsc::TrySendError::Disconnected(_) => SteerOutcome::Stale,
                        mpsc::TrySendError::Full(_) => {
                            SteerOutcome::Unavailable("Steering queue is full".into())
                        }
                    }),
                _ => Some(SteerOutcome::Stale),
            }
        };
        if let Some(result) = result {
            let _ = tx.send(result);
        }
        rx
    }
}

struct Rpc {
    early: std::collections::VecDeque<Value>,
    child: Child,
    writer: Option<SyncSender<Value>>,
    exited: Arc<std::sync::atomic::AtomicBool>,
    input: Receiver<Value>,
}
impl Rpc {
    fn spawn(cwd: &Path, agent: &str, config: &CodexRunnerConfig) -> Result<Self, String> {
        let mut args = Vec::new();
        args::push_nit_mcp_config_args(&mut args, config, agent);
        let mut command = Command::new("codex");
        command
            .current_dir(cwd)
            .args(args)
            .args(["app-server", "--listen", "stdio://"]);
        Self::connect(command)
    }
    fn connect(mut command: Command) -> Result<Self, String> {
        let (reader, writer) =
            crate::runner_output::private_output_stream().map_err(|e| e.to_string())?;
        let exited = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let stdout = ExecOutput {
            file: reader,
            exited: exited.clone(),
            position: 0,
            end: None,
        };
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(writer)
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("Codex app-server failed to start: {e}"))?;
        let mut stdin = child.stdin.take().expect("piped input");
        let (write, queue) = mpsc::sync_channel::<Value>(32);
        thread::spawn(move || {
            for message in queue {
                if serde_json::to_writer(&mut stdin, &message).is_err()
                    || stdin.write_all(b"\n").and_then(|_| stdin.flush()).is_err()
                {
                    break;
                }
            }
        });
        let (output, input) = mpsc::sync_channel(64);
        thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if let Ok(msg) = serde_json::from_str(&line) {
                    if output.send(msg).is_err() {
                        break;
                    }
                }
            }
        });
        Ok(Self {
            exited,
            early: Default::default(),
            child,
            writer: Some(write),
            input,
        })
    }
    fn receive(&mut self, timeout: Duration) -> Result<Value, mpsc::RecvTimeoutError> {
        if self.child.try_wait().ok().flatten().is_some() {
            self.exited.store(true, Ordering::Release);
        }
        self.input.recv_timeout(timeout)
    }
    fn send(&self, value: Value) -> Result<(), String> {
        self.writer
            .as_ref()
            .ok_or("Codex connection closed")?
            .try_send(value)
            .map_err(|_| "Codex app-server write queue unavailable".into())
    }
    fn reject_server_request(&self, msg: &Value) -> Result<(), String> {
        if msg.get("id").is_none() || msg.get("method").is_none() {
            return Ok(());
        }
        // A headless client cannot authorize an interactive permission escalation.
        let id = &msg["id"];
        let result=match msg["method"].as_str(){
            Some("item/commandExecution/requestApproval"|"item/fileChange/requestApproval")=>json!({"decision":"decline"}),
            Some("item/permissions/requestApproval")=>json!({"permissions":{},"scope":"turn"}),
            _=>return self.send(json!({"id":id,"error":{"code":-32601,"message":"Interactive client request unavailable in Nit"}}))
        };
        self.send(json!({"id":id,"result":result}))
    }
    fn request(
        &mut self,
        id: u64,
        method: &str,
        params: Value,
        stop: &ExecStop,
    ) -> Result<Value, String> {
        self.send(json!({"id":id,"method":method,"params":params}))?;
        let start = Instant::now();
        loop {
            if stop.cancelled.load(Ordering::Acquire) {
                return Err(nit_core::OPERATOR_CANCEL_TURN_MESSAGE.into());
            }
            if start.elapsed() > Duration::from_secs(30) {
                return Err(format!("Codex {method} timed out"));
            }
            match self.receive(Duration::from_millis(25)) {
                Ok(v) if v["id"].as_u64() == Some(id) && v.get("method").is_none() => {
                    return v.get("result").cloned().ok_or_else(|| rpc_error(&v))
                }
                Ok(v) if method == "turn/start" && v.get("id").is_none() => {
                    if self.early.len() >= 128 {
                        return Err(
                            "Too many notifications before turn/start acknowledgement".into()
                        );
                    }
                    self.early.push_back(v);
                }
                Ok(v) => self.reject_server_request(&v)?,
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(_) => return Err(format!("Codex disconnected during {method}")),
            }
        }
    }
}
impl Drop for Rpc {
    fn drop(&mut self) {
        self.writer.take();
        // EOF lets the server flush the rollout before a subsequent resume.
        let deadline = Instant::now() + Duration::from_millis(300);
        loop {
            match self.child.try_wait() {
                Ok(Some(_)) => {
                    self.exited.store(true, Ordering::Release);
                    return;
                }
                Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(10)),
                _ => break,
            }
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
        self.exited.store(true, Ordering::Release);
    }
}
fn rpc_error(value: &Value) -> String {
    value
        .pointer("/error/message")
        .and_then(Value::as_str)
        .unwrap_or("Invalid Codex app-server response")
        .to_owned()
}
fn text_input(text: &str) -> Value {
    json!([{"type":"text","text":text,"text_elements":[]}])
}
struct Task<'a> {
    agent: &'a str,
    mission: &'a Option<String>,
    cwd: &'a Path,
    resume: Option<&'a str>,
    persist: bool,
    effort: Option<&'a str>,
    prompt: &'a str,
    read_only: bool,
    config: &'a CodexRunnerConfig,
}
impl Task<'_> {
    fn open(&self, rpc: &mut Rpc, stop: &ExecStop) -> Result<CodexLiveTurn, String> {
        rpc.request(1,"initialize",json!({"clientInfo":{"name":"nit","title":"Nit","version":env!("CARGO_PKG_VERSION")},"capabilities":{"experimentalApi":true}}),stop)?;
        rpc.send(json!({"method":"initialized"}))?;
        let mut p = json!({"cwd":self.cwd,"model":args::codex_model_slug_for_agent_id(self.agent)});
        if let Some(s) = if self.read_only {
            Some("read-only")
        } else {
            self.config.sandbox.as_deref()
        } {
            p["sandbox"] = json!(s);
        }
        if let Some(a) = &self.config.approval_policy {
            p["approvalPolicy"] = json!(a);
        }
        let method = if let Some(thread) = self.resume {
            p["threadId"] = json!(thread);
            p["excludeTurns"] = json!(true);
            "thread/resume"
        } else {
            p["ephemeral"] = json!(!self.persist);
            "thread/start"
        };
        let result = rpc.request(2, method, p, stop)?;
        let thread = result
            .pointer("/thread/id")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .ok_or("Missing Codex thread ID")?;
        if self.resume.is_some_and(|prior| prior != thread) {
            return Err("Codex resumed a different thread".into());
        }
        if result
            .pointer("/thread/status/type")
            .and_then(Value::as_str)
            == Some("active")
        {
            return Err("Codex thread already has a running turn".into());
        }
        let mut p = json!({"threadId":thread,"input":text_input(self.prompt)});
        if let Some(effort) = self.effort {
            p["effort"] = json!(effort);
        }
        let result = rpc.request(3, "turn/start", p, stop)?;
        let turn = result
            .pointer("/turn/id")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .ok_or("Missing Codex turn ID")?;
        Ok(CodexLiveTurn {
            thread_id: thread.into(),
            turn_id: turn.into(),
            mission_id: self.mission.clone(),
            agent_id: self.agent.to_string(),
            cwd: self.cwd.to_owned(),
            origin: self.prompt.into(),
        })
    }
}

/// Requests that reached the writer have uncertain delivery until the exact
/// response arrives. Dropping this guard resolves every outstanding waiter.
#[derive(Default)]
struct Pending {
    requests: HashMap<u64, (Instant, Sender<SteerOutcome>)>,
}
impl Drop for Pending {
    fn drop(&mut self) {
        for (_, (_, tx)) in self.requests.drain() {
            let _ = tx.send(SteerOutcome::Unknown(
                "Turn ended before Codex acknowledged the message".into(),
            ));
        }
    }
}
impl Pending {
    fn expire(&mut self) {
        self.requests.retain(|_, (start, tx)| {
            if start.elapsed() < Duration::from_secs(10) {
                true
            } else {
                let _ = tx.send(SteerOutcome::Unknown(
                    "Codex acknowledgement timed out".into(),
                ));
                false
            }
        });
    }
    fn ack(&mut self, value: &Value, target: &CodexLiveTurn) -> bool {
        let Some((_, tx)) = value["id"]
            .as_u64()
            .and_then(|id| self.requests.remove(&id))
        else {
            return false;
        };
        let outcome = if value.pointer("/result/turnId").and_then(Value::as_str)
            == Some(&target.turn_id)
        {
            SteerOutcome::Accepted
        } else if value.get("error").is_some() {
            let detail = rpc_error(value);
            let lower = detail.to_ascii_lowercase();
            if lower.contains("no active turn")
                || lower.contains("active turn mismatch")
                || lower.contains("turn id mismatch")
            {
                SteerOutcome::Stale
            } else {
                SteerOutcome::Unavailable(detail)
            }
        } else {
            SteerOutcome::Unknown("Codex acknowledgement did not match the requested turn".into())
        };
        let _ = tx.send(outcome);
        true
    }
}
struct Session<'a> {
    target: &'a CodexLiveTurn,
    events: &'a Sender<AgentBusEvent>,
    cancel: &'a ExecStop,
    incoming: Receiver<Request>,
    tokens: Option<AgentTokenCount>,
    text: Option<String>,
}
impl Session<'_> {
    fn observe_item(&mut self, item: &Value) {
        if item["type"] == "agentMessage"
            && item
                .get("phase")
                .and_then(Value::as_str)
                .is_none_or(|p| p == "final_answer")
        {
            self.text = item["text"]
                .as_str()
                .filter(|s| !s.trim().is_empty())
                .map(str::to_owned);
        }
    }
    fn drive(&mut self, rpc: &mut Rpc) -> Result<String, String> {
        let mut pending = Pending::default();
        let mut sent = HashSet::new();
        let mut sequence = 10u64;
        let mut heartbeat = Instant::now();
        loop {
            if self.cancel.cancelled.load(Ordering::Acquire) {
                let _=rpc.send(json!({"id":4,"method":"turn/interrupt","params":{"threadId":self.target.thread_id,"turnId":self.target.turn_id}}));
                return Err(nit_core::OPERATOR_CANCEL_TURN_MESSAGE.into());
            }
            pending.expire();
            while let Ok(req) = self.incoming.try_recv() {
                if sent.contains(&req.id) {
                    let _ = req.reply.send(SteerOutcome::Unknown(
                        "This receipt was already submitted to Codex".into(),
                    ));
                    continue;
                }
                if pending.requests.len() >= 8 || sent.len() >= 2048 {
                    let _ = req.reply.send(SteerOutcome::Unavailable(
                        "Steering capacity reached for this turn".into(),
                    ));
                    continue;
                }
                sequence += 1;
                let params = json!({"threadId":self.target.thread_id,"expectedTurnId":self.target.turn_id,"clientUserMessageId":req.id,"input":text_input(&req.text)});
                if let Err(e) =
                    rpc.send(json!({"id":sequence,"method":"turn/steer","params":params}))
                {
                    let _ = req.reply.send(SteerOutcome::Unavailable(e));
                    continue;
                }
                sent.insert(req.id);
                pending
                    .requests
                    .insert(sequence, (Instant::now(), req.reply));
            }
            if heartbeat.elapsed() >= Duration::from_secs(2) {
                let _ = self.events.send(AgentBusEvent::TurnHeartbeat {
                    agent_id: self.target.agent_id.clone(),
                    mission_id: self.target.mission_id.clone(),
                });
                heartbeat = Instant::now();
            }
            let value = if let Some(v) = rpc.early.pop_front() {
                v
            } else {
                match rpc.receive(Duration::from_millis(20)) {
                    Ok(v) => v,
                    Err(mpsc::RecvTimeoutError::Timeout) => continue,
                    Err(_) => return Err("Codex app-server disconnected during turn".into()),
                }
            };
            if value.get("id").is_some() {
                if value.get("method").is_some() {
                    rpc.reject_server_request(&value)?;
                } else {
                    pending.ack(&value, self.target);
                }
                continue;
            }
            let p = &value["params"];
            if p["threadId"].as_str() != Some(&self.target.thread_id) {
                continue;
            }
            if p["turnId"]
                .as_str()
                .or_else(|| p.pointer("/turn/id").and_then(Value::as_str))
                != Some(&self.target.turn_id)
            {
                continue;
            }
            match value["method"].as_str() {
                Some("item/completed") => self.observe_item(&p["item"]),
                Some("item/started") => {
                    if let Some(stage) = p.pointer("/item/type").and_then(Value::as_str) {
                        let _ = self.events.send(AgentBusEvent::TurnStage {
                            agent_id: self.target.agent_id.clone(),
                            mission_id: self.target.mission_id.clone(),
                            stage: stage.into(),
                        });
                    }
                }
                Some("thread/tokenUsage/updated") => {
                    if let Some(usage) = p
                        .pointer("/tokenUsage/last/totalTokens")
                        .and_then(Value::as_u64)
                        .and_then(|n| u32::try_from(n).ok())
                    {
                        let tokens = AgentTokenCount {
                            total_tokens: usage,
                            context_window: p
                                .pointer("/tokenUsage/modelContextWindow")
                                .and_then(Value::as_u64)
                                .and_then(|n| u32::try_from(n).ok())
                                .unwrap_or(0),
                        };
                        let _ = self.events.send(AgentBusEvent::TokenCount {
                            agent_id: self.target.agent_id.clone(),
                            mission_id: self.target.mission_id.clone(),
                            token_count: tokens.clone(),
                        });
                        self.tokens = Some(tokens);
                    }
                }
                Some("turn/completed") => {
                    let turn = &p["turn"];
                    if let Some(items) = turn["items"].as_array() {
                        for item in items {
                            self.observe_item(item);
                        }
                    }
                    return match turn["status"].as_str() {
                        Some("completed") => self
                            .text
                            .take()
                            .ok_or_else(|| "Codex completed without a final agent message".into()),
                        Some("interrupted") => Err(nit_core::OPERATOR_CANCEL_TURN_MESSAGE.into()),
                        _ => Err(turn
                            .pointer("/error/message")
                            .and_then(Value::as_str)
                            .unwrap_or("Codex turn failed")
                            .into()),
                    };
                }
                _ => {}
            }
        }
    }
}
#[allow(clippy::too_many_arguments)]
pub(super) fn run(
    events: &Sender<AgentBusEvent>,
    agent: String,
    cwd: PathBuf,
    mission: Option<String>,
    resume: Option<String>,
    persist: bool,
    effort: Option<String>,
    prompt: String,
    read_only: bool,
    config: CodexRunnerConfig,
    cancel: Arc<ExecStop>,
    routes: LiveRoutes,
) {
    let mut thread_id = resume.clone();
    let mut token_count = None;
    let run = (|| {
        let mut rpc = Rpc::spawn(&cwd, &agent, &config)?;
        let task = Task {
            agent: &agent,
            mission: &mission,
            cwd: &cwd,
            resume: resume.as_deref(),
            persist,
            effort: effort.as_deref(),
            prompt: &prompt,
            read_only,
            config: &config,
        };
        let target = task.open(&mut rpc, &cancel)?;
        thread_id = Some(target.thread_id.clone());
        crate::swarm::report_provider_followup_input(events, &agent, mission.as_deref(), &prompt);
        let (tx, rx) = mpsc::sync_channel(8);
        routes
            .0
            .lock()
            .expect("Codex routes")
            .insert(agent.clone(), (target.clone(), tx));
        let _ = events.send(AgentBusEvent::McpStatus {
            status: nit_core::McpStatus {
                state: nit_core::McpConnectionState::Connected,
                endpoint: "codex app-server (stdio)".into(),
                last_error: None,
                latency_ms: None,
            },
        });
        let mut session = Session {
            target: &target,
            events,
            cancel: &cancel,
            incoming: rx,
            tokens: None,
            text: None,
        };
        let result = session.drive(&mut rpc);
        routes.0.lock().expect("Codex routes").remove(&agent);
        for req in session.incoming.try_iter() {
            let _ = req.reply.send(SteerOutcome::Stale);
        }
        token_count = session.tokens.take();
        result
    })();
    let event = match run {
        Ok(message) => AgentBusEvent::TurnCompleted {
            agent_id: agent,
            mission_id: mission,
            thread_id,
            token_count,
            message,
        },
        Err(message) => AgentBusEvent::TurnFailed {
            agent_id: agent,
            mission_id: mission,
            thread_id,
            token_count,
            message,
        },
    };
    let _ = events.send(event);
}

#[cfg(test)]
#[path = "../tests/codex_app_server.rs"]
mod tests;
