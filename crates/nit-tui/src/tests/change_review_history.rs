use super::*;
use nit_core::{state::ReviewCommit, Buffer};
use ratatui::{backend::TestBackend, Terminal};

fn fixture() -> AppState {
    let mut state = AppState::new(
        ".".into(),
        Buffer::empty("e", None),
        Buffer::empty("n", None),
    );
    let r = &mut state.change_review;
    r.branch = "main".into();
    r.upstream = Some("origin/main".into());
    r.ahead = 2;
    r.behind = 1;
    for i in 0..9 {
        r.commits.push(ReviewCommit {
            id: format!("abc123{i}0123456789"),
            subject: format!("Commit {i}"),
            author: "Ada".into(),
            date: "2026-11-23".into(),
        });
    }
    r.preview_commit = Some(r.commits[0].id.clone());
    r.preview = (0..40).map(|i| format!("+patch line {i}")).collect();
    state
}

fn screen(state: &AppState, width: u16, height: u16) -> String {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal
        .draw(|frame| super::super::render(frame, frame.size(), state, &Theme::default()))
        .unwrap();
    let buffer = terminal.backend().buffer();
    (0..height)
        .map(|y| {
            (0..width)
                .map(|x| buffer.get(x, y).symbol())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn clean_history_shows_tracking_selection_and_commit_patch() {
    let mut state = fixture();
    for width in [35, 70, 100] {
        let output = screen(&state, width, 17);
        for label in [
            "main",
            "Working tree clean",
            "origin/main",
            "2 ahead · 1 behind",
            "Recent commits",
            "abc1230",
            "Commit 0",
            "Patch · 2026-11-23 · Ada",
            "+patch line 0",
            "Open",
        ] {
            assert!(output.contains(label), "{label}: {output}");
        }
    }
    let area = Rect::new(8, 2, 40, 14);
    let r = layout(area, &state.change_review);
    mouse_select(
        &mut state.change_review,
        area,
        Position::new(r.commits.x + 1, r.commits.y + 1),
    );
    assert_eq!(state.change_review.selected, 1);
    let output = screen(&state, 35, 14);
    assert!(output.contains("Loading commit") && !output.contains("+patch"));
    mouse_scroll(
        &mut state.change_review,
        area,
        Position::new(r.commits.x, r.commits.y),
        1,
    );
    assert_eq!(state.change_review.selected, 2);
    state.change_review.preview_commit = Some(state.change_review.commits[2].id.clone());
    mouse_select(
        &mut state.change_review,
        area,
        Position::new(r.footer.x, r.footer.y),
    );
    let output = screen(&state, 35, 14);
    assert!(
        state.change_review.history_patch_open
            && output.contains("Commit 2")
            && output.contains("2026-11-23")
            && output.contains("‹ Back")
    );
    assert!(!output.contains("Recent commits"));
    let patch = layout(area, &state.change_review).patch;
    mouse_scroll(
        &mut state.change_review,
        area,
        Position::new(patch.x, patch.y),
        3,
    );
    assert_eq!(state.change_review.scroll, 3);
    mouse_select(
        &mut state.change_review,
        area,
        Position::new(r.footer.x, r.footer.y),
    );
    assert!(!state.change_review.history_patch_open);
}

#[test]
fn unborn_detached_and_failed_status_remain_distinct_in_compact_layout() {
    let mut state = fixture();
    state.change_review.detached = true;
    assert!(screen(&state, 35, 12).contains("Detached HEAD"));
    state.change_review.detached = false;
    state.change_review.upstream_gone = true;
    assert!(screen(&state, 35, 12).contains("Upstream gone · origin/main"));
    state.change_review.commits.clear();
    state.change_review.unborn = true;
    state.change_review.upstream = None;
    let output = screen(&state, 35, 12);
    assert!(output.contains("No commits yet") && !output.contains("Recent commits"));
    state.change_review.message = Some("No readable Git worktree here".into());
    let output = screen(&state, 35, 12);
    assert!(output.contains("No readable Git") && !output.contains("No commits yet"));
    for (width, height) in [(2, 2), (5, 3), (20, 6)] {
        screen(&fixture(), width, height);
    }
}

#[test]
fn dirty_title_retains_porcelain_tracking_context() {
    let mut state = fixture();
    state
        .change_review
        .files
        .push(nit_core::state::ChangedFile {
            path: "main.rs".into(),
            index: 'M',
            worktree: ' ',
        });
    let output = screen(&state, 70, 15);
    assert!(output.contains("main...origin/main [ahead 2, behind 1]"));
    assert!(!output.contains("Recent commits"));
}

#[test]
fn git_colors_keep_patch_headers_distinct_from_source_and_indices() {
    let theme = Theme::default();
    for (text, in_hunk, color) in [
        ("diff --git a/src.rs b/src.rs", false, theme.hl.r#type),
        ("index 123..abc", false, theme.muted),
        ("--- a/src.rs", false, theme.hl.r#type),
        ("+++ b/src.rs", false, theme.hl.r#type),
        ("+++ source begins with ++", true, theme.diff_added),
        ("--- source begins with --", true, theme.diff_deleted),
        ("Merge conflict", false, theme.error),
    ] {
        let line = patch_line(text, in_hunk, &theme);
        assert_eq!(line.to_string(), text);
        assert_eq!(line.style.fg, Some(color));
    }
    let range = patch_line("@@ -12,2 +13,5 @@ fn sample()", true, &theme);
    assert_eq!(range.spans[0].style.fg, Some(theme.hl.number));
    assert_eq!(range.spans[1].style.fg, Some(theme.muted));
    assert_eq!(status_color('M', true, false, &theme), theme.success);
    assert_eq!(status_color('M', false, false, &theme), theme.diff_modified);
    assert_eq!(status_color('U', true, true, &theme), theme.error);
}

#[test]
fn history_list_uses_roster_surface_and_patch_uses_main_canvas() {
    let state = fixture();
    let theme = Theme {
        background: Color::Rgb(9, 19, 29),
        editor_background: Color::Rgb(70, 81, 92),
        ..Theme::default()
    };
    let mut term = Terminal::new(TestBackend::new(70, 17)).unwrap();
    term.draw(|f| super::super::render(f, f.size(), &state, &theme))
        .unwrap();
    let buf = term.backend().buffer();
    let r = layout(buf.area, &state.change_review);
    for row in [r.summary.y, r.tracking.y, r.heading.y, r.commits.y + 1] {
        assert_eq!(
            buf.get(r.commits.right() - 1, row).bg,
            theme.list_background()
        );
    }
    assert_eq!(buf.get(r.commits.x, r.commits.y).bg, theme.selection_bg);
    for area in [r.detail, r.patch] {
        for y in area.y..area.bottom() {
            for x in area.x..area.right() {
                assert_eq!(buf.get(x, y).bg, theme.background);
            }
        }
    }
    assert_eq!(buf.get(r.footer.x, r.footer.y).bg, theme.background);
}
