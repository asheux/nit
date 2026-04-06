//! Tests for the history log serialisation format.
//!
//! Verifies that [`MatchHistory`] serialises to a compact JSON payload
//! containing only the fields defined in the schema, with no legacy
//! or internal-only fields leaking into the output.

use super::MatchHistory;

// ── Constants ─────────────────────────────────────────────────────────────

/// Number of rounds in the baseline fixture match.
const FIXTURE_ROUND_COUNT: usize = 4;

/// Expected field count in the compact JSON format (excluding optional fields).
const COMPACT_REQUIRED_FIELDS: usize = 10;

// ── Fixtures ──────────────────────────────────────────────────────────────

/// Outcome index string covering all four joint outcomes in order.
const ALL_OUTCOMES_SEQUENCE: &str = "0123";

/// Build a [`MatchHistory`] fixture with sensible defaults for testing.
///
/// The returned record represents a 4-round match between two FSM strategies
/// with no cycle detection and no Turing-machine metrics attached.
fn baseline_history_fixture() -> MatchHistory {
    MatchHistory {
        match_id: 7,
        match_index: 8,
        total_matches: 12,
        a: "fsm_alpha".into(),
        b: "fsm_beta".into(),
        repetition: 1,
        rounds: FIXTURE_ROUND_COUNT as u32,
        score_idx: ALL_OUTCOMES_SEQUENCE.into(),
        a_score: -6,
        b_score: -2,
        cycle: None,
        a_tm_metrics: None,
        b_tm_metrics: None,
    }
}

/// Build a minimal single-round fixture for edge-case testing.
fn single_round_fixture() -> MatchHistory {
    MatchHistory {
        match_id: 0,
        match_index: 1,
        total_matches: 1,
        a: "one".into(),
        b: "two".into(),
        repetition: 0,
        rounds: 1,
        score_idx: "0".into(),
        a_score: -1,
        b_score: -1,
        cycle: None,
        a_tm_metrics: None,
        b_tm_metrics: None,
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────

/// Verify that `MatchHistory` serialises to compact JSON with expected
/// fields present and legacy/internal fields absent.
///
/// The compact format uses `score_idx` (a string of per-round outcome
/// indices) instead of separate `a_moves`/`b_moves` arrays, and omits
/// event-log fields like `timestamp` and `event` that belong to the
/// event stream rather than the history log.
#[test]
fn compact_payload_contains_expected_fields() {
    let compact_history_record = baseline_history_fixture();

    let json_output_string =
        serde_json::to_string(&compact_history_record).expect("serialize compact history");

    // Verify that the core fields are present in the output.
    assert!(json_output_string.contains("\"score_idx\":\"0123\""));
    assert!(json_output_string.contains("\"a\":\"fsm_alpha\""));
    assert!(json_output_string.contains("\"b\":\"fsm_beta\""));

    // Legacy per-move arrays must not appear in the compact format.
    assert!(!json_output_string.contains("a_moves"));
    assert!(!json_output_string.contains("b_moves"));

    // Event-stream-only fields must not leak into the history log.
    assert!(!json_output_string.contains("timestamp"));
    assert!(!json_output_string.contains("event"));
}

/// Verify that `None`-valued optional fields (`cycle`, `a_tm_metrics`,
/// `b_tm_metrics`) are omitted entirely from the serialised JSON thanks to
/// `#[serde(skip_serializing_if = "Option::is_none")]`.
#[test]
fn none_optional_fields_are_omitted_from_json() {
    let history_without_optionals = baseline_history_fixture();

    let json_without_optionals =
        serde_json::to_string(&history_without_optionals).expect("serialize history");

    assert!(
        !json_without_optionals.contains("\"cycle\""),
        "cycle field should be skipped when None"
    );
    assert!(
        !json_without_optionals.contains("\"a_tm_metrics\""),
        "a_tm_metrics field should be skipped when None"
    );
    assert!(
        !json_without_optionals.contains("\"b_tm_metrics\""),
        "b_tm_metrics field should be skipped when None"
    );
}

/// Verify that a single-round fixture serialises without panicking and
/// contains the expected strategy identifiers.
#[test]
fn single_round_fixture_serialises_correctly() {
    let record = single_round_fixture();
    let json = serde_json::to_string(&record).expect("serialize single-round fixture");
    assert!(json.contains("\"a\":\"one\""));
    assert!(json.contains("\"b\":\"two\""));
    assert!(json.contains("\"rounds\":1"));
    assert_eq!(record.score_idx.len(), 1);
}

/// Verify field count in compact format matches expectation.
#[test]
fn compact_format_field_count() {
    let record = baseline_history_fixture();
    let val: serde_json::Value = serde_json::to_value(&record).expect("serialize to value");
    let obj = val.as_object().expect("should be JSON object");
    assert!(
        obj.len() >= COMPACT_REQUIRED_FIELDS,
        "expected at least {COMPACT_REQUIRED_FIELDS} fields, got {}",
        obj.len()
    );
}

/// Verify that the `"outcomes"` alias for `score_idx` is accepted during
/// deserialisation, preserving backwards compatibility with older log files.
#[test]
fn score_idx_accepts_outcomes_alias_on_deserialise() {
    let legacy_json_payload = r#"{
        "match_id": 1,
        "match_index": 0,
        "total_matches": 1,
        "a": "strat_x",
        "b": "strat_y",
        "repetition": 0,
        "rounds": 3,
        "outcomes": "012",
        "a_score": -3,
        "b_score": 0
    }"#;

    let deserialised_record: MatchHistory =
        serde_json::from_str(legacy_json_payload).expect("deserialise with outcomes alias");

    assert_eq!(
        deserialised_record.score_idx, "012",
        "outcomes alias should map to score_idx"
    );
}
