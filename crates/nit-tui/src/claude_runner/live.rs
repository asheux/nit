//! Correlate stream-json input with Claude's replay and terminal user UUIDs.
//! A successful pipe write alone does not acknowledge an operator message.
use crate::{claude_pool::PoolWorker, steering::SteerOutcome};
use serde_json::{json, Value};
use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::{mpsc, Arc, Mutex},
    time::{Duration, Instant},
};

#[derive(Clone, Debug)]
pub struct ClaudeLiveTurn {
    agent: String,
    mission: Option<String>,
    cwd: PathBuf,
    session: String,
    input: String,
    prompt: Arc<str>,
    tx: mpsc::Sender<Request>,
}
impl ClaudeLiveTurn {
    pub fn matches_chat_prompt(&self, text: &str) -> bool {
        crate::steering::matches_chat_prompt(&self.prompt, text)
    }
}
#[derive(Clone, Default)]
pub(super) struct Registry(Arc<Mutex<HashMap<String, ClaudeLiveTurn>>>);

impl Registry {
    pub(super) fn lookup(
        &self,
        agent: &str,
        mission: Option<&str>,
        cwd: &Path,
    ) -> Option<ClaudeLiveTurn> {
        self.0
            .lock()
            .ok()?
            .get(agent)
            .filter(|t| t.mission.as_deref() == mission && t.cwd == cwd)
            .cloned()
    }
    pub(super) fn steer(
        &self,
        target: ClaudeLiveTurn,
        id: String,
        text: String,
    ) -> mpsc::Receiver<SteerOutcome> {
        let (tx, rx) = mpsc::channel();
        let request = Request {
            id,
            text,
            reply: Some(tx),
            written: false,
            received: false,
        };
        if let Some(current) = self.lookup(&target.agent, target.mission.as_deref(), &target.cwd) {
            if current.input == target.input && current.session == target.session {
                let _ = current.tx.send(request);
            }
        }
        rx
    }
}

#[derive(Debug)]
struct Request {
    id: String,
    text: String,
    reply: Option<mpsc::Sender<SteerOutcome>>,
    written: bool,
    received: bool,
}
impl Request {
    fn finish(&mut self, outcome: SteerOutcome) {
        if let Some(tx) = self.reply.take() {
            let _ = tx.send(outcome);
        }
    }
}
impl Drop for Request {
    fn drop(&mut self) {
        if self.written {
            self.finish(SteerOutcome::Unknown(
                "Claude session ended before a correlated result; input was not replayed".into(),
            ));
        } else {
            self.finish(SteerOutcome::Stale);
        }
    }
}

pub(super) use crate::steering::message_id as uuid;
pub(super) fn envelope(input: &str, session: &str, prompt: &str) -> String {
    format!(
        "{}\n",
        json!({"type":"user","uuid":input,"session_id":session,"parent_tool_use_id":null,"message":{"role":"user","content":prompt}})
    )
}

pub(super) struct LiveInput {
    registry: Registry,
    target: ClaudeLiveTurn,
    rx: mpsc::Receiver<Request>,
    pending: HashMap<String, Request>,
    seen: HashSet<String>,
    advertised: bool,
    correlation: bool,
    closed: bool,
    pub(super) uncertain: bool,
}
impl LiveInput {
    pub(super) fn new(
        registry: Registry,
        agent: &str,
        mission: Option<String>,
        cwd: &Path,
        session: &str,
        input: String,
        prompt: &str,
    ) -> Self {
        let (tx, rx) = mpsc::channel();
        Self {
            registry,
            target: ClaudeLiveTurn {
                agent: agent.into(),
                mission,
                cwd: cwd.into(),
                session: session.into(),
                input,
                prompt: Arc::from(prompt),
                tx,
            },
            rx,
            pending: HashMap::new(),
            seen: HashSet::new(),
            advertised: false,
            correlation: false,
            closed: false,
            uncertain: false,
        }
    }
    pub(super) fn pump(&mut self, worker: &mut PoolWorker) {
        while let Ok(mut req) = self.rx.try_recv() {
            if self.closed || self.uncertain {
                continue;
            }
            if !self.seen.insert(req.id.clone()) {
                req.finish(SteerOutcome::Unknown(
                    "This input ID was already submitted. Check its original receipt.".into(),
                ));
                continue;
            }
            let id = req.id.clone();
            if id.len() != 36
                || !id.bytes().enumerate().all(|(i, b)| match i {
                    8 | 13 | 18 | 23 => b == b'-',
                    14 => b == b'4',
                    19 => matches!(b, b'8' | b'9' | b'a' | b'b'),
                    _ => b.is_ascii_hexdigit(),
                })
            {
                req.finish(SteerOutcome::Unavailable(
                    "Invalid Claude input UUID".into(),
                ));
                continue;
            }
            req.written = true;
            if worker
                .write_envelope(envelope(&id, &self.target.session, &req.text).as_bytes())
                .is_err()
            {
                self.uncertain = true;
                self.close();
                continue;
            }
            self.pending.insert(id, req);
        }
    }
    pub(super) fn close(&mut self) {
        self.closed = true;
        if let Ok(mut all) = self.registry.0.lock() {
            if all
                .get(&self.target.agent)
                .is_some_and(|t| t.input == self.target.input)
            {
                all.remove(&self.target.agent);
            }
        }
        while self.rx.try_recv().is_ok() {}
    }
    /// Return true for a supplemental/foreign event that must never enter the original transcript.
    pub(super) fn observe(&mut self, line: &str) -> bool {
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            return false;
        };
        let session = v["session_id"].as_str();
        if session.is_some_and(|id| id != self.target.session) {
            return true;
        }
        let matched_session = session == Some(self.target.session.as_str());
        let ids: Vec<&str> = v["user_message_uuids"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .chain(v["user_message_uuid"].as_str())
            .collect();
        let primary = ids.contains(&self.target.input.as_str());
        if matched_session && v["type"] == "system" && v["subtype"] == "init" && primary {
            self.correlation = true;
        }
        if matched_session && v["type"] == "user" && v["isReplay"] == true {
            if let Some(id) = v["uuid"].as_str() {
                if let Some(req) = self.pending.get_mut(id) {
                    if !req.received {
                        req.received = true;
                        if let Some(tx) = &req.reply {
                            let _ = tx.send(SteerOutcome::Received);
                        }
                    }
                } else if id == self.target.input
                    && self.correlation
                    && !self.advertised
                    && !self.closed
                {
                    if let Ok(mut all) = self.registry.0.lock() {
                        all.insert(self.target.agent.clone(), self.target.clone());
                        self.advertised = true;
                    }
                }
            }
            return true;
        }
        let secondary = matched_session && ids.iter().any(|id| self.pending.contains_key(*id));
        let ids_foreign = !ids.is_empty() && !primary;
        if v["type"] == "result" {
            if !self.pending.is_empty() && (!matched_session || ids.is_empty()) {
                // A terminal response without a reliable identity cannot be assigned
                // to either prompt. The owner will retire this process.
                self.uncertain = true;
                self.close();
                self.pending.clear();
            }
            if secondary {
                for id in ids {
                    if let Some(mut req) = self.pending.remove(id) {
                        if let Some(message) = v["result"].as_str().filter(|s| !s.is_empty()) {
                            let failed = v["is_error"].as_bool() == Some(true)
                                || v["subtype"]
                                    .as_str()
                                    .is_some_and(|s| s.starts_with("error"))
                                || v["api_error_status"].as_u64().is_some_and(|n| n >= 400);
                            req.finish(SteerOutcome::Completed {
                                message: message.into(),
                                failed,
                            });
                        }
                    }
                }
            }
            if primary {
                self.close();
            }
        }
        !primary && (secondary || ids_foreign)
    }
    pub(super) fn drain(
        &mut self,
        worker: &mut PoolWorker,
        cancel: &std::sync::atomic::AtomicBool,
    ) {
        self.close();
        let deadline = Instant::now() + Duration::from_secs(120);
        while !self.pending.is_empty()
            && Instant::now() < deadline
            && !cancel.load(std::sync::atomic::Ordering::Acquire)
        {
            while self.rx.try_recv().is_ok() {}
            if let Some(line) = worker.recv_timeout(Duration::from_millis(50)) {
                self.observe(&line.raw);
            } else if worker.is_dead() {
                break;
            }
        }
        if !self.pending.is_empty() {
            self.uncertain = true;
            self.pending.clear();
        }
    }
}
impl Drop for LiveInput {
    fn drop(&mut self) {
        self.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn identified_duplicate_and_foreign_results_leave_other_inputs_pending() {
        let mut stream = LiveInput::new(
            Default::default(),
            "sonnet",
            None,
            Path::new("work"),
            "session",
            "primary".into(),
            "first",
        );
        let mut listener = |id: &str| {
            let (tx, rx) = mpsc::channel();
            stream.pending.insert(
                id.into(),
                Request {
                    id: id.into(),
                    text: id.into(),
                    reply: Some(tx),
                    written: true,
                    received: false,
                },
            );
            rx
        };
        let first = listener("first-followup");
        let second = listener("second-followup");
        let result = |id: &str| {
            json!({
                "type":"result", "session_id":"session", "subtype":"success",
                "user_message_uuid":id, "result":id,
            })
            .to_string()
        };
        for id in ["second-followup", "second-followup", "unrelated"] {
            assert!(stream.observe(&result(id)));
        }
        assert!(
            matches!(second.recv().unwrap(), SteerOutcome::Completed { message, failed: false } if message == "second-followup")
        );
        assert!(first.try_recv().is_err());
        assert!(!stream.uncertain);
        assert!(!stream.observe(&result("primary")));
        assert!(stream.observe(&result("first-followup")));
        assert!(
            matches!(first.recv().unwrap(), SteerOutcome::Completed { message, failed: false } if message == "first-followup")
        );
        assert!(stream.pending.is_empty());
    }

    #[test]
    fn missing_association_is_unknown_and_never_returns_a_reply_to_another_request() {
        let mut stream = LiveInput::new(
            Default::default(),
            "sonnet",
            None,
            Path::new("work"),
            "session",
            "original".into(),
            "hello",
        );
        let (tx, rx) = mpsc::channel();
        stream.pending.insert(
            "other".into(),
            Request {
                id: "other".into(),
                text: "second".into(),
                reply: Some(tx),
                written: true,
                received: false,
            },
        );
        stream.observe(r#"{"type":"user","uuid":"other","session_id":"wrong","isReplay":true}"#);
        assert!(rx.try_recv().is_err());
        stream.observe(r#"{"type":"result","result":"ambiguous","session_id":"session"}"#);
        assert!(stream.uncertain && stream.pending.is_empty());
        assert!(matches!(rx.recv().unwrap(), SteerOutcome::Unknown(_)));
    }
}
