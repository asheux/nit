//! Span ordering and per-line distribution: intersects highlight spans with
//! the line-offset table and cuts them into `LineSegment`s respecting the
//! `max_per_line` cap and an optional per-line predicate.

use std::cmp::Reverse;

use super::lines::find_line;
use super::types::{HighlightSpan, LineSegment};

/// The editor paints spans in order. Paint enclosing nodes before children,
/// and keep the last matching query pattern for identical token ranges.
pub(crate) fn sort_spans(spans: &mut Vec<HighlightSpan>) {
    spans.sort_unstable_by_key(|s| (s.start_byte, Reverse(s.end_byte), Reverse(s.priority)));
    spans.dedup_by_key(|s| (s.start_byte, s.end_byte));
}

pub(crate) fn distribute_spans_to_lines(
    spans: &[HighlightSpan],
    offsets: &[usize],
    segments: &mut [Vec<LineSegment>],
    max_per_line: usize,
    predicate: impl Fn(usize) -> bool,
) {
    for span in spans {
        if span.end_byte <= span.start_byte {
            continue;
        }
        let first = find_line(offsets, span.start_byte);
        let last = find_line(offsets, span.end_byte);

        for line in first..=last {
            if line + 1 >= offsets.len() || line >= segments.len() {
                break;
            }
            if !predicate(line) {
                continue;
            }
            if max_per_line > 0 && segments[line].len() >= max_per_line {
                continue;
            }

            let line_start = offsets[line];
            let line_end = offsets[line + 1];
            debug_assert!(
                span.end_byte >= line_start,
                "span ending before line start: span={span:?} line_start={line_start}"
            );
            let seg_start = span.start_byte.max(line_start) - line_start;
            let seg_end = span.end_byte.min(line_end) - line_start;

            if seg_start < seg_end {
                segments[line].push(LineSegment {
                    start: seg_start,
                    end: seg_end,
                    group: span.group,
                });
            }
        }
    }
}
