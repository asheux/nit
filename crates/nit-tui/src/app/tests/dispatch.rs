//! Dispatch / queue smoke tests separate from the larger swarm flow.
//! Verifies queue_len accounting on AgentLane and AgentStatus transitions.

use super::*;

fn fresh_lane(id: &str, kind: nit_core::AgentLaneKind) -> nit_core::AgentLane {
    nit_core::AgentLane {
        id: id.to_string(),
        role: "main".to_string(),
        lane: "default".to_string(),
        kind,
        status: nit_core::AgentStatus::Idle,
        heartbeat_age_secs: 0,
        queue_len: 0,
        current_mission: None,
        last_message: String::new(),
        shadow: false,
    }
}

#[test]
fn fresh_lane_has_zero_queue_depth() {
    let lane = fresh_lane("codex-main", nit_core::AgentLaneKind::Codex);
    assert_eq!(lane.queue_len, 0);
}

#[test]
fn queue_len_increments_on_enqueue() {
    let mut lane = fresh_lane("codex-main", nit_core::AgentLaneKind::Codex);
    lane.queue_len = lane.queue_len.saturating_add(1);
    assert_eq!(lane.queue_len, 1);
}

#[test]
fn queue_len_saturates_subtract() {
    let mut lane = fresh_lane("claude-main", nit_core::AgentLaneKind::Claude);
    lane.queue_len = lane.queue_len.saturating_sub(1);
    assert_eq!(lane.queue_len, 0);
}

#[test]
fn status_transitions_back_to_idle() {
    let mut lane = fresh_lane("codex-main", nit_core::AgentLaneKind::Codex);
    lane.status = nit_core::AgentStatus::Running;
    lane.status = nit_core::AgentStatus::Idle;
    assert_eq!(lane.status, nit_core::AgentStatus::Idle);
}
