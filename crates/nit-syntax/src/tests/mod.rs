use std::time::{Duration, Instant};

use crate::engine::tree_sitter::TreeSitterEngine;
use crate::{
    HighlightGroup, HighlightRequest, HighlightSnapshot, LanguageId, SyntaxEngine, ViewportRange,
};

mod classification;
mod debouncer;
mod engines;
mod language_detect;
mod segments;
mod span_distribution;
mod viewport;

pub(super) const DEFAULT_TIMEOUT: Duration = Duration::from_secs(2);
pub(super) const DEFAULT_POLL: Duration = Duration::from_millis(10);
pub(super) const LONG_TIMEOUT: Duration = Duration::from_secs(5);
pub(super) const LONG_POLL: Duration = Duration::from_millis(50);

pub(super) fn make_request(
    buffer_id: usize,
    version: u64,
    lang: LanguageId,
    text: &str,
) -> HighlightRequest {
    HighlightRequest {
        buffer_id,
        version,
        language: lang,
        text: text.to_string(),
        edits: Vec::new(),
        full_reparse: true,
        max_spans_per_line: 256,
        viewport: None,
    }
}

pub(super) fn make_viewport_request(
    buffer_id: usize,
    version: u64,
    lang: LanguageId,
    text: &str,
    viewport: ViewportRange,
) -> HighlightRequest {
    HighlightRequest {
        viewport: Some(viewport),
        ..make_request(buffer_id, version, lang, text)
    }
}

pub(super) fn poll_snapshot(
    engine: &mut TreeSitterEngine,
    buffer_id: usize,
    version: u64,
    timeout: Duration,
    interval: Duration,
) -> HighlightSnapshot {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(snap) = engine.try_get_highlights(buffer_id, version) {
            return snap;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for snapshot ({timeout:?})"
        );
        std::thread::sleep(interval);
    }
}

pub(super) fn wait_for(
    engine: &mut TreeSitterEngine,
    buffer_id: usize,
    version: u64,
) -> HighlightSnapshot {
    poll_snapshot(engine, buffer_id, version, DEFAULT_TIMEOUT, DEFAULT_POLL)
}

pub(super) fn wait_for_long(
    engine: &mut TreeSitterEngine,
    buffer_id: usize,
    version: u64,
) -> HighlightSnapshot {
    poll_snapshot(engine, buffer_id, version, LONG_TIMEOUT, LONG_POLL)
}

pub(super) fn poll_until(
    engine: &mut TreeSitterEngine,
    buffer_id: usize,
    version: u64,
    predicate: impl Fn(&HighlightSnapshot) -> bool,
    timeout: Duration,
) -> HighlightSnapshot {
    let deadline = Instant::now() + timeout;
    loop {
        let candidate = engine
            .try_get_highlights(buffer_id, version)
            .filter(|s| predicate(s));
        if let Some(snap) = candidate {
            return snap;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for predicate ({timeout:?})"
        );
        std::thread::sleep(LONG_POLL);
    }
}

pub(super) fn has_group(snapshot: &HighlightSnapshot, line: usize, group: HighlightGroup) -> bool {
    snapshot
        .per_line
        .get(line)
        .is_some_and(|segs| segs.iter().any(|s| s.group == group))
}
