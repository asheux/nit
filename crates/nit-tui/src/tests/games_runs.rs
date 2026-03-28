use super::{build_replay, ReplayData};
use nit_games::game::PayoffMatrix;
use nit_games::history_log::MatchHistory;

fn compact_history(score_idx: &str) -> MatchHistory {
    MatchHistory {
        match_id: 7,
        match_index: 8,
        total_matches: 12,
        a: "all_c".into(),
        b: "all_d".into(),
        repetition: 1,
        rounds: score_idx.len() as u32,
        score_idx: score_idx.into(),
        a_score: -6,
        b_score: -2,
        cycle: None,
        a_tm_metrics: None,
        b_tm_metrics: None,
    }
}

#[test]
fn build_replay_reconstructs_actions_from_compact_history() {
    let ReplayData { title, lines, .. } =
        build_replay(compact_history("0123"), PayoffMatrix::default_pd());

    assert!(title.contains("all_c vs all_d"));
    assert!(lines.iter().any(|line| line.contains("   1  C  C   CC")));
    assert!(lines.iter().any(|line| line.contains("   2  C  D   CD")));
    assert!(lines.iter().any(|line| line.contains("   3  D  C   DC")));
    assert!(lines.iter().any(|line| line.contains("   4  D  D   DD")));
}
