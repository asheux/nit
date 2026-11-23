//! Full-file and viewport-scoped highlight strategies plus the shared
//! tree-sitter query executor that backs them.

use std::cmp;

use tree_sitter::{QueryCursor, Tree};
use tree_sitter_highlight::{HighlightConfiguration, HighlightEvent, Highlighter};

use crate::captures::{capture_group, QueryConfig};
use crate::engine::HighlightRequest;
use crate::highlight::{
    compute_line_starts, distribute_spans_to_lines, recompute_line_hashes, sort_spans, EngineKind,
    HighlightGroup, HighlightSnapshot, HighlightSpan, SyntaxStatus,
};
use crate::language::LanguageRegistry;

// Amount by which viewport-scoped highlights extend beyond the visible range,
// so small scroll jitter doesn't immediately require a re-request.
pub(super) const VIEWPORT_MARGIN_LINES: usize = 100;

pub(super) fn full_highlight(
    highlighter: &mut Highlighter,
    job: &HighlightRequest,
) -> anyhow::Result<HighlightSnapshot> {
    let config = crate::captures::highlight_config(job.language)
        .ok_or_else(|| anyhow::anyhow!("no highlight config"))?;
    let spans = highlight_source(config, highlighter, job.text.as_bytes())?;
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

pub(super) fn highlight_source(
    config: &HighlightConfiguration,
    highlighter: &mut Highlighter,
    source: &[u8],
) -> anyhow::Result<Vec<HighlightSpan>> {
    let mut spans = Vec::new();
    let mut stack: Vec<HighlightGroup> = Vec::new();

    let iter = highlighter.highlight(config, source, None, |name| {
        if name == "markdown_inline" {
            crate::captures::markdown_inline_config()
        } else {
            LanguageRegistry::from_injection_name(name).and_then(crate::captures::highlight_config)
        }
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

    Ok(spans)
}

pub(super) fn viewport_highlight(
    highlighter: &mut Highlighter,
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
    let end_line = viewport
        .last_line
        .saturating_add(VIEWPORT_MARGIN_LINES)
        .min(last_line_idx)
        .max(start_line);

    debug_assert!(
        start_line < offsets.len(),
        "start_line {start_line} out of offsets len {}",
        offsets.len()
    );
    let (mut spans, start_line, end_line) = super::window::window_spans(
        highlighter,
        job.language,
        tree,
        &job.text,
        &offsets,
        (start_line, end_line),
        cursor,
    )?;
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
        highlighted_range: (start_line != 0 || end_line + 1 < total)
            .then_some((start_line, end_line)),
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
        // tree-sitter 0.25: QueryMatches is a streaming-iterator now,
        // not `Iterator`. Drive it with `next()` instead of `for`.
        let mut matches = cursor.matches(&cfg.query, root, source);
        while let Some(m) = streaming_iterator::StreamingIterator::next(&mut matches) {
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
