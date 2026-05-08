//! Tests for the history log serialization format.

use crate::history_log::MatchHistory;

const FIXTURE_ROUND_COUNT: u32 = 4;

// Excludes optional fields (`cycle`, `a_tm_metrics`, `b_tm_metrics`); the
// `none_optional_fields_are_omitted_from_json` test confirms that omission.
const EXPECTED_FIELD_COUNT: usize = 10;

const ALL_OUTCOMES_SEQUENCE: &str = "0123";

fn baseline_history_fixture() -> MatchHistory {
    MatchHistory {
        match_id: 7,
        match_index: 8,
        total_matches: 12,
        a: "fsm_alpha".into(),
        b: "fsm_beta".into(),
        repetition: 1,
        rounds: FIXTURE_ROUND_COUNT,
        score_idx: ALL_OUTCOMES_SEQUENCE.into(),
        a_score: -6,
        b_score: -2,
        cycle: None,
        a_tm_metrics: None,
        b_tm_metrics: None,
    }
}

fn serialise(record: &MatchHistory) -> String {
    serde_json::to_string(record).expect("serialize history record")
}

#[test]
fn compact_payload_contains_expected_fields() {
    let json = serialise(&baseline_history_fixture());

    for fragment in [
        r#""score_idx":"0123""#,
        r#""a":"fsm_alpha""#,
        r#""b":"fsm_beta""#,
    ] {
        assert!(json.contains(fragment), "missing fragment: {fragment}");
    }

    // Legacy per-move arrays must not appear in the compact format, and
    // event-stream-only fields must not leak into the history log.
    for forbidden in ["a_moves", "b_moves", "timestamp", "event"] {
        assert!(
            !json.contains(forbidden),
            "compact json leaked: {forbidden}"
        );
    }
}

#[test]
fn none_optional_fields_are_omitted_from_json() {
    let json = serialise(&baseline_history_fixture());

    for skipped in ["\"cycle\"", "\"a_tm_metrics\"", "\"b_tm_metrics\""] {
        assert!(
            !json.contains(skipped),
            "{skipped} should be skipped when None"
        );
    }
}

#[test]
fn single_round_fixture_serialises_correctly() {
    let record = MatchHistory {
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
        ..baseline_history_fixture()
    };

    let json = serialise(&record);
    assert!(json.contains(r#""a":"one""#));
    assert!(json.contains(r#""b":"two""#));
    assert!(json.contains(r#""rounds":1"#));
    assert_eq!(record.score_idx.len(), 1);
}

#[test]
fn serialized_field_count() {
    let record = baseline_history_fixture();
    let val: serde_json::Value = serde_json::to_value(&record).expect("serialize to value");
    let obj = val.as_object().expect("should be JSON object");
    assert_eq!(
        obj.len(),
        EXPECTED_FIELD_COUNT,
        "expected exactly {EXPECTED_FIELD_COUNT} fields, got {}",
        obj.len()
    );
}

#[test]
fn score_idx_accepts_outcomes_alias_on_deserialise() {
    // Older log files use `outcomes` for what is now `score_idx`; the
    // deserialiser must continue to accept both spellings.
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

    let deserialised: MatchHistory =
        serde_json::from_str(legacy_json_payload).expect("deserialise with outcomes alias");
    assert_eq!(deserialised.score_idx, "012");
}
