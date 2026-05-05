use nit_core::Buffer;

use crate::{
    EngineKind, HighlightGroup, HighlightSnapshot, HighlightSpan, LanguageId, SyntaxStatus,
};

fn make_snapshot(text: &str, spans: Vec<HighlightSpan>) -> HighlightSnapshot {
    HighlightSnapshot::from_spans(
        0,
        1,
        LanguageId::PlainText,
        EngineKind::Plain,
        SyntaxStatus::Ok(EngineKind::Plain),
        text,
        spans,
        32,
    )
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
    let snap = make_snapshot(text, spans);
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
fn overlapping_spans_emit_higher_priority_first() {
    // Two spans starting at the same byte: `sort_spans` orders by
    // start_byte ascending, then priority descending, so the higher-
    // priority span lands first on the line. UI layers then choose how
    // to render the overlap — but they need a stable, priority-ordered
    // input.
    let spans = vec![
        HighlightSpan {
            start_byte: 0,
            end_byte: 3,
            group: HighlightGroup::Variable,
            priority: 1,
            modifiers: 0,
        },
        HighlightSpan {
            start_byte: 0,
            end_byte: 5,
            group: HighlightGroup::Function,
            priority: 5,
            modifiers: 0,
        },
    ];
    let snap = make_snapshot("abcdef", spans);
    assert_eq!(snap.per_line[0].len(), 2);
    assert_eq!(snap.per_line[0][0].group, HighlightGroup::Function);
    assert_eq!(snap.per_line[0][1].group, HighlightGroup::Variable);
}

#[test]
fn span_at_trailing_newline_stays_on_first_line() {
    // For text `"ab\n"`, compute_line_starts produces `[0, 3]` — one
    // logical line spanning bytes 0..3. A span over the trailing `\n`
    // alone must land on line 0 and never spill into a phantom second
    // line: the `line + 1 >= offsets.len()` guard in
    // distribute_spans_to_lines is what prevents that overflow.
    let spans = vec![HighlightSpan {
        start_byte: 2,
        end_byte: 3,
        group: HighlightGroup::Punctuation,
        priority: 0,
        modifiers: 0,
    }];
    let snap = make_snapshot("ab\n", spans);
    assert_eq!(snap.per_line.len(), 1);
    assert_eq!(snap.per_line[0].len(), 1);
    assert_eq!(snap.per_line[0][0].start, 2);
    assert_eq!(snap.per_line[0][0].end, 3);
    assert_eq!(snap.per_line[0][0].group, HighlightGroup::Punctuation);
}
