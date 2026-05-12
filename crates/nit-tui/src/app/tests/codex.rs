//! Codex backend smoke tests separate from the larger mission lifecycle.
//! Verifies AgentLaneKind variants and lane construction.

use super::*;

#[test]
fn lane_kind_codex_is_distinct_from_claude() {
    assert_ne!(
        nit_core::AgentLaneKind::Codex,
        nit_core::AgentLaneKind::Claude
    );
}

#[test]
fn lane_kind_default_is_unknown() {
    let kind: nit_core::AgentLaneKind = Default::default();
    assert_eq!(kind, nit_core::AgentLaneKind::Unknown);
}

#[test]
fn lane_kind_round_trips_through_serde() {
    let value = nit_core::AgentLaneKind::Codex;
    let json = serde_json::to_string(&value).expect("serialize");
    let back: nit_core::AgentLaneKind = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(back, value);
}

#[test]
fn lane_kind_unknown_serialises_as_snake_case() {
    let value = nit_core::AgentLaneKind::Unknown;
    let json = serde_json::to_string(&value).expect("serialize");
    assert_eq!(json, "\"unknown\"");
}
