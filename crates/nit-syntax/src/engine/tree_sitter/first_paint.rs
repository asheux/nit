//! Provisional top-of-file color while a large document's full tree is built.

use tree_sitter_highlight::Highlighter;

use super::modes::highlight_source;
use crate::engine::HighlightRequest;
use crate::highlight::{compute_line_starts, EngineKind, HighlightSnapshot, SyntaxStatus};

const MIN_DOCUMENT_BYTES: usize = 128 * 1024;
const PREFIX_BYTE_LIMIT: usize = 128 * 1024;
const PREFIX_LINE_LIMIT: usize = 512;
const LOOKAHEAD: usize = 64;

pub(super) fn top_snapshot(
    job: &HighlightRequest,
    highlighter: &mut Highlighter,
) -> Option<HighlightSnapshot> {
    let viewport = job.viewport.as_ref()?;
    if viewport.first_line != 0
        || !job.full_reparse
        || !job.edits.is_empty()
        || job.text.len() < MIN_DOCUMENT_BYTES
    {
        return None;
    }
    // A just-opened buffer may still have a one-line initial viewport.
    let covered = viewport.last_line.checked_add(1)?.max(64);
    if covered > PREFIX_LINE_LIMIT - LOOKAHEAD {
        return None;
    }
    let mut ends = job.text.as_bytes()[..PREFIX_BYTE_LIMIT.min(job.text.len())]
        .iter()
        .enumerate()
        .filter_map(|(idx, &byte)| (byte == b'\n').then_some(idx + 1));
    let visible_end = ends.nth(covered - 1)?;
    let parse_end = ends.take(LOOKAHEAD).last()?;
    let prefix = &job.text[..parse_end];
    let config = crate::captures::highlight_config(job.language)?;
    let mut spans = highlight_source(config, highlighter, prefix.as_bytes()).ok()?;
    spans.retain(|span| span.start_byte < visible_end);
    for span in &mut spans {
        span.end_byte = span.end_byte.min(visible_end);
    }
    let mut snapshot = HighlightSnapshot::from_spans(
        job.buffer_id,
        job.version,
        job.language,
        EngineKind::TreeSitter,
        SyntaxStatus::Ok(EngineKind::TreeSitter),
        &job.text[..visible_end],
        spans,
        job.max_spans_per_line,
    );
    // Keep offsets and row counts valid for the real document. Unpainted rows
    // have no spans and zero hashes until the authoritative result arrives.
    snapshot.line_start_bytes = compute_line_starts(&job.text);
    let lines = snapshot.line_start_bytes.len() - 1;
    snapshot.per_line.resize_with(lines, Vec::new);
    snapshot.line_hashes.resize(lines, 0);
    snapshot.highlighted_range = Some((0, covered - 1));
    Some(snapshot)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::{has_group, make_viewport_request};
    use crate::{HighlightGroup, LanguageId, ViewportRange};

    #[test]
    fn preview_preserves_multibyte_line_offsets_and_embedded_colors() {
        let line = "const café = 42;\r\n";
        let text = format!(
            "<script>\n{line}</script>\n{}",
            "<p>héllo</p>\n".repeat(11000)
        );
        let mut job = make_viewport_request(
            5,
            8,
            LanguageId::Html,
            &text,
            ViewportRange {
                first_line: 0,
                last_line: 20,
                total_lines: text.lines().count(),
            },
        );
        let mut highlighter = Highlighter::new();
        let snapshot = top_snapshot(&job, &mut highlighter).unwrap();
        assert_eq!((snapshot.buffer_id, snapshot.version), (5, 8));
        assert!(has_group(&snapshot, 1, HighlightGroup::Number));
        assert_eq!(
            snapshot.line_hashes[1],
            crate::highlight::hash_line_bytes(line.as_bytes())
        );
        assert_eq!(
            snapshot.line_start_bytes[2],
            "<script>\n".len() + line.len()
        );
        assert_eq!(snapshot.line_start_bytes.last(), Some(&text.len()));
        assert_eq!(snapshot.highlighted_range, Some((0, 63)));
        assert!(snapshot.line_hashes[64..].iter().all(|h| *h == 0));
        assert!(snapshot.per_line[64..].iter().all(Vec::is_empty));
        job.viewport.as_mut().unwrap().first_line = 1;
        assert!(top_snapshot(&job, &mut highlighter).is_none());
        job.viewport.as_mut().unwrap().first_line = 0;
        job.full_reparse = false;
        assert!(top_snapshot(&job, &mut highlighter).is_none());
    }

    #[test]
    fn oversized_first_line_does_not_trigger_an_unbounded_preview() {
        let job = make_viewport_request(
            1,
            1,
            LanguageId::Rust,
            &"界".repeat(PREFIX_BYTE_LIMIT),
            ViewportRange {
                first_line: 0,
                last_line: 10,
                total_lines: 1,
            },
        );
        assert!(top_snapshot(&job, &mut Highlighter::new()).is_none());
    }
}

#[cfg(test)]
mod publication {
    use super::super::worker::worker_loop;
    use crate::tests::make_viewport_request;
    use crate::{LanguageId, ViewportRange};
    use std::{sync::mpsc, time::Duration};

    #[test]
    fn worker_replaces_top_preview_with_full_context_for_the_same_revision() {
        let text = format!(
            "{}/*{}*/\n",
            "fn café() {}\n".repeat(180),
            "x".repeat(150_000)
        );
        let request = make_viewport_request(
            4,
            11,
            LanguageId::Rust,
            &text,
            ViewportRange {
                first_line: 0,
                last_line: 12,
                total_lines: 181,
            },
        );
        let (jobs, input) = mpsc::channel();
        let (output, results) = mpsc::channel();
        let worker = std::thread::spawn(move || worker_loop(input, output));
        jobs.send(request).unwrap();
        drop(jobs);
        let preview = results
            .recv_timeout(Duration::from_secs(5))
            .unwrap()
            .snapshot;
        let authoritative = results
            .recv_timeout(Duration::from_secs(5))
            .unwrap()
            .snapshot;
        worker.join().unwrap();
        assert_eq!(preview.version, authoritative.version);
        assert_eq!(preview.highlighted_range, Some((0, 63)));
        assert_eq!(preview.line_hashes[90], 0);
        assert_ne!(authoritative.line_hashes[90], 0);
        assert!(!authoritative.per_line[90].is_empty());
    }
}
