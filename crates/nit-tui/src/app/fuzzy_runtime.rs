use std::path::{Path, PathBuf};

use nit_core::{AppState, Buffer, SearchMode};

use crate::{
    fuzzy_preview_runner::{PreviewModel, PreviewRunner},
    fuzzy_search_runner::{ContentSearchRunner, FileIndexRunner, FuzzyCommand, FuzzyMatcherRunner},
    theme::Theme,
};

use super::*;

pub(super) struct FuzzySearchRuntime {
    pub(super) indexer: FileIndexRunner,
    pub(super) fuzzy: FuzzyMatcherRunner,
    pub(super) content: ContentSearchRunner,
    pub(super) preview: PreviewRunner,

    pub(super) index_gen: u64,
    pub(super) file_gen: u64,
    pub(super) content_gen: u64,
    pub(super) preview_gen: u64,

    pub(super) index_ready: bool,
    pub(super) index_filters: Option<(bool, bool)>,

    pub(super) preview_model: Option<PreviewModel>,
    pub(super) last_preview_key: Option<PreviewKey>,
    pub(super) preview_scroll_delta: i32,
    pub(super) last_open: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct PreviewKey {
    pub(super) mode: SearchMode,
    pub(super) path: PathBuf,
    pub(super) line_hint: usize,
    pub(super) query: String,
}

impl FuzzySearchRuntime {
    pub(super) fn new(theme: &Theme, highlight: nit_core::HighlightConfig) -> Self {
        Self {
            indexer: FileIndexRunner::spawn(),
            fuzzy: FuzzyMatcherRunner::spawn(),
            content: ContentSearchRunner::spawn(),
            preview: PreviewRunner::spawn(theme.clone(), highlight),
            index_gen: 0,
            file_gen: 0,
            content_gen: 0,
            preview_gen: 0,
            index_ready: false,
            index_filters: None,
            preview_model: None,
            last_preview_key: None,
            preview_scroll_delta: 0,
            last_open: false,
        }
    }

    pub(super) fn shutdown(&mut self) {
        self.indexer.shutdown();
        self.fuzzy.shutdown();
        self.content.shutdown();
        self.preview.shutdown();
    }

    pub(super) fn update_syntax_config(&self, highlight: nit_core::HighlightConfig) {
        self.preview.update_config(highlight);
    }

    pub(super) fn tick_open(&mut self, state: &mut AppState) {
        let open_now = state.fuzzy_search.open;
        if open_now == self.last_open {
            return;
        }
        self.last_open = open_now;
        self.preview_model = None;
        self.last_preview_key = None;
        self.preview_scroll_delta = 0;
        if open_now {
            self.ensure_index(state);
            self.run_search_for_mode(state);
            self.request_preview_for_selection(state);
            return;
        }
        // Closing: bump generation counters so any in-flight responses are
        // ignored, drain status flags, and quickly cancel content search.
        self.file_gen = self.file_gen.wrapping_add(1);
        self.preview_gen = self.preview_gen.wrapping_add(1);
        self.fuzzy.send(FuzzyCommand::Query {
            generation: 0,
            query: String::new(),
        });
        state.fuzzy_search.status_msg.clear();
        state.fuzzy_search.indexing = false;
        state.fuzzy_search.searching = false;
        self.content_gen = self.content_gen.wrapping_add(1);
        self.content.search(
            self.content_gen,
            state.workspace_root.clone(),
            String::new(),
            state.fuzzy_search.show_hidden,
            state.fuzzy_search.show_ignored,
        );
    }

    pub(super) fn ensure_index(&mut self, state: &mut AppState) {
        let filters = (
            state.fuzzy_search.show_hidden,
            state.fuzzy_search.show_ignored,
        );
        let needs = !self.index_ready || self.index_filters != Some(filters);
        if !needs {
            return;
        }
        self.index_filters = Some(filters);
        self.index_ready = false;
        self.index_gen = self.index_gen.wrapping_add(1);
        state.fuzzy_search.indexing = true;
        state.fuzzy_search.status_msg = "Indexing…".into();
        self.preview_model = None;
        self.preview_scroll_delta = 0;

        self.fuzzy.send(FuzzyCommand::ResetIndex {
            generation: self.index_gen,
            root: state.workspace_root.clone(),
        });
        self.indexer.build(
            self.index_gen,
            state.workspace_root.clone(),
            state.fuzzy_search.show_hidden,
            state.fuzzy_search.show_ignored,
        );
    }

    pub(super) fn rebuild_index(&mut self, state: &mut AppState) {
        self.index_ready = false;
        self.index_filters = None;
        self.ensure_index(state);
    }

    pub(super) fn run_search_for_mode(&mut self, state: &mut AppState) {
        match state.fuzzy_search.mode {
            SearchMode::Files => self.run_file_query(state),
            SearchMode::Content => self.run_content_query(state),
        }
    }

    pub(super) fn run_file_query(&mut self, state: &mut AppState) {
        self.file_gen = self.file_gen.wrapping_add(1);
        state.fuzzy_search.searching = false;
        state.fuzzy_search.file_results.clear();
        state.fuzzy_search.selected = 0;
        state.fuzzy_search.scroll_offset = 0;
        self.fuzzy.send(FuzzyCommand::Query {
            generation: self.file_gen,
            query: state.fuzzy_search.query.clone(),
        });
    }

    pub(super) fn run_content_query(&mut self, state: &mut AppState) {
        self.content_gen = self.content_gen.wrapping_add(1);
        state.fuzzy_search.match_results.clear();
        state.fuzzy_search.selected = 0;
        state.fuzzy_search.scroll_offset = 0;
        let query = state.fuzzy_search.query.trim().to_string();
        if query.is_empty() {
            state.fuzzy_search.searching = false;
            state.fuzzy_search.status_msg = "Type to search".into();
            self.content.search(
                self.content_gen,
                state.workspace_root.clone(),
                String::new(),
                state.fuzzy_search.show_hidden,
                state.fuzzy_search.show_ignored,
            );
            return;
        }
        state.fuzzy_search.searching = true;
        state.fuzzy_search.status_msg = "Searching…".into();
        self.content.search(
            self.content_gen,
            state.workspace_root.clone(),
            query,
            state.fuzzy_search.show_hidden,
            state.fuzzy_search.show_ignored,
        );
    }

    pub(super) fn request_preview_for_selection(&mut self, state: &AppState) {
        if !state.fuzzy_search.open {
            return;
        }
        let (path, line_hint, query) = match state.fuzzy_search.mode {
            SearchMode::Files => {
                let Some(item) = state
                    .fuzzy_search
                    .file_results
                    .get(state.fuzzy_search.selected)
                else {
                    return;
                };
                (item.abs_path.clone(), None, String::new())
            }
            SearchMode::Content => {
                let Some(item) = state
                    .fuzzy_search
                    .match_results
                    .get(state.fuzzy_search.selected)
                else {
                    return;
                };
                (
                    item.abs_path.clone(),
                    Some(item.line),
                    state.fuzzy_search.query.trim().to_string(),
                )
            }
        };
        let key = PreviewKey {
            mode: state.fuzzy_search.mode,
            path: path.clone(),
            line_hint: line_hint.unwrap_or(0),
            query: if matches!(state.fuzzy_search.mode, SearchMode::Content) {
                query.clone()
            } else {
                String::new()
            },
        };
        if self.last_preview_key.as_ref() == Some(&key) {
            return;
        }
        let override_content = dirty_buffer_override(&state.buffers, &path);
        self.preview_scroll_delta = 0;
        self.last_preview_key = Some(key);
        self.preview_gen = self.preview_gen.wrapping_add(1);
        self.preview.request(
            self.preview_gen,
            state.fuzzy_search.mode,
            path,
            line_hint,
            query,
            override_content,
        );
    }
}

/// Live unsaved-buffer content for `path`, so previews reflect edits not disk.
pub(crate) fn dirty_buffer_override(buffers: &[Buffer], path: &Path) -> Option<String> {
    let target = std::fs::canonicalize(path).ok();
    buffers.iter().find_map(|buf| {
        if !buf.is_dirty() {
            return None;
        }
        let buf_path = buf.path()?;
        let same = match (&target, std::fs::canonicalize(buf_path).ok()) {
            (Some(t), Some(b)) => t.as_path() == b.as_path(),
            // Unsaved buffer with no on-disk twin: compare lexically instead.
            _ => buf_path.as_path() == path,
        };
        same.then(|| buf.content_as_string())
    })
}
