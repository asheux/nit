use nit_core::Buffer;

use crate::{
    EngineKind, HighlightGroup, HighlightSnapshot, HighlightSpan, LanguageId, SyntaxStatus,
};

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
