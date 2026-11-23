use super::*;
use nit_core::{
    state::{AgentLane, AgentLaneKind, AgentStatus, AgentTurnState, TurnOutcome},
    Buffer,
};
use std::{sync::mpsc, time::Instant};

struct Desk {
    dir: tempfile::TempDir,
    state: AppState,
    pending: PendingSteers,
    vitals: VitalsState,
    tx: mpsc::Sender<SteerOutcome>,
    id: String,
}

impl Desk {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let mut state = AppState::new(
            dir.path().into(),
            Buffer::empty("source", None),
            Buffer::empty("notes", None),
        );
        state.agents.messages.clear();
        state.agents.agents = vec![AgentLane {
            id: "agent".into(),
            role: "coder".into(),
            lane: "Codex".into(),
            kind: AgentLaneKind::Codex,
            status: AgentStatus::Running,
            heartbeat_age_secs: 0,
            queue_len: 0,
            current_mission: None,
            last_message: String::new(),
            shadow: false,
        }];
        let now = Instant::now();
        state.agents.active_turns.insert(
            "agent".into(),
            AgentTurnState {
                started_at: now,
                last_output_at: now,
                last_heartbeat_at: now,
                stage: None,
            },
        );
        state.agents.codex_turn_prompt_idx.insert("agent".into(), 0);
        chat_input::record_chat_message(&mut state, AgentChannel::Agent, "original", "original");
        let mut pending = PendingSteers::default();
        let mut vitals = VitalsState::new(4);
        assert!(submit(
            &mut state,
            &mut vitals,
            None,
            None,
            &mut pending,
            "agent",
            None,
            "first",
            "first",
            true
        ));
        // Inject the provider acknowledgment channel after creating the real on-disk receipt.
        state.agents.queued_codex_turns.clear();
        state.agents.agents[0].queue_len = 0;
        let (tx, rx) = mpsc::channel();
        pending.0[0].reply = Some(rx);
        pending.0[0].transition(&mut state, Status::Steering, "Sending to the active turn");
        let id = pending.0[0].note.id.clone();
        Self {
            dir,
            state,
            pending,
            vitals,
            tx,
            id,
        }
    }
    fn poll(&mut self) {
        let (codex, _) = CodexRunner::test_channel();
        let (claude, _) = ClaudeRunner::test_channel();
        self.pending
            .poll(&mut self.state, &mut self.vitals, &codex, &claude);
    }
    fn terminal(&mut self, ok: bool) {
        let event = if ok {
            AgentBusEvent::TurnCompleted {
                agent_id: "agent".into(),
                mission_id: None,
                thread_id: None,
                token_count: None,
                message: "original answer".into(),
            }
        } else {
            AgentBusEvent::TurnFailed {
                agent_id: "agent".into(),
                mission_id: None,
                thread_id: None,
                token_count: None,
                message: "original failed".into(),
            }
        };
        self.pending.event(&mut self.state, &event);
    }
    fn note(&self) -> Receipt {
        followups::find(self.dir.path(), &self.id).unwrap()
    }
}

#[test]
fn codex_acceptance_and_parent_terminal_commute() {
    for terminal_first in [true, false] {
        for success in [true, false] {
            let mut desk = Desk::new();
            if terminal_first {
                desk.terminal(success);
            }
            desk.tx.send(SteerOutcome::Accepted).unwrap();
            desk.poll();
            if !terminal_first {
                assert_eq!(desk.note().status, Status::Accepted);
                desk.terminal(success);
            }
            assert_eq!(
                desk.note().status,
                if success {
                    Status::Delivered
                } else {
                    Status::Failed
                }
            );
            assert_eq!(
                desk.note().detail.as_deref(),
                Some(if success {
                    "Original turn result: original answer"
                } else {
                    "Original turn result: original failed"
                })
            );
            assert!(desk.state.agents.queued_codex_turns.is_empty());
        }
    }
}

#[test]
fn claude_echo_remains_received_after_parent_failure_until_its_own_result() {
    for failed in [false, true] {
        let mut desk = Desk::new();
        desk.tx.send(SteerOutcome::Received).unwrap();
        desk.poll();
        desk.terminal(false);
        assert_eq!(desk.note().status, Status::Received);
        desk.tx
            .send(SteerOutcome::Completed {
                message: "specific followup result".into(),
                failed,
            })
            .unwrap();
        desk.poll();
        let note = desk.note();
        assert_eq!(
            note.status,
            if failed {
                Status::Failed
            } else {
                Status::Delivered
            }
        );
        assert_eq!(note.detail.as_deref(), Some("specific followup result"));
        let message = desk.state.agents.messages.last().unwrap();
        assert_eq!(message.prompt_msg_idx, Some(1));
        assert_eq!(
            message.turn.unwrap().outcome,
            if failed {
                TurnOutcome::Failed
            } else {
                TurnOutcome::Completed
            }
        );
    }
}

#[test]
fn uncertain_delivery_and_context_change_never_replay() {
    for stale in [true, false] {
        let mut desk = Desk::new();
        if stale {
            desk.state.workspace_root = desk.dir.path().join("different");
            desk.tx.send(SteerOutcome::Stale).unwrap();
        } else {
            desk.tx
                .send(SteerOutcome::Unknown("connection lost".into()))
                .unwrap();
        }
        desk.poll();
        desk.poll();
        assert_eq!(
            desk.note().status,
            if stale {
                Status::Failed
            } else {
                Status::Unknown
            }
        );
        assert!(desk.state.agents.queued_codex_turns.is_empty());
    }
}

#[test]
fn stale_live_note_precedes_later_explicit_queue_entries() {
    let mut desk = Desk::new();
    for text in ["second", "third"] {
        assert!(submit(
            &mut desk.state,
            &mut desk.vitals,
            None,
            None,
            &mut desk.pending,
            "agent",
            None,
            text,
            text,
            true
        ));
    }
    desk.tx.send(SteerOutcome::Stale).unwrap();
    desk.poll();
    desk.poll();
    assert_eq!(desk.note().status, Status::Pending);
    assert_eq!(
        desk.state
            .agents
            .queued_codex_turns
            .iter()
            .map(|q| q.prompt.as_str())
            .collect::<Vec<_>>(),
        ["first", "second", "third"]
    );
}

#[test]
fn a_different_turn_start_blocks_late_fallback_even_after_lane_becomes_idle() {
    let mut desk = Desk::new();
    desk.state
        .agents
        .codex_turn_prompt_idx
        .insert("agent".into(), 7);
    desk.pending.event(
        &mut desk.state,
        &AgentBusEvent::TurnStarted {
            agent_id: "agent".into(),
            mission_id: Some("other mission".into()),
            resume_thread_id: None,
        },
    );
    desk.state.agents.active_turns.clear();
    desk.state.agents.codex_turn_prompt_idx.clear();
    desk.tx.send(SteerOutcome::Stale).unwrap();
    desk.poll();
    assert_eq!(desk.note().status, Status::Failed);
    assert!(desk.state.agents.queued_codex_turns.is_empty());
}
