//! Heartbeat-only batch coalescer, complementary to the runner's
//! frame-rate cap. Collapsing earlier `TurnHeartbeat` events cannot
//! regress observable state because the bus handler just refreshes
//! `last_heartbeat_at`, so only the most recent heartbeat matters for
//! the liveness reaper. Every other variant is preserved verbatim.

use std::collections::{HashMap, HashSet};

use nit_core::AgentBusEvent;

/// Drop superseded `TurnHeartbeat` events from a per-tick batch in
/// place, keeping the latest heartbeat per `agent_id` at its original
/// position so it cannot jump past a lifecycle anchor that arrived
/// after the earliest one. Returns the number of dropped events.
pub(crate) fn coalesce_heartbeats(events: &mut Vec<AgentBusEvent>) -> usize {
    if events.len() < 2 {
        return 0;
    }
    let mut last_idx_for_agent: HashMap<&str, usize> = HashMap::new();
    let mut total_heartbeats = 0usize;
    for (idx, event) in events.iter().enumerate() {
        if let AgentBusEvent::TurnHeartbeat { agent_id, .. } = event {
            last_idx_for_agent.insert(agent_id.as_str(), idx);
            total_heartbeats += 1;
        }
    }
    if total_heartbeats < 2 {
        return 0;
    }
    let keep: HashSet<usize> = last_idx_for_agent.into_values().collect();
    let mut idx = 0usize;
    let dropped_before = events.len();
    events.retain(|event| {
        let here = idx;
        idx += 1;
        match event {
            AgentBusEvent::TurnHeartbeat { .. } => keep.contains(&here),
            _ => true,
        }
    });
    dropped_before - events.len()
}
