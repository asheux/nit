//! Shared `#[cfg(test)]` fixtures for the centralized files in
//! `crates/nit-core/src/tests/`.
//!
//! Existing test files still inline their local helpers; new tests should
//! reach for these so the canonical setup lives in one place.

#![allow(dead_code)]

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::buffer::Buffer;
use crate::state::AppState;

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

pub(crate) fn temp_dir(label: &str) -> PathBuf {
    let pid = std::process::id();
    let n = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("nit-core-test-{label}-{pid}-{n}"));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

pub(crate) fn test_state() -> AppState {
    let editor = Buffer::from_str("editor", "", None);
    let notes = Buffer::from_str("notes", "", None);
    AppState::new(PathBuf::from("."), editor, notes)
}

pub(crate) fn test_state_in(root: PathBuf) -> AppState {
    let editor = Buffer::from_str("editor", "", None);
    let notes = Buffer::from_str("notes", "", None);
    AppState::new(root, editor, notes)
}
