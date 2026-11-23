//! Bind a live correction to one execution of one task. A receipt written
//! before transport submission survives an interruption without automatic replay.
use super::operator_followups::{save, transition};
use super::{push_system_alert_to_mission, SwarmRuntime, SwarmTaskState};
use crate::steering::{live_turn, SteerOutcome};
use crate::{claude_runner::ClaudeRunner, codex_runner::CodexRunner};
use nit_core::{
    followups::{Source, Status},
    AgentBusEvent, AppState,
};
use std::{
    collections::HashMap,
    sync::mpsc::{Receiver, TryRecvError},
    time::{Duration, Instant},
};

struct BoundTurn {
    task: String,
    nonce: String,
}
struct Waiting {
    mission: String,
    agent: String,
    receipt: String,
    ack: Receiver<SteerOutcome>,
    since: Instant,
}
#[derive(Default)]
pub(super) struct LiveFollowups {
    turns: HashMap<(String, String), BoundTurn>,
    waiting: Vec<Waiting>,
    terminals: Vec<AgentBusEvent>,
}

impl LiveFollowups {
    pub(super) fn bind(&mut self, mission: &str, task: &str, agent: &str, nonce: String) {
        self.turns.insert(
            (mission.into(), agent.into()),
            BoundTurn {
                task: task.into(),
                nonce,
            },
        );
    }

    pub(super) fn close(&mut self, mission: &str) {
        self.turns.retain(|(mid, _), _| mid != mission);
        self.waiting.retain(|w| w.mission != mission);
    }

    fn awaiting(&self, event: &AgentBusEvent) -> bool {
        let (AgentBusEvent::TurnCompleted {
            agent_id,
            mission_id: Some(mission_id),
            ..
        }
        | AgentBusEvent::TurnFailed {
            agent_id,
            mission_id: Some(mission_id),
            ..
        }) = event
        else {
            return false;
        };
        self.waiting
            .iter()
            .any(|w| w.mission == *mission_id && w.agent == *agent_id)
    }
}

impl SwarmRuntime {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn submit_operator_followup(
        &mut self,
        state: &mut AppState,
        mission: &str,
        target: Option<&str>,
        text: &str,
        source: Option<&Source>,
        queue_only: bool,
        codex: Option<&CodexRunner>,
        claude: Option<&ClaudeRunner>,
    ) -> Result<String, String> {
        let queued = self.queue_operator_followup(state, mission, target, text, source)?;
        if queue_only || codex.is_none() {
            return Ok(queued);
        }
        let inbox = self.followups.get_mut(mission).expect("queued receipt");
        let (receipt, earlier) = inbox.entries.split_last_mut().expect("queued receipt");
        // Preserve receipt order. An earlier explicit queue must not be overtaken
        // by a later live correction for the same task.
        if earlier.iter().any(|r| {
            r.task_id == receipt.task_id
                && matches!(
                    r.status,
                    Status::Pending | Status::Submitted | Status::Steering
                )
        }) {
            return Ok(queued);
        }
        let key = (mission.to_owned(), receipt.agent_id.clone());
        let Some(binding) = self.live_followups.turns.get(&key) else {
            return Ok(queued);
        };
        let run = &self.runs[mission];
        if binding.task != receipt.task_id
            || !run.tasks.iter().any(|t| {
                t.id == binding.task
                    && t.agent_id == receipt.agent_id
                    && t.state == SwarmTaskState::Running
            })
        {
            return Ok(queued);
        }
        // Claude session acknowledgments cannot bind a correction to this DAG turn.
        let Some(turn) = live_turn(
            codex,
            None,
            &receipt.agent_id,
            Some(mission),
            &run.spawn_cwd,
        )
        .filter(|turn| turn.matches_task_turn(&binding.nonce)) else {
            return Ok(queued);
        };
        let mut outbound = receipt.text.clone();
        if let Some(source) = source {
            outbound.push_str(&format!("\nSource: {}", source.path));
            if let Some(rev) = &source.revision {
                outbound.push_str(&format!(" at {rev}"));
            }
            if let Some(hunk) = &source.hunk {
                outbound.push_str(&format!("\n{hunk}"));
            }
        }
        let mut steering = receipt.clone();
        steering.status = Status::Steering;
        steering.detail=Some("Sending to the active task turn; confirmation pending. Inspect before resending after interruption.".into());
        // If this write fails, the durable queued receipt is still valid and
        // no live transport call has occurred.
        if save(&inbox.root, &steering).is_err() {
            return Ok(queued);
        }
        *receipt = steering;
        self.live_followups.waiting.push(Waiting {
            mission: mission.into(),
            agent: receipt.agent_id.clone(),
            receipt: receipt.id.clone(),
            ack: turn.send(codex, claude, receipt.id.clone(), outbound),
            since: Instant::now(),
        });
        Ok(format!(
            "Steering {} ({}) · awaiting acknowledgment · receipt {}",
            receipt.task_id, receipt.agent_id, receipt.id
        ))
    }

    pub(crate) fn defer_terminal_for_live_followup(&mut self, event: &AgentBusEvent) -> bool {
        if self.live_followups.awaiting(event) {
            self.live_followups.terminals.push(event.clone());
            true
        } else {
            false
        }
    }

    pub(crate) fn take_ready_live_terminals(&mut self) -> Vec<AgentBusEvent> {
        let mut ready = Vec::new();
        for event in std::mem::take(&mut self.live_followups.terminals) {
            if self.live_followups.awaiting(&event) {
                self.live_followups.terminals.push(event);
            } else {
                ready.push(event);
            }
        }
        ready
    }

    pub(crate) fn poll_live_followups(&mut self, state: &mut AppState) -> bool {
        let mut changed = false;
        self.live_followups.waiting.retain(|w| {
            let outcome = match w.ack.try_recv() {
                Ok(outcome) => outcome,
                Err(TryRecvError::Empty) if w.since.elapsed() < Duration::from_secs(15) => {
                    return true
                }
                Err(_) => {
                    SteerOutcome::Unknown("Acknowledgment timed out or the transport closed".into())
                }
            };
            changed = true;
            let Some(inbox) = self.followups.get_mut(&w.mission) else {
                return false;
            };
            let Some(r) = inbox.entries.iter_mut().find(|r| r.id == w.receipt) else {
                return false;
            };
            let (status, detail) = acknowledge(outcome);
            transition(&inbox.root, r, status, &detail, state);
            push_system_alert_to_mission(
                state,
                &w.mission,
                format!(
                    "{} ({}) · {detail} · receipt {}",
                    r.task_id, r.agent_id, r.id
                ),
            );
            false
        });
        changed
    }
}

fn acknowledge(outcome: SteerOutcome) -> (Status, String) {
    match outcome {
        SteerOutcome::Accepted => (
            Status::Accepted,
            "Provider accepted the correction for this task turn".into(),
        ),
        SteerOutcome::Stale => (
            Status::Pending,
            "Turn closed before submission; queued for the same task".into(),
        ),
        SteerOutcome::Unavailable(reason) => (
            Status::Pending,
            format!("Live steering unavailable ({reason}); queued for the same task"),
        ),
        SteerOutcome::Unknown(reason) => (
            Status::Unknown,
            format!("Delivery uncertain ({reason}); inspect the turn before recovering this note"),
        ),
        SteerOutcome::Received | SteerOutcome::Completed { .. } => (
            Status::Unknown,
            "Session acknowledgment without task confirmation; inspect before resending".into(),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::swarm::test_fixtures::test_runtime_with_running_tasks;
    use nit_core::{AgentBusEvent, Buffer};
    use std::sync::mpsc;
    const MID: &str = "mis-001";

    fn setup() -> (
        SwarmRuntime,
        AppState,
        tempfile::TempDir,
        mpsc::Sender<SteerOutcome>,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let mut rt =
            test_runtime_with_running_tasks(MID, &[("coder", "review"), ("tester", "test")]);
        rt.set_test_workspace(MID, dir.path());
        let state = AppState::new(
            dir.path().into(),
            Buffer::empty("editor", None),
            Buffer::empty("notes", None),
        );
        rt.queue_operator_followup(&state, MID, Some("task-00"), "Keep this proof", None)
            .unwrap();
        let inbox = rt.followups.get_mut(MID).unwrap();
        let r = &mut inbox.entries[0];
        r.status = Status::Steering;
        save(&inbox.root, r).unwrap();
        let (tx, ack) = mpsc::channel();
        rt.live_followups.waiting.push(Waiting {
            mission: MID.into(),
            agent: "coder".into(),
            receipt: r.id.clone(),
            since: Instant::now(),
            ack,
        });
        (rt, state, dir, tx)
    }

    fn done(agent: &str) -> AgentBusEvent {
        AgentBusEvent::TurnCompleted {
            agent_id: agent.into(),
            mission_id: Some(MID.into()),
            message: "done\n<SWARM_TASK_COMPLETE>".into(),
            thread_id: None,
            token_count: None,
        }
    }

    #[test]
    fn accepted_live_input_releases_only_its_terminal_once_without_a_continuation() {
        let (mut rt, mut state, _dir, tx) = setup();
        assert!(rt.defer_terminal_for_live_followup(&done("coder")));
        assert!(!rt.defer_terminal_for_live_followup(&done("tester")));
        assert!(rt.take_ready_live_terminals().is_empty());
        tx.send(SteerOutcome::Accepted).unwrap();
        assert!(rt.poll_live_followups(&mut state));
        assert_eq!(rt.followups[MID].entries[0].status, Status::Accepted);
        let ready = rt.take_ready_live_terminals();
        assert_eq!(ready.len(), 1);
        assert!(rt.take_ready_live_terminals().is_empty());
        assert!(rt.handle_event(&mut state, &ready[0]).is_empty());
        assert_eq!(rt.followups[MID].entries[0].status, Status::Delivered);
    }

    #[test]
    fn definite_rejection_preserves_fifo_and_reuses_the_original_task() {
        let (mut rt, mut state, _dir, tx) = setup();
        rt.queue_operator_followup(&state, MID, Some("task-00"), "Second note", None)
            .unwrap();
        assert!(rt.defer_terminal_for_live_followup(&done("coder")));
        tx.send(SteerOutcome::Stale).unwrap();
        rt.poll_live_followups(&mut state);
        let event = rt.take_ready_live_terminals().pop().unwrap();
        let out = rt.handle_event(&mut state, &event);
        assert_eq!(out.len(), 1);
        let prompt = &out[0].prompt;
        assert!(prompt.find("Keep this proof").unwrap() < prompt.find("Second note").unwrap());
        assert!(prompt.contains("[nit-task-turn "));
        assert_eq!(out[0].agent_id, "coder");
    }

    #[test]
    fn abort_while_waiting_preserves_uncertain_receipt_without_reopening_the_run() {
        let (mut rt, mut state, dir, tx) = setup();
        assert!(rt.defer_terminal_for_live_followup(&done("coder")));
        let id = rt.followups[MID].entries[0].id.clone();
        rt.abort_mission(&mut state, MID);
        assert!(tx.send(SteerOutcome::Accepted).is_err());
        let event = rt.take_ready_live_terminals().pop().unwrap();
        assert!(rt.handle_event(&mut state, &event).is_empty());
        assert!(rt.mission_was_aborted(MID));
        let bytes = std::fs::read(
            nit_core::followups::directory(dir.path(), MID).join(format!("{id}.json")),
        )
        .unwrap();
        let saved: nit_core::followups::Receipt = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(saved.status, Status::Unknown);
        assert_eq!(saved.text, "Keep this proof");
    }

    #[test]
    fn missing_ack_does_not_replay_even_if_provider_later_accepted() {
        let (mut rt, mut state, dir, tx) = setup();
        assert!(rt.defer_terminal_for_live_followup(&done("coder")));
        rt.live_followups.waiting[0].since = Instant::now() - Duration::from_secs(60);
        rt.poll_live_followups(&mut state);
        assert!(tx.send(SteerOutcome::Accepted).is_err());
        let event = rt.take_ready_live_terminals().pop().unwrap();
        assert!(rt.handle_event(&mut state, &event).is_empty());
        let r = &rt.followups[MID].entries[0];
        let bytes = std::fs::read(
            nit_core::followups::directory(dir.path(), MID).join(format!("{}.json", r.id)),
        )
        .unwrap();
        let saved: nit_core::followups::Receipt = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(saved.status, Status::Unknown);
        assert_eq!(saved.text, "Keep this proof");
    }
}
