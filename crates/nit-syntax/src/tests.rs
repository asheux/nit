use std::time::{Duration, Instant};

use nit_core::Buffer;

use crate::engine::{HighlightRequest, SyntaxEngine, ViewportRange};
use crate::highlight::{
    map_line_segments_to_chars, HighlightGroup, HighlightSnapshot, HighlightSpan, LineSegment,
};
use crate::registry::LanguageId;
use crate::tree_sitter_engine::TreeSitterEngine;

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
    let snapshot = HighlightSnapshot::from_spans(
        0,
        1,
        LanguageId::PlainText,
        crate::highlight::EngineKind::Plain,
        crate::highlight::SyntaxStatus::Ok(crate::highlight::EngineKind::Plain),
        text,
        spans,
        32,
    );
    assert_eq!(snapshot.per_line.len(), 2);
    assert_eq!(snapshot.per_line[0][0].start, 0);
    assert_eq!(snapshot.per_line[0][0].end, 3);
    assert_eq!(snapshot.per_line[1][0].start, 0);
    assert_eq!(snapshot.per_line[1][0].end, 1);
}

#[test]
fn buffer_edit_points_for_insert() {
    let mut buffer = Buffer::empty("test", None);
    buffer.insert_str("a\nb");
    let edits = buffer.take_pending_edits();
    assert_eq!(edits.len(), 1);
    let edit = &edits[0];
    assert_eq!(edit.start_point.row, 0);
    assert_eq!(edit.new_end_point.row, 1);
}

#[test]
fn rust_highlights_keywords() {
    let mut engine = TreeSitterEngine::new();
    let text = "fn main() { let x = 42; }\n";
    let request = HighlightRequest {
        buffer_id: 1,
        version: 1,
        language: LanguageId::Rust,
        text: text.to_string(),
        edits: Vec::new(),
        full_reparse: true,
        max_spans_per_line: 256,
        viewport: None,
    };
    engine.schedule_rehighlight(request);
    let snapshot = wait_for_snapshot(&mut engine, 1, 1);
    assert!(line_has_group(&snapshot, 0, HighlightGroup::Keyword));
    assert!(line_has_group(&snapshot, 0, HighlightGroup::Number));
}

#[test]
fn python_highlights_keywords() {
    let mut engine = TreeSitterEngine::new();
    let text = "def foo(x):\n    return x\n";
    let request = HighlightRequest {
        buffer_id: 2,
        version: 1,
        language: LanguageId::Python,
        text: text.to_string(),
        edits: Vec::new(),
        full_reparse: true,
        max_spans_per_line: 256,
        viewport: None,
    };
    engine.schedule_rehighlight(request);
    let snapshot = wait_for_snapshot(&mut engine, 2, 1);
    assert!(line_has_group(&snapshot, 0, HighlightGroup::Keyword));
}

#[test]
fn javascript_highlights_keywords() {
    let mut engine = TreeSitterEngine::new();
    let text = "function foo() { return 1; }\n";
    let request = HighlightRequest {
        buffer_id: 4,
        version: 1,
        language: LanguageId::JavaScript,
        text: text.to_string(),
        edits: Vec::new(),
        full_reparse: true,
        max_spans_per_line: 256,
        viewport: None,
    };
    engine.schedule_rehighlight(request);
    let snapshot = wait_for_snapshot(&mut engine, 4, 1);
    assert!(line_has_group(&snapshot, 0, HighlightGroup::Keyword));
}

#[test]
fn markdown_highlights_heading() {
    let mut engine = TreeSitterEngine::new();
    let text = "# Title\n\nText\n";
    let request = HighlightRequest {
        buffer_id: 3,
        version: 1,
        language: LanguageId::Markdown,
        text: text.to_string(),
        edits: Vec::new(),
        full_reparse: true,
        max_spans_per_line: 256,
        viewport: None,
    };
    engine.schedule_rehighlight(request);
    let snapshot = wait_for_snapshot(&mut engine, 3, 1);
    assert!(line_has_group(&snapshot, 0, HighlightGroup::Heading));
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

// --- New tests for v2 features ---

#[test]
fn viewport_scoped_highlight_produces_spans_for_visible_range() {
    let mut engine = TreeSitterEngine::new();
    // Generate a file with many lines
    let mut lines = Vec::new();
    for i in 0..500 {
        lines.push(format!("let x{i} = {i};\n"));
    }
    let text = lines.join("");
    let request = HighlightRequest {
        buffer_id: 10,
        version: 1,
        language: LanguageId::Rust,
        text: text.to_string(),
        edits: Vec::new(),
        full_reparse: true,
        max_spans_per_line: 256,
        // Viewport at lines 200-220
        viewport: Some(ViewportRange {
            first_line: 200,
            last_line: 220,
            total_lines: 500,
        }),
    };
    engine.schedule_rehighlight(request);
    let snapshot = wait_for_snapshot(&mut engine, 10, 1);

    // Should have highlighted_range set
    assert!(snapshot.highlighted_range.is_some());
    let (hl_start, hl_end) = snapshot.highlighted_range.unwrap();

    // Buffer zone is 100 lines, so range should be ~100-320
    assert!(hl_start <= 100, "hl_start={hl_start}");
    assert!(hl_end >= 320, "hl_end={hl_end}");

    // Lines in the highlighted range should have spans
    assert!(
        !snapshot.per_line[200].is_empty(),
        "viewport line 200 should have spans"
    );
    assert!(
        !snapshot.per_line[210].is_empty(),
        "viewport line 210 should have spans"
    );

    // Lines far outside the initial highlighted range may still be empty
    // (or filled by progressive fill if it already ran). Verify that line 0
    // has a sentinel hash (0) if it's outside the highlighted range, indicating
    // it was not part of the initial viewport highlight.
    if hl_start > 0 {
        assert_eq!(
            snapshot.line_hashes[0], 0,
            "line 0 should have sentinel hash when outside initial range"
        );
    }
}

#[test]
fn large_file_parses_without_fallback() {
    let mut engine = TreeSitterEngine::new();
    // Generate a 5MB+ file
    let line = "let variable_name = \"hello world\";\n";
    let repeat_count = (5_000_000 / line.len()) + 1;
    let text: String = line.repeat(repeat_count);
    assert!(text.len() > 5_000_000);

    let request = HighlightRequest {
        buffer_id: 20,
        version: 1,
        language: LanguageId::Rust,
        text: text.clone(),
        edits: Vec::new(),
        full_reparse: true,
        max_spans_per_line: 256,
        viewport: Some(ViewportRange {
            first_line: 0,
            last_line: 50,
            total_lines: repeat_count,
        }),
    };
    engine.schedule_rehighlight(request);
    let snapshot = wait_for_snapshot_long(&mut engine, 20, 1);

    // Status should be TreeSitter, not error or plain
    assert_eq!(
        snapshot.status,
        crate::highlight::SyntaxStatus::Ok(crate::highlight::EngineKind::TreeSitter)
    );
    // Should have spans in viewport area
    assert!(
        !snapshot.per_line[0].is_empty(),
        "first line should have spans"
    );
}

#[test]
fn progressive_fill_covers_full_file() {
    let mut engine = TreeSitterEngine::new();
    let mut lines = Vec::new();
    for i in 0..400 {
        lines.push(format!("let x{i} = {i};\n"));
    }
    let text = lines.join("");

    // Viewport at the beginning
    let request = HighlightRequest {
        buffer_id: 30,
        version: 1,
        language: LanguageId::Rust,
        text: text.clone(),
        edits: Vec::new(),
        full_reparse: true,
        max_spans_per_line: 256,
        viewport: Some(ViewportRange {
            first_line: 0,
            last_line: 30,
            total_lines: 400,
        }),
    };
    engine.schedule_rehighlight(request);

    // Wait for progressive fill to complete (highlighted_range becomes None)
    let start = Instant::now();
    loop {
        if let Some(snapshot) = engine.try_get_highlights(30, 1) {
            if snapshot.highlighted_range.is_none() {
                // Progressive fill completed - check that late lines have spans
                assert!(
                    !snapshot.per_line[350].is_empty(),
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

    // First: highlight as Rust
    let request = HighlightRequest {
        buffer_id: 40,
        version: 1,
        language: LanguageId::Rust,
        text: text.to_string(),
        edits: Vec::new(),
        full_reparse: true,
        max_spans_per_line: 256,
        viewport: None,
    };
    engine.schedule_rehighlight(request);
    let snap1 = wait_for_snapshot(&mut engine, 40, 1);
    assert_eq!(snap1.language, LanguageId::Rust);

    // Second: highlight same buffer as Python
    let request = HighlightRequest {
        buffer_id: 40,
        version: 2,
        language: LanguageId::Python,
        text: text.to_string(),
        edits: Vec::new(),
        full_reparse: true,
        max_spans_per_line: 256,
        viewport: None,
    };
    engine.schedule_rehighlight(request);
    let snap2 = wait_for_snapshot(&mut engine, 40, 2);
    assert_eq!(snap2.language, LanguageId::Python);
}

#[test]
fn worker_recovers_from_error() {
    let mut engine = TreeSitterEngine::new();

    // Send a request with an invalid/unknown language that has no config
    // The worker should handle this gracefully
    let request = HighlightRequest {
        buffer_id: 50,
        version: 1,
        language: LanguageId::PlainText,
        text: "hello\n".to_string(),
        edits: Vec::new(),
        full_reparse: true,
        max_spans_per_line: 256,
        viewport: None,
    };
    engine.schedule_rehighlight(request);

    // Should get a result (even if it's an error snapshot)
    let start = Instant::now();
    loop {
        if let Some(snapshot) = engine.try_get_highlights(50, 1) {
            // Worker survived and produced a result
            assert_eq!(snapshot.buffer_id, 50);
            break;
        }
        if start.elapsed() > Duration::from_millis(500) {
            panic!("worker did not respond");
        }
        std::thread::sleep(Duration::from_millis(10));
    }

    // Worker should still be alive for subsequent requests
    let request = HighlightRequest {
        buffer_id: 51,
        version: 1,
        language: LanguageId::Rust,
        text: "let x = 1;\n".to_string(),
        edits: Vec::new(),
        full_reparse: true,
        max_spans_per_line: 256,
        viewport: None,
    };
    engine.schedule_rehighlight(request);
    let snapshot = wait_for_snapshot(&mut engine, 51, 1);
    assert!(line_has_group(&snapshot, 0, HighlightGroup::Keyword));
}

#[test]
fn highlighted_range_none_for_eager_mode() {
    let mut engine = TreeSitterEngine::new();
    let text = "fn main() {}\n";
    let request = HighlightRequest {
        buffer_id: 60,
        version: 1,
        language: LanguageId::Rust,
        text: text.to_string(),
        edits: Vec::new(),
        full_reparse: true,
        max_spans_per_line: 256,
        viewport: None, // No viewport = eager mode
    };
    engine.schedule_rehighlight(request);
    let snapshot = wait_for_snapshot(&mut engine, 60, 1);
    assert!(
        snapshot.highlighted_range.is_none(),
        "eager mode should have highlighted_range = None"
    );
}

fn wait_for_snapshot(
    engine: &mut TreeSitterEngine,
    buffer_id: usize,
    version: u64,
) -> HighlightSnapshot {
    let start = Instant::now();
    loop {
        if let Some(snapshot) = engine.try_get_highlights(buffer_id, version) {
            return snapshot;
        }
        if start.elapsed() > Duration::from_millis(250) {
            panic!("timed out waiting for highlight snapshot");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn wait_for_snapshot_long(
    engine: &mut TreeSitterEngine,
    buffer_id: usize,
    version: u64,
) -> HighlightSnapshot {
    let start = Instant::now();
    loop {
        if let Some(snapshot) = engine.try_get_highlights(buffer_id, version) {
            return snapshot;
        }
        if start.elapsed() > Duration::from_secs(5) {
            panic!("timed out waiting for highlight snapshot (large file)");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn line_has_group(snapshot: &HighlightSnapshot, line: usize, group: HighlightGroup) -> bool {
    snapshot
        .per_line
        .get(line)
        .map(|segments| segments.iter().any(|s| s.group == group))
        .unwrap_or(false)
}
