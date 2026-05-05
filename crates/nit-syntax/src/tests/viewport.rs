use crate::engine::tree_sitter::TreeSitterEngine;
use crate::{EngineKind, LanguageId, SyntaxEngine, SyntaxStatus, ViewportRange};

use super::{make_viewport_request, poll_until, wait_for, wait_for_long, LONG_TIMEOUT};

#[test]
fn viewport_scoped_highlight_produces_spans_for_visible_range() {
    let mut engine = TreeSitterEngine::new();
    let lines: Vec<String> = (0..500).map(|i| format!("let x{i} = {i};\n")).collect();
    let text = lines.join("");

    let req = make_viewport_request(
        10,
        1,
        LanguageId::Rust,
        &text,
        ViewportRange {
            first_line: 200,
            last_line: 220,
            total_lines: 500,
        },
    );
    engine.schedule_rehighlight(req);
    let snap = wait_for(&mut engine, 10, 1);

    // None means progressive fill already completed (entire file covered).
    if let Some((hl_start, hl_end)) = snap.highlighted_range {
        assert!(hl_start <= 100, "hl_start={hl_start}");
        assert!(hl_end >= 320, "hl_end={hl_end}");
    }

    assert!(
        !snap.per_line[200].is_empty(),
        "viewport line 200 should have spans"
    );
    assert!(
        !snap.per_line[210].is_empty(),
        "viewport line 210 should have spans"
    );

    let viewport_excludes_start = matches!(snap.highlighted_range, Some((s, _)) if s > 0);
    if viewport_excludes_start {
        assert_eq!(
            snap.line_hashes[0], 0,
            "line 0 should have sentinel hash when outside initial range"
        );
    }
}

#[test]
fn large_file_parses_without_fallback() {
    let mut engine = TreeSitterEngine::new();
    let line = "let variable_name = \"hello world\";\n";
    let repeat_count = (5_000_000 / line.len()) + 1;
    let text: String = line.repeat(repeat_count);
    assert!(text.len() > 5_000_000);

    let req = make_viewport_request(
        20,
        1,
        LanguageId::Rust,
        &text,
        ViewportRange {
            first_line: 0,
            last_line: 50,
            total_lines: repeat_count,
        },
    );
    engine.schedule_rehighlight(req);
    let snap = wait_for_long(&mut engine, 20, 1);

    assert_eq!(snap.status, SyntaxStatus::Ok(EngineKind::TreeSitter));
    assert!(!snap.per_line[0].is_empty(), "first line should have spans");
}

#[test]
fn progressive_fill_covers_full_file() {
    let mut engine = TreeSitterEngine::new();
    let lines: Vec<String> = (0..400).map(|i| format!("let x{i} = {i};\n")).collect();
    let text = lines.join("");

    let req = make_viewport_request(
        30,
        1,
        LanguageId::Rust,
        &text,
        ViewportRange {
            first_line: 0,
            last_line: 30,
            total_lines: 400,
        },
    );
    engine.schedule_rehighlight(req);

    let snap = poll_until(
        &mut engine,
        30,
        1,
        |s| s.highlighted_range.is_none(),
        LONG_TIMEOUT,
    );
    assert!(
        !snap.per_line[350].is_empty(),
        "line 350 should have spans after progressive fill"
    );
}

#[test]
fn stale_viewport_first_line_past_total_does_not_panic() {
    // Bug #2 regression: an operator can carry a ViewportRange over
    // from a previous, larger buffer when files swap. The pre-fix
    // viewport_highlight indexed `offsets[start_line]` unguarded and
    // panicked; the worker would catch_unwind and drop BufferState
    // every frame, defeating incremental highlight. The fix in
    // engine/tree_sitter/modes.rs clamps start_line to the buffer's
    // last valid line.
    let mut engine = TreeSitterEngine::new();
    let req = make_viewport_request(
        70,
        1,
        LanguageId::Rust,
        "fn main() {}\n",
        ViewportRange {
            first_line: 200,
            last_line: 220,
            total_lines: 500,
        },
    );
    engine.schedule_rehighlight(req);
    let snap = wait_for(&mut engine, 70, 1);

    assert_eq!(
        snap.status,
        SyntaxStatus::Ok(EngineKind::TreeSitter),
        "stale-viewport request must produce a healthy snapshot, got {:?}",
        snap.status,
    );
    assert!(
        !snap.per_line[0].is_empty(),
        "buffer's single line should still receive spans after clamping"
    );
}

#[test]
fn one_line_buffer_with_viewport_first_line_above_zero() {
    // Edge case: viewport.first_line > 0 against a 1-line buffer. The
    // last_line_idx clamp in viewport_highlight reduces start_line to 0
    // and end_line to 0; the snapshot must come back Ok with the line
    // populated.
    let mut engine = TreeSitterEngine::new();
    let req = make_viewport_request(
        80,
        1,
        LanguageId::Rust,
        "fn main() {}\n",
        ViewportRange {
            first_line: 1,
            last_line: 1,
            total_lines: 1,
        },
    );
    engine.schedule_rehighlight(req);
    let snap = wait_for(&mut engine, 80, 1);

    assert_eq!(snap.status, SyntaxStatus::Ok(EngineKind::TreeSitter));
    assert!(!snap.per_line[0].is_empty());
}
