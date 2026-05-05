//! Progressive-fill: after a viewport-scoped job returns, continue expanding
//! the highlighted range downward then upward in chunks, so the rest of the
//! buffer becomes highlighted without blocking the initial response.

use std::collections::HashMap;
use std::sync::mpsc::Sender;
use std::time::Duration;

use crate::captures::QueryConfig;
use crate::engine::HighlightRequest;
use crate::highlight::{
    distribute_spans_to_lines, recompute_line_hashes, sort_spans, HighlightSnapshot,
};
use crate::language::LanguageId;

use super::job::BufferState;
use super::modes::collect_spans;
use super::worker::{HighlightResult, WorkerState};

// Chunk size for progressive fill beyond the initial viewport window.
pub(super) const FILL_CHUNK_LINES: usize = 500;
// Worker wakes up to drive progressive fill even when no request arrives.
pub(super) const FILL_IDLE_TIMEOUT: Duration = Duration::from_millis(10);

/// Outstanding progressive-fill work for a viewport-scoped job: expands
/// below the initial range first, then above, chunk by chunk.
pub(super) struct ProgressiveFill {
    buffer_id: usize,
    version: u64,
    language: LanguageId,
    text: String,
    line_start_bytes: Vec<usize>,
    next_below: Option<usize>,
    next_above: Option<usize>,
    chunk_size: usize,
    total_lines: usize,
    max_spans_per_line: usize,
}

pub(super) fn make_progressive_fill(
    job: &HighlightRequest,
    snapshot: &HighlightSnapshot,
) -> Option<ProgressiveFill> {
    let (hl_start, hl_end) = snapshot.highlighted_range?;
    let total = snapshot.per_line.len();
    debug_assert!(
        hl_end < total,
        "highlighted_range end {hl_end} >= total lines {total}"
    );

    if hl_end + 1 >= total && hl_start == 0 {
        return None;
    }

    Some(ProgressiveFill {
        buffer_id: job.buffer_id,
        version: job.version,
        language: job.language,
        text: job.text.clone(),
        line_start_bytes: snapshot.line_start_bytes.clone(),
        next_below: (hl_end + 1 < total).then_some(hl_end + 1),
        next_above: (hl_start > 0).then_some(hl_start),
        chunk_size: FILL_CHUNK_LINES,
        total_lines: total,
        max_spans_per_line: job.max_spans_per_line,
    })
}

pub(super) fn step_progressive_fills(
    fills: &mut HashMap<usize, ProgressiveFill>,
    state: &mut WorkerState,
    res_tx: &Sender<HighlightResult>,
) {
    let ids: Vec<usize> = fills.keys().copied().collect();
    for id in ids {
        let Some(fill) = fills.get_mut(&id) else {
            continue;
        };
        if process_fill_chunk(fill, &mut state.buffers, &state.query_configs, res_tx) {
            fills.remove(&id);
        }
    }
}

fn process_fill_chunk(
    fill: &mut ProgressiveFill,
    buffers: &mut HashMap<usize, BufferState>,
    query_configs: &HashMap<LanguageId, QueryConfig>,
    res_tx: &Sender<HighlightResult>,
) -> bool {
    let Some((start_line, end_line)) = next_fill_chunk(fill) else {
        return true;
    };

    let Some(cfg) = query_configs.get(&fill.language) else {
        return true;
    };
    // Pattern-destructure the buffer state so `tree`, `snapshot`, and `cursor`
    // land as independent field borrows — avoids an aliasing error when we
    // pass an immutable `tree` alongside a mutable `snapshot`/`cursor` below.
    let Some(BufferState {
        tree,
        snapshot,
        cursor,
        ..
    }) = buffers.get_mut(&fill.buffer_id)
    else {
        return true;
    };
    let Some(tree) = tree.as_ref() else {
        return true;
    };
    let Some(snapshot) = snapshot.as_mut() else {
        return true;
    };
    if snapshot.version != fill.version {
        return true;
    }

    debug_assert!(
        start_line < fill.line_start_bytes.len(),
        "start_line {start_line} out of fill line_start_bytes len {}",
        fill.line_start_bytes.len()
    );
    let start_byte = fill.line_start_bytes[start_line];
    let end_byte = fill
        .line_start_bytes
        .get(end_line + 1)
        .copied()
        .unwrap_or(fill.text.len());

    let mut spans = Vec::new();
    collect_spans(
        cfg,
        tree,
        fill.text.as_bytes(),
        &[(start_byte, end_byte)],
        &mut spans,
        cursor,
    );

    sort_spans(&mut spans);
    distribute_spans_to_lines(
        &spans,
        &fill.line_start_bytes,
        &mut snapshot.per_line,
        fill.max_spans_per_line,
        |line| line >= start_line && line <= end_line,
    );

    recompute_line_hashes(
        fill.text.as_bytes(),
        &fill.line_start_bytes,
        &mut snapshot.line_hashes,
        start_line..=end_line,
    );

    expand_highlighted_range(snapshot, fill, start_line, end_line);

    let _ = res_tx.send(HighlightResult {
        buffer_id: fill.buffer_id,
        snapshot: snapshot.clone(),
    });

    fill.next_below.is_none() && fill.next_above.is_none()
}

// Advance the fill cursor, filling downward first and then upward. The
// `next_below`/`next_above` options are updated so that subsequent calls
// pick up where this one left off.
fn next_fill_chunk(fill: &mut ProgressiveFill) -> Option<(usize, usize)> {
    if let Some(next) = fill.next_below {
        let end = (next + fill.chunk_size).min(fill.total_lines);
        fill.next_below = (end < fill.total_lines).then_some(end);
        return Some((next, end.saturating_sub(1)));
    }
    if let Some(above_end) = fill.next_above {
        let start = above_end.saturating_sub(fill.chunk_size);
        fill.next_above = (start > 0).then_some(start);
        return Some((start, above_end.saturating_sub(1)));
    }
    None
}

fn expand_highlighted_range(
    snapshot: &mut HighlightSnapshot,
    fill: &ProgressiveFill,
    start_line: usize,
    end_line: usize,
) {
    let (prev_start, prev_end) = snapshot
        .highlighted_range
        .unwrap_or((0, fill.total_lines.saturating_sub(1)));
    let new_start = prev_start.min(start_line);
    let new_end = prev_end.max(end_line);

    let fully_covered = new_start == 0 && new_end >= fill.total_lines.saturating_sub(1);
    snapshot.highlighted_range = if fully_covered {
        None
    } else {
        Some((new_start, new_end))
    };
}
