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

#[test]
fn idle_poll_preserves_cached_line_map_while_edit_is_pending() {
    let mut buffer = Buffer::from_str("test", "a\nb\nc\n", None);
    let mut runtime = SyntaxRuntime::new(default_config());
    runtime.prime_buffer(1, &buffer, true);
    buffer.insert_char('x');
    runtime.note_buffer_change(1, &mut buffer);
    assert!(runtime.render_snapshot_for(1, &buffer).line_map.is_some());
    runtime.poll_results(1, buffer.version());
    assert!(runtime.render_cache.contains_key(&1));
}

#[test]
fn active_documents_are_primed_after_command_open_and_slot_replacement() {
    use nit_core::{apply_action, Action, AppState};
    use nit_syntax::LanguageId;
    let root = std::env::temp_dir().join(format!("nit-syntax-switch-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let file = root.join("review.py");
    std::fs::write(&file, "def review():\n    return 42\n").unwrap();
    let editor = Buffer::from_str("review.rs", "fn review() {}", Some(root.join("review.rs")));
    let mut state = AppState::new(root.clone(), editor, Buffer::empty("notes", None));
    let mut cfg = default_config();
    cfg.engine = nit_core::HighlightEngine::TreeSitter;
    let mut runtime = SyntaxRuntime::new(cfg);
    let first = state.active_editor_buffer_id;
    runtime.prime_buffer(first, state.editor_buffer(), true);
    apply_action(&mut state, Action::CommandPromptOpen);
    state.command_line.as_mut().unwrap().input = "e review.py".into();
    apply_action(&mut state, Action::CommandPromptExecute);
    let second = state.active_editor_buffer_id;
    assert_ne!(first, second);
    fn wait(runtime: &mut SyntaxRuntime, id: usize, buf: &Buffer, language: LanguageId) {
        runtime.tick(id, buf);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(4);
        loop {
            runtime.poll_results(id, buf.version());
            if let Some(snap) = runtime.snapshot_for(id, buf.version()) {
                assert_eq!(snap.language, language);
                assert!(!snap.per_line.is_empty());
                break;
            }
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }
    wait(
        &mut runtime,
        second,
        state.editor_buffer(),
        LanguageId::Python,
    );
    wait(&mut runtime, first, &state.buffers[first], LanguageId::Rust);
    let previous = runtime.documents[&first].origin;
    runtime.prime_buffer(first, &state.buffers[first], false); // An outstanding older request.
    let replacement = Buffer::from_str("new.py", "class New: pass", Some(file));
    assert_eq!(state.buffers[first].version(), replacement.version());
    assert!(runtime
        .render_snapshot_for(first, &replacement)
        .snapshot
        .is_none());
    wait(&mut runtime, first, &replacement, LanguageId::Python);
    assert!(runtime.documents[&first].origin > previous);
    std::fs::remove_dir_all(root).unwrap();
}
