//! Pin a busy chat follow-up before it leaves the composer. Each receipt has
//! exactly one delivery path, including when a provider disconnects after write.
use super::{chat_input, dispatch, timestamp_label};
use crate::{
    claude_runner::ClaudeRunner,
    codex_runner::CodexRunner,
    steering::{self, SteerOutcome},
    vitals::VitalsState,
};
use nit_core::{
    followups::{self, Receipt, Status},
    AgentBusEvent, AgentChannel, AgentMessage, AppState,
};
use std::{
    io,
    path::{Path, PathBuf},
    sync::mpsc::{Receiver, TryRecvError},
    time::{SystemTime, UNIX_EPOCH},
};

#[derive(Default)]
pub(crate) struct PendingSteers(Vec<Followup>);
struct Followup {
    root: PathBuf,
    note: Receipt,
    prompt: usize,
    parent: Option<usize>,
    reply: Option<Receiver<SteerOutcome>>,
    parent_result: Option<(bool, String)>,
    notice: Option<usize>,
    fallback_blocked: bool,
    saved: bool,
}

fn save(root: &Path, note: &Receipt) -> io::Result<()> {
    let dir = followups::ensure_directory(root, &[".nit", "chat", "followups"])?;
    nit_utils::write_atomic(&dir.join(format!("{}.json", note.id)), |writer| {
        serde_json::to_writer_pretty(writer, note).map_err(io::Error::other)
    })
}

fn queued(state: &AppState, agent: &str, prompt: Option<usize>) -> bool {
    state
        .agents
        .queued_codex_turns
        .iter()
        .any(|q| q.agent_id == agent && (prompt.is_none() || q.prompt_msg_idx == prompt))
        || state
            .agents
            .queued_claude_turns
            .iter()
            .any(|q| q.agent_id == agent && (prompt.is_none() || q.prompt_msg_idx == prompt))
}
fn active_prompt(state: &AppState, agent: &str) -> Option<usize> {
    let agents = &state.agents;
    if agents.agents_get(agent)?.is_claude() {
        agents.claude_turn_prompt_idx.get(agent).copied()
    } else {
        agents.codex_turn_prompt_idx.get(agent).copied()
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn submit(
    state: &mut AppState,
    vitals: &mut VitalsState,
    codex: Option<&CodexRunner>,
    claude: Option<&ClaudeRunner>,
    pending: &mut PendingSteers,
    agent: &str,
    mission: Option<&str>,
    text: &str,
    raw: &str,
    force_queue: bool,
) -> bool {
    let root = dispatch::resolve_dispatch_cwd(state, agent);
    let parent = active_prompt(state, agent);
    let turn = if force_queue
        || queued(state, agent, None)
        || pending
            .0
            .iter()
            .any(|f| f.note.agent_id == agent && f.reply.is_some())
    {
        None
    } else {
        parent
            .and_then(|i| state.agents.messages.get(i))
            .filter(|m| m.agent_id.is_none() && m.mission_id.as_deref() == mission)
            .and_then(|m| {
                steering::live_turn(codex, claude, agent, mission, &root)
                    .filter(|t| t.matches_chat_prompt(&m.text))
            })
    };
    let Some(uuid) = steering::message_id() else {
        state.status = Some("Could not create a follow-up receipt. Draft kept.".into());
        return false;
    };
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let label = if turn.is_some() {
        "Sending to the active turn"
    } else {
        "Queued for the next turn"
    };
    let note = Receipt {
        id: format!("{nanos:x}-{uuid}"),
        run_id: uuid.clone(),
        agent_id: agent.into(),
        mission_id: mission.unwrap_or_default().into(),
        task_id: String::new(),
        text: text.into(),
        source: None,
        status: if turn.is_some() {
            Status::Steering
        } else {
            Status::Pending
        },
        detail: Some(label.into()),
    };
    if let Err(err) = save(&root, &note) {
        state.status = Some(format!("Draft kept. Could not save follow-up: {err}"));
        return false;
    }
    let prompt = state.agents.messages.len();
    chat_input::record_chat_message(state, AgentChannel::Agent, text, raw);
    chat_input::chat_history_remember(state, raw);
    state.agents.chat_needs_recipient = false;
    let mut entry = Followup {
        root,
        note,
        prompt,
        parent,
        reply: None,
        parent_result: None,
        notice: None,
        fallback_blocked: false,
        saved: true,
    };
    if let Some(turn) = turn {
        entry.notice = Some(state.agents.messages.len());
        state.agents.messages.push(AgentMessage {
            at: timestamp_label(state),
            channel: AgentChannel::Agent,
            agent_id: Some(agent.into()),
            mission_id: mission.map(str::to_owned),
            text: format!("{label} · {agent}"),
            kind: Some("followup-history".into()),
            prompt_msg_idx: Some(prompt),
            turn: None,
        });
        entry.reply = Some(turn.send(codex, claude, uuid, text.into()));
    } else {
        entry.queue(state, vitals);
    }
    state.status = Some(format!("{label} · {agent}"));
    pending.0.push(entry);
    true
}

impl Followup {
    fn queue(&self, state: &mut AppState, vitals: &mut VitalsState) {
        let enqueue = if state
            .agents
            .agents_get(&self.note.agent_id)
            .is_some_and(|a| a.is_claude())
        {
            dispatch::enqueue_claude_turn
        } else {
            dispatch::enqueue_codex_turn
        };
        enqueue(
            state,
            vitals,
            Some(self.note.agent_id.clone()),
            self.mission(),
            self.note.text.clone(),
            Some(self.prompt),
        );
    }
    fn mission(&self) -> Option<String> {
        (!self.note.mission_id.is_empty()).then(|| self.note.mission_id.clone())
    }
    fn transition(&mut self, state: &mut AppState, status: Status, detail: impl Into<String>) {
        let detail = detail.into();
        self.note.status = status;
        self.note.detail = Some(detail.clone());
        self.saved = false;
        if let Some(message) = self.notice.and_then(|i| state.agents.messages.get_mut(i)) {
            if message.kind.as_deref() == Some("followup-history")
                && message.agent_id.as_deref() == Some(self.note.agent_id.as_str())
                && message.prompt_msg_idx == Some(self.prompt)
            {
                message.text = format!("{detail} · {}", self.note.agent_id);
                state.agents.console_rows_cache.key = None;
                self.mark_provenance_dirty(state);
            }
        }
        state.status = Some(format!("{detail} · {}", self.note.agent_id));
    }
    fn mark_provenance_dirty(&self, state: &mut AppState) {
        if let Some(mid) = self.mission() {
            super::mark_mission_provenance_dirty(state, &mid);
        } else {
            chat_input::mark_ad_hoc_provenance_dirty(state, &self.note.agent_id);
        }
    }
    fn persist(&mut self, state: &mut AppState) {
        if !self.saved {
            match save(&self.root, &self.note) {
                Ok(()) => self.saved = true,
                Err(err) => {
                    state.status = Some(format!(
                        "Follow-up {} could not be updated: {err}",
                        self.note.id
                    ))
                }
            }
        }
    }
    fn finish(&mut self, state: &mut AppState, success: bool) {
        if success {
            self.transition(
                state,
                Status::Delivered,
                "Turn completed with your follow-up",
            );
        } else {
            self.transition(
                state,
                Status::Failed,
                "Turn failed or was cancelled. Follow-up saved",
            );
        }
    }
    fn finish_accepted(&mut self, state: &mut AppState) {
        if let Some((success, result)) = self.parent_result.take() {
            self.finish(state, success);
            if !result.is_empty() {
                self.note.detail = Some(format!("Original turn result: {result}"));
            }
        }
    }

    fn apply(&mut self, state: &mut AppState, vitals: &mut VitalsState, outcome: SteerOutcome) {
        if !matches!(outcome, SteerOutcome::Received) {
            self.reply = None;
        }
        match outcome {
            SteerOutcome::Accepted => {
                self.transition(state, Status::Accepted, "Accepted on the current turn");
                self.finish_accepted(state);
            }
            SteerOutcome::Received => {
                self.transition(state, Status::Received, "Received by the Claude session")
            }
            SteerOutcome::Completed { message, failed } => {
                state.agents.messages.push(AgentMessage {
                    at: timestamp_label(state),
                    channel: AgentChannel::Agent,
                    agent_id: Some(self.note.agent_id.clone()),
                    mission_id: self.mission(),
                    text: message.clone(),
                    prompt_msg_idx: Some(self.prompt),
                    turn: Some(nit_core::state::AgentTurnReceipt {
                        outcome: if failed {
                            nit_core::state::TurnOutcome::Failed
                        } else {
                            nit_core::state::TurnOutcome::Completed
                        },
                        elapsed_ms: None,
                    }),
                    kind: None,
                });
                self.mark_provenance_dirty(state);
                self.finish(state, !failed);
                // Keep the correlated reply available through /followups after restart.
                self.note.detail = Some(message);
            }
            SteerOutcome::Unknown(reason) => self.transition(
                state,
                Status::Unknown,
                format!("Delivery unconfirmed. Check /followups before retrying. {reason}"),
            ),
            SteerOutcome::Stale | SteerOutcome::Unavailable(_) => {
                // A definitive rejection permits exactly one queued delivery.
                // A later turn, lane reset, or cwd change requires the operator
                // to rebind the saved text explicitly.
                let can_queue = !self.fallback_blocked
                    && !matches!(self.parent_result, Some((false, _)))
                    && state.agents.agents_get(&self.note.agent_id).is_some()
                    && self.root == dispatch::resolve_dispatch_cwd(state, &self.note.agent_id)
                    && state
                        .agents
                        .messages
                        .get(self.prompt)
                        .is_some_and(|m| m.agent_id.is_none() && m.text == self.note.text)
                    && (!state.agents.active_turns.contains_key(&self.note.agent_id)
                        || active_prompt(state, &self.note.agent_id) == self.parent);
                if can_queue {
                    // Live delivery was attempted only with an empty lane queue.
                    // Notes queued since then belong after this definitively unsent one.
                    self.queue(state, vitals);
                    let agent = &self.note.agent_id;
                    if state
                        .agents
                        .agents_get(agent)
                        .is_some_and(|a| a.is_claude())
                    {
                        prioritize_fallback(
                            &mut state.agents.queued_claude_turns,
                            |q| (&q.agent_id, q.prompt_msg_idx),
                            agent,
                            self.prompt,
                        );
                    } else {
                        prioritize_fallback(
                            &mut state.agents.queued_codex_turns,
                            |q| (&q.agent_id, q.prompt_msg_idx),
                            agent,
                            self.prompt,
                        );
                    }
                    self.transition(
                        state,
                        Status::Pending,
                        "Current turn closed. Queued for the next turn",
                    );
                } else {
                    self.transition(state,Status::Failed,"Turn changed before delivery. Restore the saved note with /followups recover");
                }
            }
        }
    }
}

fn prioritize_fallback<T>(
    queue: &mut std::collections::VecDeque<T>,
    key: impl Fn(&T) -> (&String, Option<usize>),
    agent: &str,
    prompt: usize,
) {
    if !queue.back().is_some_and(|q| {
        let (lane, index) = key(q);
        lane == agent && index == Some(prompt)
    }) {
        return;
    }
    let entry = queue.pop_back().expect("checked queue back");
    let position = queue
        .iter()
        .position(|q| key(q).0 == agent)
        .unwrap_or(queue.len());
    queue.insert(position, entry);
}

impl PendingSteers {
    pub(super) fn cancel(&mut self, target: &str) {
        for note in &mut self.0 {
            if note.note.agent_id == target {
                note.fallback_blocked = true;
            }
        }
    }

    pub(super) fn event(&mut self, state: &mut AppState, event: &AgentBusEvent) {
        let (agent, mission, result) = match event {
            AgentBusEvent::TurnStarted {
                agent_id,
                mission_id,
                ..
            } => (agent_id, mission_id, None),
            AgentBusEvent::TurnCompleted {
                agent_id,
                mission_id,
                message,
                ..
            } => (agent_id, mission_id, Some((true, message))),
            AgentBusEvent::TurnFailed {
                agent_id,
                mission_id,
                message,
                ..
            } => (agent_id, mission_id, Some((false, message))),
            _ => return,
        };
        let active = active_prompt(state, agent);
        if result.is_none() {
            for entry in &mut self.0 {
                if entry.note.agent_id == *agent
                    && (active.is_none() || entry.parent != active || entry.mission() != *mission)
                {
                    entry.fallback_blocked = true;
                }
            }
        }
        let Some(idx) = active else { return };
        for entry in self.0.iter_mut().filter(|f| {
            &f.note.agent_id == agent && f.note.mission_id == mission.as_deref().unwrap_or_default()
        }) {
            if entry.prompt == idx
                && matches!(entry.note.status, Status::Pending | Status::Submitted)
            {
                if let Some((ok, _)) = result {
                    entry.finish(state, ok)
                } else {
                    entry.transition(state, Status::Submitted, "Follow-up turn started");
                }
            } else if entry.parent == Some(idx)
                && matches!(entry.note.status, Status::Steering | Status::Accepted)
            {
                if let Some((ok, message)) = result {
                    entry.parent_result = Some((ok, message.clone()));
                    if entry.note.status == Status::Accepted {
                        entry.finish_accepted(state);
                    }
                }
            }
            entry.persist(state);
        }
    }

    pub(crate) fn poll(
        &mut self,
        state: &mut AppState,
        vitals: &mut VitalsState,
        codex: &CodexRunner,
        claude: &ClaudeRunner,
    ) -> bool {
        let mut changed = false;
        let mut queued_fallback = false;
        for entry in &mut self.0 {
            while let Some(rx) = &entry.reply {
                let outcome = match rx.try_recv() {
                    Ok(outcome) => outcome,
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        SteerOutcome::Unknown("Provider connection closed".into())
                    }
                };
                entry.apply(state, vitals, outcome);
                queued_fallback |= entry.note.status == Status::Pending;
                changed = true;
            }
            if entry.note.status == Status::Pending
                && !queued(state, &entry.note.agent_id, Some(entry.prompt))
                && !(state.agents.active_turns.contains_key(&entry.note.agent_id)
                    && active_prompt(state, &entry.note.agent_id) == Some(entry.prompt))
            {
                entry.transition(
                    state,
                    Status::Failed,
                    "Queue entry closed before delivery. Follow-up saved",
                );
                changed = true;
            }
            entry.persist(state);
        }
        self.0.retain(|item| {
            !item.saved
                || !matches!(
                    item.note.status,
                    Status::Delivered | Status::Failed | Status::Unknown
                )
        });
        if queued_fallback {
            dispatch::maybe_dispatch_next_queued_codex_turn(state, vitals, Some(codex));
            dispatch::maybe_dispatch_next_queued_claude_turn(state, vitals, Some(claude));
        }
        changed
    }
}

#[cfg(test)]
mod tests;
