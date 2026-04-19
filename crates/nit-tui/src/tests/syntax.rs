use super::*;
use nit_core::{HighlightConfig, HighlightEngine};
use nit_syntax::{EngineKind, HighlightSnapshot, LanguageId, SyntaxStatus};

fn default_config() -> HighlightConfig {
    HighlightConfig {
        enabled: true,
        engine: HighlightEngine::Plain,
        debounce_ms: 10,
        max_file_bytes: 10_000,
        max_spans_per_line: 256,
    }
}

fn setup_runtime(buffer_id: usize) -> (Buffer, SyntaxRuntime) {
    let mut buffer = Buffer::empty("test", None);
    buffer.insert_str("a\nb\nc\n");
    buffer.take_pending_edits();
    let mut runtime = SyntaxRuntime::new(default_config());
    let snapshot = HighlightSnapshot::plain(
        buffer_id,
        buffer.version(),
        LanguageId::PlainText,
        EngineKind::Plain,
        SyntaxStatus::Ok(EngineKind::Plain),
        &buffer.content_as_string(),
    );
    runtime.snapshots.insert(buffer_id, snapshot);
    (buffer, runtime)
}

#[test]
fn render_snapshot_maps_unchanged_lines() {
    let (mut buffer, mut runtime) = setup_runtime(1);

    buffer.cursor.line = 1;
    buffer.cursor.col = 1;
    buffer.insert_str("x");
    runtime.note_buffer_change(1, &mut buffer);

    let view = runtime.render_snapshot_for(1, &buffer);
    let map = view.line_map.expect("line map");
    assert_eq!(map.len(), 4);
    assert_eq!(map[0], Some(0));
    assert_eq!(map[1], None);
    assert_eq!(map[2], Some(2));
    assert_eq!(map[3], None);
}

#[test]
fn render_snapshot_maps_shifted_lines() {
    let (mut buffer, mut runtime) = setup_runtime(2);

    buffer.cursor.line = 0;
    buffer.cursor.col = 0;
    buffer.insert_str("\n");
    runtime.note_buffer_change(2, &mut buffer);

    let view = runtime.render_snapshot_for(2, &buffer);
    let map = view.line_map.expect("line map");
    assert_eq!(map.len(), 5);
    assert_eq!(map[0], None);
    assert_eq!(map[1], None);
    assert_eq!(map[2], Some(1));
    assert_eq!(map[3], Some(2));
    assert_eq!(map[4], None);
}

#[test]
fn render_snapshot_allows_reuse_during_full_reparse() {
    let (buffer, mut runtime) = setup_runtime(3);

    runtime.full_reparse_pending.insert(3, true);
    let view = runtime.render_snapshot_for(3, &buffer);
    assert!(view.snapshot.is_some());
}

#[test]
fn render_snapshot_reuses_shifted_lines_on_full_reparse() {
    let (mut buffer, mut runtime) = setup_runtime(4);

    buffer.cursor.line = 0;
    buffer.cursor.col = 0;
    buffer.insert_str("\n");
    buffer.take_pending_edits();
    runtime.full_reparse_pending.insert(4, true);

    let view = runtime.render_snapshot_for(4, &buffer);
    let map = view.line_map.expect("line map");
    assert_eq!(map.len(), 5);
    assert_eq!(map[0], None);
    assert_eq!(map[1], Some(0));
    assert_eq!(map[2], Some(1));
    assert_eq!(map[3], Some(2));
    assert_eq!(map[4], None);
}
