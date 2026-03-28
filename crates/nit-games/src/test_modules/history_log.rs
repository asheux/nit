use super::MatchHistory;

#[test]
fn match_history_serializes_compact_payload() {
    let record = MatchHistory {
        match_id: 7,
        match_index: 8,
        total_matches: 12,
        a: "fsm_a".into(),
        b: "fsm_b".into(),
        repetition: 1,
        rounds: 4,
        score_idx: "0123".into(),
        a_score: -6,
        b_score: -2,
        cycle: None,
        a_tm_metrics: None,
        b_tm_metrics: None,
    };

    let json = serde_json::to_string(&record).expect("serialize compact history");
    assert!(json.contains("\"score_idx\":\"0123\""));
    assert!(json.contains("\"a\":\"fsm_a\""));
    assert!(json.contains("\"b\":\"fsm_b\""));
    assert!(!json.contains("a_moves"));
    assert!(!json.contains("b_moves"));
    assert!(!json.contains("timestamp"));
    assert!(!json.contains("event"));
}
