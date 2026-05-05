//! Span ordering and per-line distribution: intersects highlight spans with
//! the line-offset table and cuts them into `LineSegment`s respecting the
//! `max_per_line` cap and an optional per-line predicate.

use std::cmp::Ordering;

use super::lines::find_line;
use super::types::{HighlightSpan, LineSegment};

/// Sort by `start_byte` ascending; break ties by `priority` descending so
/// higher-priority spans land first on the line.
pub(crate) fn sort_spans(spans: &mut [HighlightSpan]) {
    spans.sort_unstable_by(|a, b| match a.start_byte.cmp(&b.start_byte) {
        Ordering::Equal => b.priority.cmp(&a.priority),
        ord => ord,
    });
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
