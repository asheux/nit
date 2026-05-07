//! Shared `#[cfg(test)]` fixtures for the centralized files in
//! `crates/nit-core/src/tests/`.

#![allow(dead_code)]

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use crate::buffer::Buffer;
use crate::state::{AgentLane, AgentLaneKind, AgentStatus, AgentTurnState, AppState};
use crate::substrate::{Signal, SignalKind, SignalTarget, SubstrateState};

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

pub(crate) fn temp_dir(label: &str) -> PathBuf {
    let pid = std::process::id();
    let n = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("nit-core-test-{label}-{pid}-{n}"));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

pub(crate) fn test_state() -> AppState {
    let editor = Buffer::from_str("editor", "", None);
    let notes = Buffer::from_str("notes", "", None);
    AppState::new(PathBuf::from("."), editor, notes)
}

pub(crate) fn test_state_in(root: PathBuf) -> AppState {
    let editor = Buffer::from_str("editor", "", None);
    let notes = Buffer::from_str("notes", "", None);
    AppState::new(root, editor, notes)
}

pub(crate) fn add_codex_agent(state: &mut AppState, id: &str) {
    add_lane(state, id, AgentLaneKind::Codex, "Codex");
}

pub(crate) fn add_claude_agent(state: &mut AppState, id: &str) {
    add_lane(state, id, AgentLaneKind::Claude, "Claude");
}

fn add_lane(state: &mut AppState, id: &str, kind: AgentLaneKind, lane_label: &str) {
    state.agents.agents.push(AgentLane {
        id: id.into(),
        role: id.into(),
        lane: lane_label.into(),
        kind,
        status: AgentStatus::Running,
        heartbeat_age_secs: 0,
        queue_len: 1,
        current_mission: None,
        last_message: String::new(),
        shadow: false,
    });
    let now = Instant::now();
    state.agents.active_turns.insert(
        id.into(),
        AgentTurnState {
            started_at: now,
            last_heartbeat_at: now,
            last_output_at: now,
            stage: None,
        },
    );
}

pub(crate) fn inject_warning(
    state: &mut AppState,
    posted_by: &str,
    posted_at_gen: u64,
    counter: u64,
) {
    let id = format!("{posted_at_gen}-{posted_by}-{counter}");
    state.substrate.emit_signal(Signal {
        id,
        kind: SignalKind::Warning,
        posted_by: posted_by.into(),
        posted_at_gen,
        target: SignalTarget::Agent {
            agent_id: posted_by.into(),
        },
        initial_strength: SubstrateState::DEFAULT_INITIAL_STRENGTH,
        payload: serde_json::Value::Null,
    });
}
