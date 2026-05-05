//! Incremental rehighlight: carry unchanged lines forward from the previous
//! snapshot, mark edit-touched lines dirty, and rebuild only those lines.

use std::cmp;
use std::collections::HashMap;

use nit_core::BufferEdit;
use tree_sitter::{QueryCursor, Tree};

use crate::captures::QueryConfig;
use crate::engine::HighlightRequest;
use crate::highlight::{
    compute_line_starts, distribute_spans_to_lines, find_line, recompute_line_hashes, sort_spans,
    EngineKind, HighlightSnapshot, LineSegment, SyntaxStatus,
};
use crate::language::LanguageId;

use super::modes::collect_spans;

/// Re-highlight only the lines affected by edits, reusing carried-forward
/// content from `prev` for everything else. Lines marked dirty are those
/// `changed_ranges` reports plus any that couldn't be mapped from the prior
/// snapshot.
pub(super) fn incremental_highlight(
    prev: &HighlightSnapshot,
    edited_old: Option<&Tree>,
    tree: &Tree,
    query_configs: &HashMap<LanguageId, QueryConfig>,
    cursor: &mut QueryCursor,
    job: &HighlightRequest,
) -> HighlightSnapshot {
    let offsets = compute_line_starts(&job.text);
    let line_count = offsets.len().saturating_sub(1);

    let (mut per_line, mut line_hashes, copied) = carry_forward_lines(prev, &job.edits, line_count);

    let dirty = compute_dirty_lines(&copied, edited_old, tree, &offsets, line_count);

    if dirty.contains(&true) {
        let ctx = IncrementalCtx {
            dirty: &dirty,
            offsets: &offsets,
            per_line: &mut per_line,
            line_hashes: &mut line_hashes,
            query_configs,
            tree,
            cursor,
            job,
        };
        rehighlight_dirty(ctx);
    }

    HighlightSnapshot {
        buffer_id: job.buffer_id,
        version: job.version,
        language: job.language,
        engine: EngineKind::TreeSitter,
        status: SyntaxStatus::Ok(EngineKind::TreeSitter),
        duration_ms: 0,
        line_start_bytes: offsets,
        line_hashes,
        per_line,
        highlighted_range: None,
    }
}

/// Clone each old line's segments/hash into the new layout via the edit-shifted
/// line map. Returns `(per_line, line_hashes, copied)`, where `copied[i]` is
/// true iff line `i` received content from the previous snapshot.
fn carry_forward_lines(
    prev: &HighlightSnapshot,
    edits: &[BufferEdit],
    line_count: usize,
) -> (Vec<Vec<LineSegment>>, Vec<u64>, Vec<bool>) {
    // Snapshot invariant: per_line and line_hashes are produced together; a
    // length skew would silently zero-fill hashes for lines we still copy.
    debug_assert_eq!(
        prev.line_hashes.len(),
        prev.per_line.len(),
        "snapshot per_line/line_hashes length skew: {} vs {}",
        prev.per_line.len(),
        prev.line_hashes.len(),
    );

    let mut per_line = vec![Vec::new(); line_count];
    let mut line_hashes = vec![0u64; line_count];
    let mut copied = vec![false; line_count];

    let line_map = build_line_map(prev.per_line.len(), edits);
    for (old_i, new_i) in line_map.into_iter().enumerate() {
        let Some(new_i) = new_i else { continue };
        if new_i >= line_count {
            continue;
        }
        per_line[new_i] = prev.per_line[old_i].clone();
        if let Some(&hash) = prev.line_hashes.get(old_i) {
            line_hashes[new_i] = hash;
        }
        copied[new_i] = true;
    }

    (per_line, line_hashes, copied)
}

/// Mark a line as dirty when either: tree-sitter's `changed_ranges` report
/// touches it (with a one-line bleed on each side to cover multi-line tokens),
/// or when no previous content could be carried forward to it.
fn compute_dirty_lines(
    copied: &[bool],
    edited_old: Option<&Tree>,
    tree: &Tree,
    offsets: &[usize],
    line_count: usize,
) -> Vec<bool> {
    let mut dirty = vec![false; line_count];

    if let Some(old_tree) = edited_old {
        for range in old_tree.changed_ranges(tree) {
            if range.end_byte == 0 || line_count == 0 {
                continue;
            }
            let start = find_line(offsets, range.start_byte).saturating_sub(1);
            let end = cmp::min(
                find_line(offsets, range.end_byte.saturating_sub(1)).saturating_add(1),
                line_count.saturating_sub(1),
            );
            let bound = end.saturating_add(1).min(dirty.len());
            if start < bound {
                dirty[start..bound].fill(true);
            }
        }
    }

    for (i, &was_copied) in copied.iter().enumerate() {
        if !was_copied {
            dirty[i] = true;
        }
    }

    dirty
}

struct IncrementalCtx<'a> {
    dirty: &'a [bool],
    offsets: &'a [usize],
    per_line: &'a mut [Vec<LineSegment>],
    line_hashes: &'a mut [u64],
    query_configs: &'a HashMap<LanguageId, QueryConfig>,
    tree: &'a Tree,
    cursor: &'a mut QueryCursor,
    job: &'a HighlightRequest,
}

fn rehighlight_dirty(ctx: IncrementalCtx<'_>) {
    let IncrementalCtx {
        dirty,
        offsets,
        per_line,
        line_hashes,
        query_configs,
        tree,
        cursor,
        job,
    } = ctx;

    for (i, &is_dirty) in dirty.iter().enumerate() {
        if is_dirty {
            per_line[i].clear();
        }
    }

    if let Some(cfg) = query_configs.get(&job.language) {
        let mut spans = Vec::new();
        let ranges = dirty_byte_ranges(dirty, offsets);
        collect_spans(cfg, tree, job.text.as_bytes(), &ranges, &mut spans, cursor);
        sort_spans(&mut spans);
        distribute_spans_to_lines(&spans, offsets, per_line, job.max_spans_per_line, |line| {
            dirty[line]
        });
    }

    recompute_line_hashes(
        job.text.as_bytes(),
        offsets,
        line_hashes,
        dirty.iter().enumerate().filter(|(_, &d)| d).map(|(i, _)| i),
    );
}

fn dirty_byte_ranges(dirty: &[bool], offsets: &[usize]) -> Vec<(usize, usize)> {
    let mut ranges = Vec::new();
    let mut i = 0;
    while i < dirty.len() {
        if !dirty[i] {
            i += 1;
            continue;
        }
        let start = i;
        while i < dirty.len() && dirty[i] {
            i += 1;
        }
        let end = i.saturating_sub(1);
        let start_byte = *offsets.get(start).unwrap_or(&0);
        let end_byte = *offsets.get(end + 1).unwrap_or(&start_byte);
        ranges.push((start_byte, end_byte));
    }
    ranges
}

fn build_line_map(old_lines: usize, edits: &[BufferEdit]) -> Vec<Option<usize>> {
    let mut map: Vec<Option<usize>> = (0..old_lines).map(Some).collect();
    for edit in edits {
        let start = edit.start_point.row;
        let old_end = edit.old_end_point.row;
        let new_end = edit.new_end_point.row;
        let delta = new_end as isize - old_end as isize;
        for entry in map.iter_mut() {
            let Some(line) = *entry else { continue };
            if line < start {
                continue;
            } else if line > old_end {
                let shifted = line as isize + delta;
                *entry = (shifted >= 0).then_some(shifted as usize);
            } else {
                *entry = None;
            }
        }
    }
    map
}
