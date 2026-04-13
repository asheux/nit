use std::time::{Duration, Instant};

use nit_core::Buffer;

use crate::engine::{HighlightRequest, SyntaxEngine, ViewportRange};
use crate::highlight::{
    map_line_segments_to_chars, EngineKind, HighlightGroup, HighlightSnapshot, HighlightSpan,
    LineSegment, SyntaxStatus,
};
use crate::registry::LanguageId;
use crate::tree_sitter_engine::TreeSitterEngine;

fn make_request(buffer_id: usize, version: u64, lang: LanguageId, text: &str) -> HighlightRequest {
    HighlightRequest {
        buffer_id,
        version,
        language: lang,
        text: text.to_string(),
        edits: Vec::new(),
        full_reparse: true,
        max_spans_per_line: 256,
        viewport: None,
    }
}

fn make_viewport_request(
    buffer_id: usize,
    version: u64,
    lang: LanguageId,
    text: &str,
    viewport: ViewportRange,
) -> HighlightRequest {
    HighlightRequest {
        viewport: Some(viewport),
        ..make_request(buffer_id, version, lang, text)
    }
}

fn poll_snapshot(
    engine: &mut TreeSitterEngine,
    buffer_id: usize,
    version: u64,
    timeout: Duration,
    interval: Duration,
) -> HighlightSnapshot {
    let start = Instant::now();
    loop {
        if let Some(snap) = engine.try_get_highlights(buffer_id, version) {
            return snap;
        }
        if start.elapsed() > timeout {
            panic!("timed out waiting for highlight snapshot ({timeout:?})");
        }
        std::thread::sleep(interval);
    }
}

fn wait_for(engine: &mut TreeSitterEngine, buffer_id: usize, version: u64) -> HighlightSnapshot {
    poll_snapshot(
        engine,
        buffer_id,
        version,
        Duration::from_secs(2),
        Duration::from_millis(10),
    )
}

fn wait_for_long(
    engine: &mut TreeSitterEngine,
    buffer_id: usize,
    version: u64,
) -> HighlightSnapshot {
    poll_snapshot(
        engine,
        buffer_id,
        version,
        Duration::from_secs(5),
        Duration::from_millis(50),
    )
}

fn has_group(snapshot: &HighlightSnapshot, line: usize, group: HighlightGroup) -> bool {
    snapshot
        .per_line
        .get(line)
        .is_some_and(|segs| segs.iter().any(|s| s.group == group))
}

#[test]
fn split_spans_across_lines() {
    let text = "ab\ncd";
    let spans = vec![HighlightSpan {
        start_byte: 0,
        end_byte: 4,
        group: HighlightGroup::Keyword,
        priority: 0,
        modifiers: 0,
    }];
    let snap = HighlightSnapshot::from_spans(
        0,
        1,
        LanguageId::PlainText,
        EngineKind::Plain,
        SyntaxStatus::Ok(EngineKind::Plain),
        text,
        spans,
        32,
    );
    assert_eq!(snap.per_line.len(), 2);
    assert_eq!(snap.per_line[0][0].start, 0);
    assert_eq!(snap.per_line[0][0].end, 3);
    assert_eq!(snap.per_line[1][0].start, 0);
    assert_eq!(snap.per_line[1][0].end, 1);
}

#[test]
fn buffer_edit_points_for_insert() {
    let mut buffer = Buffer::empty("test", None);
    buffer.insert_str("a\nb");
    let edits = buffer.take_pending_edits();
    assert_eq!(edits.len(), 1);
    assert_eq!(edits[0].start_point.row, 0);
    assert_eq!(edits[0].new_end_point.row, 1);
}

#[test]
fn rust_highlights_keywords() {
    let mut engine = TreeSitterEngine::new();
    let req = make_request(1, 1, LanguageId::Rust, "fn main() { let x = 42; }\n");
    engine.schedule_rehighlight(req);
    let snap = wait_for(&mut engine, 1, 1);
    assert!(has_group(&snap, 0, HighlightGroup::Keyword));
    assert!(has_group(&snap, 0, HighlightGroup::Number));
}

#[test]
fn python_highlights_keywords() {
    let mut engine = TreeSitterEngine::new();
    let req = make_request(2, 1, LanguageId::Python, "def foo(x):\n    return x\n");
    engine.schedule_rehighlight(req);
    let snap = wait_for(&mut engine, 2, 1);
    assert!(has_group(&snap, 0, HighlightGroup::Keyword));
}

#[test]
fn javascript_highlights_keywords() {
    let mut engine = TreeSitterEngine::new();
    let req = make_request(
        4,
        1,
        LanguageId::JavaScript,
        "function foo() { return 1; }\n",
    );
    engine.schedule_rehighlight(req);
    let snap = wait_for(&mut engine, 4, 1);
    assert!(has_group(&snap, 0, HighlightGroup::Keyword));
}

#[test]
fn markdown_highlights_heading() {
    let mut engine = TreeSitterEngine::new();
    let req = make_request(3, 1, LanguageId::Markdown, "# Title\n\nText\n");
    engine.schedule_rehighlight(req);
    let snap = wait_for(&mut engine, 3, 1);
    assert!(has_group(&snap, 0, HighlightGroup::Heading));
}

#[test]
fn map_segments_handles_multibyte_and_tabs() {
    let line = "a\té🙂b";
    let start = line.find('é').unwrap();
    let end = start + 'é'.len_utf8() + '🙂'.len_utf8();
    let segments = vec![LineSegment {
        start,
        end,
        group: HighlightGroup::String,
    }];
    let mapped = map_line_segments_to_chars(line, &segments).expect("map segments");
    assert_eq!(mapped.len(), 1);
    assert_eq!(mapped[0].start, 2);
    assert_eq!(mapped[0].end, 4);
}

#[test]
fn map_segments_rejects_mid_char_boundary() {
    let line = "é";
    let segments = vec![LineSegment {
        start: 1,
        end: 2,
        group: HighlightGroup::String,
    }];
    assert!(map_line_segments_to_chars(line, &segments).is_err());
}

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

    assert!(snap.highlighted_range.is_some());
    let (hl_start, hl_end) = snap.highlighted_range.unwrap();
    assert!(hl_start <= 100, "hl_start={hl_start}");
    assert!(hl_end >= 320, "hl_end={hl_end}");

    assert!(
        !snap.per_line[200].is_empty(),
        "viewport line 200 should have spans"
    );
    assert!(
        !snap.per_line[210].is_empty(),
        "viewport line 210 should have spans"
    );

    if hl_start > 0 {
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

    let start = Instant::now();
    loop {
        if let Some(snap) = engine.try_get_highlights(30, 1) {
            if snap.highlighted_range.is_none() {
                assert!(
                    !snap.per_line[350].is_empty(),
                    "line 350 should have spans after progressive fill"
                );
                return;
            }
        }
        if start.elapsed() > Duration::from_secs(5) {
            panic!("progressive fill did not complete within 5 seconds");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn language_change_invalidates_cache() {
    let mut engine = TreeSitterEngine::new();
    let text = "fn main() {}\n";

    engine.schedule_rehighlight(make_request(40, 1, LanguageId::Rust, text));
    let snap1 = wait_for(&mut engine, 40, 1);
    assert_eq!(snap1.language, LanguageId::Rust);

    engine.schedule_rehighlight(make_request(40, 2, LanguageId::Python, text));
    let snap2 = wait_for(&mut engine, 40, 2);
    assert_eq!(snap2.language, LanguageId::Python);
}

#[test]
fn worker_recovers_from_error() {
    let mut engine = TreeSitterEngine::new();

    engine.schedule_rehighlight(make_request(50, 1, LanguageId::PlainText, "hello\n"));

    let start = Instant::now();
    loop {
        if let Some(snap) = engine.try_get_highlights(50, 1) {
            assert_eq!(snap.buffer_id, 50);
            break;
        }
        if start.elapsed() > Duration::from_millis(500) {
            panic!("worker did not respond");
        }
        std::thread::sleep(Duration::from_millis(10));
    }

    engine.schedule_rehighlight(make_request(51, 1, LanguageId::Rust, "let x = 1;\n"));
    let snap = wait_for(&mut engine, 51, 1);
    assert!(has_group(&snap, 0, HighlightGroup::Keyword));
}

#[test]
fn highlighted_range_none_for_eager_mode() {
    let mut engine = TreeSitterEngine::new();
    let req = make_request(60, 1, LanguageId::Rust, "fn main() {}\n");
    engine.schedule_rehighlight(req);
    let snap = wait_for(&mut engine, 60, 1);
    assert!(
        snap.highlighted_range.is_none(),
        "eager mode should have highlighted_range = None"
    );
}
