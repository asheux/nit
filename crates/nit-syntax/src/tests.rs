use std::time::{Duration, Instant};

use nit_core::Buffer;

use crate::engine::{HighlightRequest, SyntaxEngine};
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

fn line_has_group(snapshot: &HighlightSnapshot, line: usize, group: HighlightGroup) -> bool {
    snapshot
        .per_line
        .get(line)
        .map(|segments| segments.iter().any(|s| s.group == group))
        .unwrap_or(false)
}
