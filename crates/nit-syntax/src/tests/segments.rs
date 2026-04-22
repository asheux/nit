use crate::{map_line_segments_to_chars, HighlightGroup, LineSegment};

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
