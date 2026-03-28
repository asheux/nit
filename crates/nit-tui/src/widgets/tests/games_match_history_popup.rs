use super::*;
use crate::theme::Theme;
use ratatui::layout::Rect;
use ratatui::text::Line;

fn line_text(line: &Line<'_>) -> String {
    line.spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect()
}

#[test]
fn max_round_limit_caps_preview_at_500() {
    let entries = vec![nit_games::MatchHistoryPreview {
        match_index: 1,
        total_matches: 1,
        a: "0".into(),
        b: "867".into(),
        rounds_total: 700,
        outcomes: "2".repeat(700),
    }];

    assert_eq!(
        max_round_limit(&entries),
        nit_games::MatchHistoryPreview::DISPLAY_ROUND_CAP
    );
    assert_eq!(
        entries[0].preview_outcomes().len(),
        nit_games::MatchHistoryPreview::DISPLAY_ROUND_CAP
    );
}

#[test]
fn empty_history_popup_explains_disabled_batch_capture() {
    let mut state = AppState::new(
        std::env::temp_dir(),
        nit_core::Buffer::empty("x", None),
        nit_core::Buffer::empty("n", None),
    );
    state.games.match_history.capture_disabled_for_run = true;
    state.games.status = nit_core::GamesStatus::Running;
    state.games.runtime.backend = nit_games::RuntimeAcceleratorBackend::Metal;
    state.games.runtime.metal_matches = 4096;

    let lines = build_lines(
        &state,
        &Theme::default(),
        Rect {
            x: 0,
            y: 0,
            width: 100,
            height: 30,
        },
    );

    assert!(lines.iter().any(|line| {
        line_text(line).contains("Detailed history previews are disabled during batch/GPU runs")
    }));
    assert!(lines
        .iter()
        .any(|line| { line_text(line).contains("live round tiles are unavailable here") }));
    assert!(lines
        .iter()
        .any(|line| line_text(line).contains("Metal batching active")));
}
