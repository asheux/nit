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

#[test]
fn map_segments_drops_segments_past_byte_len() {
    // A caller may carry a stale segment from a longer prior version of
    // the line — rather than panic, the mapper should silently drop any
    // segment whose start sits at or past byte_len, while preserving
    // valid segments alongside it.
    let line = "abc";
    let segments = vec![
        LineSegment {
            start: 5,
            end: 9,
            group: HighlightGroup::String,
        },
        LineSegment {
            start: 1,
            end: 2,
            group: HighlightGroup::Keyword,
        },
    ];
    let mapped = map_line_segments_to_chars(line, &segments).expect("map segments");
    assert_eq!(mapped.len(), 1);
    assert_eq!(mapped[0].start, 1);
    assert_eq!(mapped[0].end, 2);
    assert_eq!(mapped[0].group, HighlightGroup::Keyword);
}

#[test]
fn map_segments_handles_trailing_newline_byte() {
    // Lines that include their trailing `\n` exercise the boundary
    // sentinel push at `byte_len`. The newline is one ASCII byte, so the
    // char index for it equals the byte index, but only because the
    // sentinel is present in the boundary table.
    let line = "ab\n";
    let segments = vec![LineSegment {
        start: 2,
        end: 3,
        group: HighlightGroup::Punctuation,
    }];
    let mapped = map_line_segments_to_chars(line, &segments).expect("map segments");
    assert_eq!(mapped.len(), 1);
    assert_eq!(mapped[0].start, 2);
    assert_eq!(mapped[0].end, 3);
}
