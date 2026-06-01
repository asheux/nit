use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use nit_core::Buffer;

use crate::app::dirty_buffer_override;

use super::*;

fn temp_file(label: &str, contents: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "nit-fuzzy-preview-{label}-{}-{nanos}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let path = dir.join("sample.rs");
    std::fs::write(&path, contents).expect("write file");
    path
}

fn line_text(line: &Line<'_>) -> String {
    line.spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect()
}

fn recv_model(rx: &Receiver<PreviewEvent>) -> PreviewModel {
    match rx.recv().expect("preview event") {
        PreviewEvent::Ready { model, .. } => model,
        PreviewEvent::Error { message, .. } => panic!("unexpected preview error: {message}"),
    }
}

fn plain_build(path: &Path, override_content: Option<&str>) -> PreviewModel {
    let theme = Theme::default();
    let mut manager = SyntaxManager::new(to_syntax_config(HighlightConfig::default()));
    let mut version = 0u64;
    build_preview(
        path,
        SearchMode::Files,
        None,
        "",
        &theme,
        &mut manager,
        &mut version,
        false,
        override_content,
    )
    .expect("build preview")
}

#[test]
fn window_from_str_windows_from_start_line() {
    let (lines, truncated) = window_from_str("a\nb\nc\nd", 2, 2);
    assert_eq!(lines, vec!["b".to_string(), "c".to_string()]);
    assert!(!truncated);
}

#[test]
fn window_from_str_caps_long_line_on_char_boundary() {
    // 3-byte glyphs land the byte cap mid-codepoint, exercising the boundary walk.
    let long = "→".repeat(PREVIEW_MAX_LINE_BYTES);
    let (lines, truncated) = window_from_str(&long, 1, 4);
    assert!(truncated);
    assert!(lines[0].len() <= PREVIEW_MAX_LINE_BYTES);
}

#[test]
fn dirty_buffer_override_returns_unsaved_content() {
    let path = temp_file("dirty", "DISK CONTENT\n");
    let mut buf = Buffer::from_str("sample", "", Some(path.clone()));
    buf.insert_str("LIVE EDIT\nsecond line");
    assert!(buf.is_dirty());

    let got = dirty_buffer_override(&[buf], &path);
    assert_eq!(got.as_deref(), Some("LIVE EDIT\nsecond line"));
}

#[test]
fn dirty_buffer_override_skips_clean_buffer() {
    let path = temp_file("clean", "DISK CONTENT\n");
    let buf = Buffer::from_str("sample", "DISK CONTENT\n", Some(path.clone()));
    assert!(!buf.is_dirty());

    assert!(dirty_buffer_override(&[buf], &path).is_none());
}

#[test]
fn dirty_buffer_override_skips_unrelated_path() {
    let target = temp_file("target", "DISK\n");
    let other = temp_file("other", "OTHER\n");
    let mut buf = Buffer::from_str("other", "", Some(other));
    buf.insert_str("unsaved elsewhere");

    assert!(dirty_buffer_override(&[buf], &target).is_none());
}

#[test]
fn build_preview_prefers_override_else_disk() {
    let path = temp_file("render", "DISK ONE\nDISK TWO\n");

    let live = plain_build(&path, Some("LIVE ONE\nLIVE TWO"));
    assert_eq!(line_text(&live.lines[0]), "LIVE ONE");
    assert_eq!(line_text(&live.lines[1]), "LIVE TWO");

    let from_disk = plain_build(&path, None);
    assert_eq!(line_text(&from_disk.lines[0]), "DISK ONE");
    assert_eq!(line_text(&from_disk.lines[1]), "DISK TWO");
}

#[test]
fn override_is_not_pinned_in_cache() {
    let path = temp_file("cache", "DISK CONTENT\n");
    let theme = Theme::default();
    let mut manager = SyntaxManager::new(to_syntax_config(HighlightConfig::default()));
    let mut version = 0u64;
    let mut cache = PreviewCache::new();
    let (tx, rx) = unbounded();

    let live = PendingRequest {
        generation: 1,
        mode: SearchMode::Files,
        path: path.clone(),
        line_hint: None,
        query: String::new(),
        override_content: Some("LIVE BUFFER".to_string()),
    };
    serve_request(
        live,
        &theme,
        &mut manager,
        &mut version,
        &mut cache,
        false,
        &tx,
    );
    let model = recv_model(&rx);
    assert_eq!(line_text(&model.lines[0]), "LIVE BUFFER");
    // Without this guard the override would outlive the dirty buffer and keep
    // shadowing disk after a save; the empty cache proves it was not stored.
    assert!(cache.entries.is_empty());

    let clean = PendingRequest {
        generation: 2,
        mode: SearchMode::Files,
        path,
        line_hint: None,
        query: String::new(),
        override_content: None,
    };
    serve_request(
        clean,
        &theme,
        &mut manager,
        &mut version,
        &mut cache,
        false,
        &tx,
    );
    let model = recv_model(&rx);
    assert_eq!(line_text(&model.lines[0]), "DISK CONTENT");
}
