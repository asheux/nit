//! Byte-indexed segments → char-indexed segments via a precomputed table of
//! char boundaries; segments that fall mid-UTF-8 are rejected since they
//! cannot map to a whole number of chars.

use super::types::{LineSegment, MappedLineSegment, SegmentMapError};

pub fn map_line_segments_to_chars(
    line: &str,
    segments: &[LineSegment],
) -> Result<Vec<MappedLineSegment>, SegmentMapError> {
    if segments.is_empty() || line.is_empty() {
        return Ok(Vec::new());
    }

    let byte_len = line.len();
    let mut boundaries: Vec<usize> = line.char_indices().map(|(pos, _)| pos).collect();
    boundaries.push(byte_len);

    let mut result = Vec::with_capacity(segments.len());
    for seg in segments {
        if let Some(mapped) = resolve_segment_chars(seg, byte_len, &boundaries)? {
            result.push(mapped);
        }
    }
    Ok(result)
}

fn resolve_segment_chars(
    seg: &LineSegment,
    byte_len: usize,
    boundaries: &[usize],
) -> Result<Option<MappedLineSegment>, SegmentMapError> {
    let start = seg.start;
    let end = seg.end.min(byte_len);

    if start >= byte_len || start >= end {
        return Ok(None);
    }

    let err = || SegmentMapError {
        start,
        end,
        line_len: byte_len,
    };

    let char_start = boundaries.binary_search(&start).map_err(|_| err())?;
    let char_end = boundaries.binary_search(&end).map_err(|_| err())?;

    Ok((char_start < char_end).then_some(MappedLineSegment {
        start: char_start,
        end: char_end,
        group: seg.group,
    }))
}
