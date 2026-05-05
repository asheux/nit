//! Full-file and viewport-scoped highlight strategies plus the shared
//! tree-sitter query executor that backs them.

use std::cmp;
use std::collections::HashMap;

use tree_sitter::{QueryCursor, Tree};
use tree_sitter_highlight::{HighlightConfiguration, HighlightEvent, Highlighter};

use crate::captures::{capture_group, QueryConfig};
use crate::engine::HighlightRequest;
use crate::highlight::{
    compute_line_starts, distribute_spans_to_lines, recompute_line_hashes, sort_spans, EngineKind,
    HighlightGroup, HighlightSnapshot, HighlightSpan, SyntaxStatus,
};
use crate::language::{LanguageId, LanguageRegistry};

use super::job::fallback_snapshot;

// Amount by which viewport-scoped highlights extend beyond the visible range,
// so small scroll jitter doesn't immediately require a re-request.
pub(super) const VIEWPORT_MARGIN_LINES: usize = 100;

pub(super) fn full_highlight(
    configs: &HashMap<LanguageId, HighlightConfiguration>,
    config: &HighlightConfiguration,
    highlighter: &mut Highlighter,
    job: &HighlightRequest,
) -> anyhow::Result<HighlightSnapshot> {
    let mut spans = Vec::new();
    let mut stack: Vec<HighlightGroup> = Vec::new();

    let iter = highlighter.highlight(config, job.text.as_bytes(), None, |name| {
        LanguageRegistry::from_injection_name(name).and_then(|id| configs.get(&id))
    })?;

    for event in iter {
        match event? {
            HighlightEvent::HighlightStart(s) => stack.push(capture_group(s.0)),
            HighlightEvent::HighlightEnd => {
                stack.pop();
            }
            HighlightEvent::Source { start, end } => {
                let group = stack.last().copied().unwrap_or(HighlightGroup::Normal);
                if group != HighlightGroup::Normal && end > start {
                    spans.push(HighlightSpan {
                        start_byte: start,
                        end_byte: end,
                        group,
                        priority: 0,
                        modifiers: 0,
                    });
                }
            }
        }
    }

    Ok(HighlightSnapshot::from_spans(
        job.buffer_id,
        job.version,
        job.language,
        EngineKind::TreeSitter,
        SyntaxStatus::Ok(EngineKind::TreeSitter),
        &job.text,
        spans,
        job.max_spans_per_line,
    ))
}

pub(super) fn viewport_highlight(
    query_configs: &HashMap<LanguageId, QueryConfig>,
    tree: &Tree,
    job: &HighlightRequest,
    cursor: &mut QueryCursor,
) -> anyhow::Result<HighlightSnapshot> {
    let viewport = job.viewport.as_ref().unwrap();
    let offsets = compute_line_starts(&job.text);
    let total = offsets.len().saturating_sub(1);

    // Clamp the viewport against the actual buffer size: a request can carry a
    // `first_line` from a previous, larger buffer, and indexing `offsets`
    // unguarded would panic and force the worker to drop BufferState every
    // frame, defeating incremental highlight entirely.
    let last_line_idx = total.saturating_sub(1);
    let start_line = viewport
        .first_line
        .saturating_sub(VIEWPORT_MARGIN_LINES)
        .min(last_line_idx);
    let end_line = (viewport.last_line + VIEWPORT_MARGIN_LINES)
        .min(last_line_idx)
        .max(start_line);

    debug_assert!(
        start_line < offsets.len(),
        "start_line {start_line} out of offsets len {}",
        offsets.len()
    );
    let start_byte = offsets.get(start_line).copied().unwrap_or(0);
    let end_byte = offsets.get(end_line + 1).copied().unwrap_or(job.text.len());

    let Some(cfg) = query_configs.get(&job.language) else {
        return Ok(fallback_snapshot(
            job,
            SyntaxStatus::Error("no query config".into()),
        ));
    };

    let mut spans = Vec::new();
    collect_spans(
        cfg,
        tree,
        job.text.as_bytes(),
        &[(start_byte, end_byte)],
        &mut spans,
        cursor,
    );
    sort_spans(&mut spans);

    let mut per_line = vec![Vec::new(); total];
    distribute_spans_to_lines(
        &spans,
        &offsets,
        &mut per_line,
        job.max_spans_per_line,
        |_| true,
    );

    let mut line_hashes = vec![0u64; total];
    recompute_line_hashes(
        job.text.as_bytes(),
        &offsets,
        &mut line_hashes,
        start_line..=end_line.min(total.saturating_sub(1)),
    );

    Ok(HighlightSnapshot {
        buffer_id: job.buffer_id,
        version: job.version,
        language: job.language,
        engine: EngineKind::TreeSitter,
        status: SyntaxStatus::Ok(EngineKind::TreeSitter),
        duration_ms: 0,
        line_start_bytes: offsets,
        line_hashes,
        per_line,
        highlighted_range: Some((start_line, end_line)),
    })
}

pub(super) fn collect_spans(
    cfg: &QueryConfig,
    tree: &Tree,
    source: &[u8],
    ranges: &[(usize, usize)],
    spans: &mut Vec<HighlightSpan>,
    cursor: &mut QueryCursor,
) {
    let root = tree.root_node();
    for &(start, end) in ranges {
        if start >= end {
            continue;
        }
        cursor.set_byte_range(start..end);
        for m in cursor.matches(&cfg.query, root, source) {
            let priority = cmp::min(m.pattern_index, u8::MAX as usize) as u8;
            for cap in m.captures {
                let group = cfg.highlight_for_index(cap.index as usize);
                if group == HighlightGroup::Normal {
                    continue;
                }
                let node = cap.node;
                let span_start = node.start_byte().max(start);
                let span_end = node.end_byte().min(end);
                if span_start < span_end {
                    spans.push(HighlightSpan {
                        start_byte: span_start,
                        end_byte: span_end,
                        group,
                        priority,
                        modifiers: 0,
                    });
                }
            }
        }
    }
}
