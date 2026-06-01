#![allow(clippy::too_many_arguments)]
use std::path::{Path, PathBuf};

use arboard::Clipboard;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use nit_core::state::file_tree::{FileTreePrompt, FileTreePromptKind};
use nit_core::{actions::Action, AppState, FileTreeKind, Mode, PaneId, SearchMode, YankKind};

use crate::{
    file_tree,
    file_tree_runner::{FileTreeCommand, FileTreeMutation, FileTreeRunner},
    file_watcher::FileWatcher,
    fuzzy_preview_runner::PreviewEvent,
    fuzzy_search_runner::{ContentEvent, FuzzyEvent, IndexEvent},
    widgets::file_tree_view,
};

use super::*;

pub(super) fn file_tree_tick(state: &mut AppState, runner: &FileTreeRunner) -> bool {
    if !state.file_tree.open {
        return false;
    }
    if dispatch_submitted_prompt(state, runner) {
        return true;
    }

    let preserve = file_tree::selected_path(state);
    let mut requested = false;
    for dir in file_tree::needed_dirs(state) {
        if state.file_tree.cache.contains_key(&dir) || state.file_tree.loading_dirs.contains(&dir) {
            continue;
        }
        state.file_tree.loading_dirs.insert(dir.clone());
        runner.send(FileTreeCommand::ListDir {
            dir,
            show_hidden: state.file_tree.show_hidden,
            show_ignored: state.file_tree.show_ignored,
        });
        requested = true;
    }

    if requested || state.file_tree.rows.is_empty() {
        file_tree::rebuild_view(state, Some(preserve));
        return true;
    }
    false
}

pub(super) fn handle_fuzzy_index_event(
    state: &mut AppState,
    runtime: &mut FuzzySearchRuntime,
    event: IndexEvent,
) {
    let open = state.fuzzy_search.open;
    match event {
        IndexEvent::Started { generation } => {
            if generation != runtime.index_gen {
                return;
            }
            if open {
                state.fuzzy_search.indexing = true;
                state.fuzzy_search.status_msg = "Indexing…".into();
            }
        }
        IndexEvent::Batch {
            generation,
            files,
            total_indexed,
        } => {
            if generation != runtime.index_gen {
                return;
            }
            if open {
                state.fuzzy_search.indexing = true;
                state.fuzzy_search.status_msg = format!("Indexing… ({total_indexed} files)");
            }
            runtime
                .fuzzy
                .send(FuzzyCommand::IndexBatch { generation, files });
        }
        IndexEvent::Done {
            generation,
            total_files,
            duration_ms,
        } => {
            if generation != runtime.index_gen {
                return;
            }
            runtime.index_ready = true;
            runtime.fuzzy.send(FuzzyCommand::IndexDone { generation });
            if open {
                state.fuzzy_search.indexing = false;
                if state.fuzzy_search.mode == SearchMode::Files
                    && state.fuzzy_search.query.is_empty()
                {
                    state.fuzzy_search.status_msg = format!("{total_files} files");
                } else if !state.fuzzy_search.searching {
                    state.fuzzy_search.status_msg =
                        format!("Indexed {total_files} files in {duration_ms}ms");
                }
            }
        }
        IndexEvent::Error {
            generation,
            message,
        } => {
            if generation != runtime.index_gen {
                return;
            }
            runtime.index_ready = false;
            if open {
                state.fuzzy_search.indexing = false;
                state.fuzzy_search.status_msg = format!("Index error: {message}");
                state.status = Some(format!("Search index error: {message}"));
            }
        }
    }
}

pub(super) fn handle_fuzzy_file_event(
    state: &mut AppState,
    runtime: &mut FuzzySearchRuntime,
    event: FuzzyEvent,
) {
    if !state.fuzzy_search.open {
        return;
    }
    match event {
        FuzzyEvent::ResultsReplace {
            generation,
            results,
            total_indexed,
            total_matches,
            duration_ms,
        } => {
            if generation != runtime.file_gen {
                return;
            }
            state.fuzzy_search.file_results = results;
            let len = state.fuzzy_search.file_results.len();
            state.fuzzy_search.selected = state.fuzzy_search.selected.min(len.saturating_sub(1));
            state.fuzzy_search.scroll_offset = state.fuzzy_search.scroll_offset.min(len);
            if state.fuzzy_search.mode == SearchMode::Files {
                if state.fuzzy_search.query.is_empty() {
                    if state.fuzzy_search.indexing {
                        state.fuzzy_search.status_msg =
                            format!("Indexing… ({total_indexed} files)");
                    } else {
                        state.fuzzy_search.status_msg = format!("{total_indexed} files");
                    }
                } else {
                    state.fuzzy_search.status_msg =
                        format!("{total_matches} matches (showing {len}) · {duration_ms}ms");
                }
                runtime.request_preview_for_selection(state);
            }
        }
        FuzzyEvent::ResultsAppend {
            generation,
            results,
            total_indexed,
        } => {
            if generation != runtime.file_gen {
                return;
            }
            state.fuzzy_search.file_results.extend(results);
            if state.fuzzy_search.mode == SearchMode::Files {
                if state.fuzzy_search.indexing {
                    state.fuzzy_search.status_msg = format!("Indexing… ({total_indexed} files)");
                } else if state.fuzzy_search.query.is_empty() {
                    state.fuzzy_search.status_msg = format!("{total_indexed} files");
                }
                runtime.request_preview_for_selection(state);
            }
        }
    }
}

pub(super) fn handle_fuzzy_content_event(
    state: &mut AppState,
    runtime: &mut FuzzySearchRuntime,
    event: ContentEvent,
) {
    if !state.fuzzy_search.open {
        return;
    }
    match event {
        ContentEvent::Started { generation } => {
            if generation != runtime.content_gen {
                return;
            }
            state.fuzzy_search.searching = true;
            state.fuzzy_search.status_msg = "Searching…".into();
        }
        ContentEvent::MatchBatch {
            generation,
            results,
        } => {
            if generation != runtime.content_gen {
                return;
            }
            state.fuzzy_search.match_results.extend(results);
            if state.fuzzy_search.mode == SearchMode::Content {
                state.fuzzy_search.status_msg = format!(
                    "Searching… ({} matches)",
                    state.fuzzy_search.match_results.len()
                );
                runtime.request_preview_for_selection(state);
            }
        }
        ContentEvent::Done {
            generation,
            total_matches,
            duration_ms,
        } => {
            if generation != runtime.content_gen {
                return;
            }
            state.fuzzy_search.searching = false;
            if state.fuzzy_search.mode == SearchMode::Content {
                state.fuzzy_search.status_msg =
                    format!("{total_matches} matches · {duration_ms}ms");
                runtime.request_preview_for_selection(state);
            }
        }
        ContentEvent::Error {
            generation,
            message,
        } => {
            if generation != runtime.content_gen {
                return;
            }
            state.fuzzy_search.searching = false;
            state.fuzzy_search.status_msg = format!("Search error: {message}");
            state.status = Some(format!("Search error: {message}"));
        }
    }
}

pub(super) fn handle_fuzzy_preview_event(
    state: &mut AppState,
    runtime: &mut FuzzySearchRuntime,
    event: PreviewEvent,
) {
    if !state.fuzzy_search.open {
        return;
    }
    match event {
        PreviewEvent::Ready { generation, model } => {
            if generation != runtime.preview_gen {
                return;
            }
            runtime.preview_model = Some(model);
        }
        PreviewEvent::Error {
            generation,
            message,
        } => {
            if generation != runtime.preview_gen {
                return;
            }
            runtime.preview_model = None;
            tracing::debug!("preview error: {message}");
        }
    }
}

pub(super) fn handle_file_tree_key(
    key: &KeyEvent,
    state: &mut AppState,
    syntax: &mut SyntaxRuntime,
    editor_area: ratatui::layout::Rect,
) -> bool {
    if !state.file_tree.open {
        return false;
    }
    // When the substrate overlay is open, all keys (including Esc) should
    // route to the overlay handler, not to the file tree. Otherwise Esc
    // would close the file tree instead of the overlay.
    if state.show_substrate_overlay {
        return false;
    }
    if is_global_quit_key(key) {
        return false;
    }
    // While the inline name prompt is open every other key feeds the input
    // (so typing `r`/`q`/`:` edits the name instead of firing tree actions).
    if state.file_tree.prompt.is_some() {
        return handle_file_tree_prompt_key(key, state);
    }
    if is_petri_show_key(key, state) {
        return false;
    }
    if ctrl_nav_dir(key).is_some() {
        return false;
    }
    if is_command_prompt_open_key(key)
        || is_help_toggle_key(key)
        || is_games_history_open_key(key, state)
    {
        return false;
    }

    if matches!(
        key,
        KeyEvent {
            code: KeyCode::Char('t'),
            modifiers,
            ..
        } if modifiers.contains(KeyModifiers::CONTROL)
    ) {
        state.file_tree.open = false;
        return true;
    }

    match key.code {
        KeyCode::Esc | KeyCode::Char('q') => {
            state.file_tree.open = false;
            true
        }
        KeyCode::Char('R') => {
            file_tree::clear_cache(state);
            state.status = Some("NITTree refreshed".into());
            true
        }
        KeyCode::Char('r') => {
            open_file_tree_prompt(state, FileTreePromptKind::Rename);
            true
        }
        KeyCode::Char('n') => {
            open_file_tree_prompt(state, FileTreePromptKind::NewFile);
            true
        }
        KeyCode::Char('N') => {
            open_file_tree_prompt(state, FileTreePromptKind::NewDir);
            true
        }
        KeyCode::Char('.') => {
            state.file_tree.show_hidden = !state.file_tree.show_hidden;
            file_tree::clear_cache(state);
            state.status = Some(if state.file_tree.show_hidden {
                "NITTree: hidden files ON".into()
            } else {
                "NITTree: hidden files OFF".into()
            });
            true
        }
        KeyCode::Char('i') => {
            state.file_tree.show_ignored = !state.file_tree.show_ignored;
            file_tree::clear_cache(state);
            state.status = Some(if state.file_tree.show_ignored {
                "NITTree: ignored files ON".into()
            } else {
                "NITTree: ignored files OFF".into()
            });
            true
        }
        KeyCode::Enter => {
            let Some(row) = state.file_tree.rows.get(state.file_tree.selected) else {
                return true;
            };
            match row.kind {
                nit_core::FileTreeKind::File => {
                    let path = row.path.clone();
                    let _ = apply_action_with_syntax(state, syntax, Action::OpenFile(path));
                    state.file_tree.open = false;
                    true
                }
                nit_core::FileTreeKind::Dir => {
                    let path = row.path.clone();
                    if state.file_tree.expanded_dirs.contains(&path) {
                        // Collapse the directory and any expanded descendants to avoid
                        // background work for items that are no longer visible.
                        state
                            .file_tree
                            .expanded_dirs
                            .retain(|p| !p.starts_with(&path));
                    } else {
                        state.file_tree.expanded_dirs.insert(path.clone());
                    }
                    file_tree::rebuild_view(state, Some(path));
                    adjust_file_tree_scroll(state, editor_area);
                    true
                }
                nit_core::FileTreeKind::Loading => true,
            }
        }
        KeyCode::Up
        | KeyCode::Char('k')
        | KeyCode::Down
        | KeyCode::Char('j')
        | KeyCode::PageUp
        | KeyCode::PageDown
        | KeyCode::Home
        | KeyCode::End => {
            let inner_height = editor_area.height.saturating_sub(2) as usize;
            let page = inner_height.max(1);
            let len = state.file_tree.rows.len();
            if len == 0 {
                return true;
            }
            match key.code {
                KeyCode::Up | KeyCode::Char('k') => {
                    state.file_tree.selected = state.file_tree.selected.saturating_sub(1);
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    state.file_tree.selected = (state.file_tree.selected + 1).min(len - 1);
                }
                KeyCode::PageUp => {
                    state.file_tree.selected = state.file_tree.selected.saturating_sub(page);
                }
                KeyCode::PageDown => {
                    state.file_tree.selected = (state.file_tree.selected + page).min(len - 1);
                }
                KeyCode::Home => {
                    state.file_tree.selected = 0;
                }
                KeyCode::End => {
                    state.file_tree.selected = len - 1;
                }
                _ => {}
            }
            adjust_file_tree_scroll(state, editor_area);
            true
        }
        _ => true,
    }
}

fn open_file_tree_prompt(state: &mut AppState, kind: FileTreePromptKind) {
    let root = state.file_tree.root.clone();
    let selected = state
        .file_tree
        .rows
        .get(state.file_tree.selected)
        .filter(|r| matches!(r.kind, FileTreeKind::File | FileTreeKind::Dir))
        .map(|r| (r.path.clone(), matches!(r.kind, FileTreeKind::Dir)));

    let (target_dir, source) = match kind {
        FileTreePromptKind::Rename => {
            let Some((path, _)) = selected else {
                state.status = Some("NITTree: nothing to rename".into());
                return;
            };
            let parent = path
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| root.clone());
            (parent, Some(path))
        }
        // Create inside a selected directory, alongside a selected file, or in
        // the tree root when nothing is selected.
        _ => {
            let dir = match selected {
                Some((path, true)) => path,
                Some((path, false)) => path
                    .parent()
                    .map(Path::to_path_buf)
                    .unwrap_or_else(|| root.clone()),
                None => root.clone(),
            };
            (dir, None)
        }
    };

    state.file_tree.prompt = Some(FileTreePrompt {
        kind,
        input: String::new(),
        target_dir,
        source,
        submitted: false,
    });
}

fn handle_file_tree_prompt_key(key: &KeyEvent, state: &mut AppState) -> bool {
    match key.code {
        KeyCode::Esc => state.file_tree.prompt = None,
        KeyCode::Enter => submit_file_tree_prompt(state),
        KeyCode::Backspace => {
            if let Some(prompt) = state.file_tree.prompt.as_mut() {
                prompt.input.pop();
            }
        }
        KeyCode::Char(c) if !c.is_control() && !key.modifiers.contains(KeyModifiers::CONTROL) => {
            if let Some(prompt) = state.file_tree.prompt.as_mut() {
                prompt.input.push(c);
            }
        }
        _ => {}
    }
    true
}

// Validates the typed name at submit time; on success flags the prompt so the
// per-frame tick dispatches it, otherwise reports the refusal and leaves the
// prompt open for another attempt.
fn submit_file_tree_prompt(state: &mut AppState) {
    let Some((name, target)) = state.file_tree.prompt.as_ref().map(|p| {
        let name = p.input.trim().to_string();
        let target = p.target_dir.join(&name);
        (name, target)
    }) else {
        return;
    };
    let rejection = if !nit_utils::paths::is_safe_leaf_name(&name) {
        Some(format!("invalid name '{name}'"))
    } else if target.exists() {
        Some(format!("'{name}' already exists"))
    } else if !nit_utils::paths::path_within(&state.file_tree.root, &target) {
        Some(format!("'{name}' escapes the workspace"))
    } else {
        None
    };
    match rejection {
        Some(message) => state.status = Some(format!("NITTree: {message}")),
        None => {
            if let Some(prompt) = state.file_tree.prompt.as_mut() {
                prompt.submitted = true;
            }
        }
    }
}

fn dispatch_submitted_prompt(state: &mut AppState, runner: &FileTreeRunner) -> bool {
    if !matches!(state.file_tree.prompt.as_ref(), Some(p) if p.submitted) {
        return false;
    }
    let prompt = state
        .file_tree
        .prompt
        .take()
        .expect("submitted prompt present");
    let name = prompt.input.trim().to_string();
    let target = prompt.target_dir.join(&name);
    let op = match (prompt.kind, prompt.source) {
        (FileTreePromptKind::Rename, Some(from)) => FileTreeMutation::Rename { from, to: target },
        (FileTreePromptKind::Rename, None) => return true,
        (FileTreePromptKind::NewFile, _) => FileTreeMutation::CreateFile { path: target },
        (FileTreePromptKind::NewDir, _) => FileTreeMutation::CreateDir { path: target },
    };
    runner.send(FileTreeCommand::Mutate {
        workspace_root: state.file_tree.root.clone(),
        parent: prompt.target_dir,
        op,
        show_hidden: state.file_tree.show_hidden,
        show_ignored: state.file_tree.show_ignored,
    });
    let verb = match prompt.kind {
        FileTreePromptKind::Rename => "rename",
        FileTreePromptKind::NewFile => "new file",
        FileTreePromptKind::NewDir => "new dir",
    };
    state.status = Some(format!("NITTree: {verb} {name}"));
    true
}

pub(super) fn handle_fuzzy_search_key(
    key: &KeyEvent,
    state: &mut AppState,
    syntax: &mut SyntaxRuntime,
    runtime: &mut FuzzySearchRuntime,
    screen: ratatui::layout::Rect,
) -> bool {
    if !state.fuzzy_search.open {
        return false;
    }
    if is_global_quit_key(key) {
        return false;
    }
    // Allow global pause/resume while the modal is open.
    if is_job_pause_key(key) {
        return false;
    }

    let popup = dynamic_popup_rect(screen, fuzzy_popup_size(screen, state));
    let list_height = popup
        .height
        .saturating_sub(6) // outer(2) + header/footer(2) + results block(2)
        .max(1) as usize;
    let preview_page = ((list_height as i32) / 2).max(1);

    match key {
        KeyEvent {
            code: KeyCode::Esc, ..
        } => {
            state.fuzzy_search.close();
            runtime.preview_model = None;
            runtime.last_preview_key = None;
            true
        }
        KeyEvent {
            code: KeyCode::Enter | KeyCode::Char('\n') | KeyCode::Char('\r'),
            modifiers,
            ..
        } if modifiers.is_empty() => match state.fuzzy_search.mode {
            SearchMode::Files => {
                let Some(item) = state
                    .fuzzy_search
                    .file_results
                    .get(state.fuzzy_search.selected)
                else {
                    return true;
                };
                let path = item.abs_path.clone();
                let _ = apply_action_with_syntax(state, syntax, Action::OpenFile(path));
                state.file_tree.open = false;
                state.fuzzy_search.close();
                runtime.preview_model = None;
                runtime.last_preview_key = None;
                true
            }
            SearchMode::Content => {
                let Some(item) = state
                    .fuzzy_search
                    .match_results
                    .get(state.fuzzy_search.selected)
                else {
                    return true;
                };
                let path = item.abs_path.clone();
                let line = item.line;
                let col = item.col;
                let _ = apply_action_with_syntax(state, syntax, Action::OpenFile(path));
                {
                    let buf = state.editor_buffer_mut();
                    let total = buf.lines_len().max(1);
                    let target_line = line.saturating_sub(1).min(total.saturating_sub(1));
                    buf.cursor.line = target_line;
                    buf.cursor.col = col.saturating_sub(1);
                    buf.ensure_visible();
                }
                state.file_tree.open = false;
                state.fuzzy_search.close();
                runtime.preview_model = None;
                runtime.last_preview_key = None;
                true
            }
        },
        KeyEvent {
            code: KeyCode::Tab, ..
        } => {
            state.fuzzy_search.mode = match state.fuzzy_search.mode {
                SearchMode::Files => SearchMode::Content,
                SearchMode::Content => SearchMode::Files,
            };
            state.fuzzy_search.selected = 0;
            state.fuzzy_search.scroll_offset = 0;
            runtime.run_search_for_mode(state);
            runtime.preview_model = None;
            runtime.last_preview_key = None;
            true
        }
        KeyEvent {
            code: KeyCode::F(2),
            ..
        } => {
            state.fuzzy_search.show_hidden = !state.fuzzy_search.show_hidden;
            runtime.rebuild_index(state);
            runtime.run_search_for_mode(state);
            runtime.preview_model = None;
            runtime.last_preview_key = None;
            true
        }
        KeyEvent {
            code: KeyCode::Char('.'),
            modifiers,
            ..
        } if modifiers.contains(KeyModifiers::CONTROL) => {
            state.fuzzy_search.show_hidden = !state.fuzzy_search.show_hidden;
            runtime.rebuild_index(state);
            runtime.run_search_for_mode(state);
            runtime.preview_model = None;
            runtime.last_preview_key = None;
            true
        }
        KeyEvent {
            code: KeyCode::Char('\u{1e}'),
            modifiers,
            ..
        } if modifiers.is_empty() => {
            state.fuzzy_search.show_hidden = !state.fuzzy_search.show_hidden;
            runtime.rebuild_index(state);
            runtime.run_search_for_mode(state);
            runtime.preview_model = None;
            runtime.last_preview_key = None;
            true
        }
        KeyEvent {
            code: KeyCode::F(3),
            ..
        } => {
            state.fuzzy_search.show_ignored = !state.fuzzy_search.show_ignored;
            runtime.rebuild_index(state);
            runtime.run_search_for_mode(state);
            runtime.preview_model = None;
            runtime.last_preview_key = None;
            true
        }
        KeyEvent {
            code: KeyCode::Char('g') | KeyCode::Char('G'),
            modifiers,
            ..
        } if modifiers.contains(KeyModifiers::CONTROL) => {
            state.fuzzy_search.show_ignored = !state.fuzzy_search.show_ignored;
            runtime.rebuild_index(state);
            runtime.run_search_for_mode(state);
            runtime.preview_model = None;
            runtime.last_preview_key = None;
            true
        }
        KeyEvent {
            code: KeyCode::Char('\u{7}'),
            modifiers,
            ..
        } if modifiers.is_empty() => {
            state.fuzzy_search.show_ignored = !state.fuzzy_search.show_ignored;
            runtime.rebuild_index(state);
            runtime.run_search_for_mode(state);
            runtime.preview_model = None;
            runtime.last_preview_key = None;
            true
        }
        KeyEvent {
            code: KeyCode::F(5),
            ..
        } => {
            match state.fuzzy_search.mode {
                SearchMode::Files => {
                    runtime.rebuild_index(state);
                    runtime.run_file_query(state);
                }
                SearchMode::Content => {
                    runtime.run_content_query(state);
                }
            }
            runtime.preview_model = None;
            runtime.last_preview_key = None;
            true
        }
        KeyEvent {
            code: KeyCode::Char('r') | KeyCode::Char('R'),
            modifiers,
            ..
        } if modifiers.contains(KeyModifiers::CONTROL) => {
            match state.fuzzy_search.mode {
                SearchMode::Files => {
                    runtime.rebuild_index(state);
                    runtime.run_file_query(state);
                }
                SearchMode::Content => {
                    runtime.run_content_query(state);
                }
            }
            runtime.preview_model = None;
            runtime.last_preview_key = None;
            true
        }
        KeyEvent {
            code: KeyCode::Char('\u{12}'),
            modifiers,
            ..
        } if modifiers.is_empty() => {
            match state.fuzzy_search.mode {
                SearchMode::Files => {
                    runtime.rebuild_index(state);
                    runtime.run_file_query(state);
                }
                SearchMode::Content => {
                    runtime.run_content_query(state);
                }
            }
            runtime.preview_model = None;
            runtime.last_preview_key = None;
            true
        }
        KeyEvent {
            code: KeyCode::Backspace,
            modifiers,
            ..
        } => {
            if modifiers.contains(KeyModifiers::CONTROL) {
                delete_last_word(&mut state.fuzzy_search.query);
            } else {
                state.fuzzy_search.query.pop();
            }
            runtime.preview_model = None;
            runtime.last_preview_key = None;
            runtime.run_search_for_mode(state);
            true
        }
        KeyEvent {
            code: KeyCode::Char('u') | KeyCode::Char('U'),
            modifiers,
            ..
        } if modifiers.contains(KeyModifiers::CONTROL) => {
            runtime.preview_scroll_delta =
                runtime.preview_scroll_delta.saturating_sub(preview_page);
            true
        }
        KeyEvent {
            code: KeyCode::Char('\u{15}'),
            modifiers,
            ..
        } if modifiers.is_empty() => {
            runtime.preview_scroll_delta =
                runtime.preview_scroll_delta.saturating_sub(preview_page);
            true
        }
        KeyEvent {
            code: KeyCode::PageUp,
            modifiers,
            ..
        } if modifiers.contains(KeyModifiers::CONTROL) => {
            runtime.preview_scroll_delta =
                runtime.preview_scroll_delta.saturating_sub(preview_page);
            true
        }
        KeyEvent {
            code: KeyCode::Char('d') | KeyCode::Char('D'),
            modifiers,
            ..
        } if modifiers.contains(KeyModifiers::CONTROL) => {
            runtime.preview_scroll_delta =
                runtime.preview_scroll_delta.saturating_add(preview_page);
            true
        }
        KeyEvent {
            code: KeyCode::Char('\u{4}'),
            modifiers,
            ..
        } if modifiers.is_empty() => {
            runtime.preview_scroll_delta =
                runtime.preview_scroll_delta.saturating_add(preview_page);
            true
        }
        KeyEvent {
            code: KeyCode::PageDown,
            modifiers,
            ..
        } if modifiers.contains(KeyModifiers::CONTROL) => {
            runtime.preview_scroll_delta =
                runtime.preview_scroll_delta.saturating_add(preview_page);
            true
        }
        KeyEvent {
            code: KeyCode::Char('y') | KeyCode::Char('Y'),
            modifiers,
            ..
        } if modifiers.contains(KeyModifiers::CONTROL) => {
            runtime.preview_scroll_delta = runtime.preview_scroll_delta.saturating_sub(1);
            true
        }
        KeyEvent {
            code: KeyCode::Char('\u{19}'),
            modifiers,
            ..
        } if modifiers.is_empty() => {
            runtime.preview_scroll_delta = runtime.preview_scroll_delta.saturating_sub(1);
            true
        }
        KeyEvent {
            code: KeyCode::Up,
            modifiers,
            ..
        } if modifiers.contains(KeyModifiers::CONTROL) => {
            runtime.preview_scroll_delta = runtime.preview_scroll_delta.saturating_sub(1);
            true
        }
        KeyEvent {
            code: KeyCode::Char('e') | KeyCode::Char('E'),
            modifiers,
            ..
        } if modifiers.contains(KeyModifiers::CONTROL) => {
            runtime.preview_scroll_delta = runtime.preview_scroll_delta.saturating_add(1);
            true
        }
        KeyEvent {
            code: KeyCode::Char('\u{5}'),
            modifiers,
            ..
        } if modifiers.is_empty() => {
            runtime.preview_scroll_delta = runtime.preview_scroll_delta.saturating_add(1);
            true
        }
        KeyEvent {
            code: KeyCode::Down,
            modifiers,
            ..
        } if modifiers.contains(KeyModifiers::CONTROL) => {
            runtime.preview_scroll_delta = runtime.preview_scroll_delta.saturating_add(1);
            true
        }
        KeyEvent {
            code: KeyCode::Char('\u{0b}'),
            modifiers,
            ..
        } if modifiers.is_empty() => {
            // Some terminals report Ctrl+K as a raw control character.
            state.fuzzy_search.selected = state.fuzzy_search.selected.saturating_sub(1);
            adjust_fuzzy_scroll(state, list_height);
            runtime.request_preview_for_selection(state);
            true
        }
        KeyEvent {
            code: KeyCode::Char('k') | KeyCode::Char('K') | KeyCode::Char('\u{0b}'),
            modifiers,
            ..
        } if modifiers.contains(KeyModifiers::CONTROL) => {
            state.fuzzy_search.selected = state.fuzzy_search.selected.saturating_sub(1);
            adjust_fuzzy_scroll(state, list_height);
            runtime.request_preview_for_selection(state);
            true
        }
        KeyEvent {
            code: KeyCode::Char('j') | KeyCode::Char('J') | KeyCode::Char('\n'),
            modifiers,
            ..
        } if modifiers.contains(KeyModifiers::CONTROL) => {
            let len = fuzzy_results_len(state);
            if len > 0 {
                state.fuzzy_search.selected = (state.fuzzy_search.selected + 1).min(len - 1);
            }
            adjust_fuzzy_scroll(state, list_height);
            runtime.request_preview_for_selection(state);
            true
        }
        KeyEvent {
            code: KeyCode::Up, ..
        } => {
            state.fuzzy_search.selected = state.fuzzy_search.selected.saturating_sub(1);
            adjust_fuzzy_scroll(state, list_height);
            runtime.request_preview_for_selection(state);
            true
        }
        KeyEvent {
            code: KeyCode::Down,
            ..
        } => {
            let len = fuzzy_results_len(state);
            if len > 0 {
                state.fuzzy_search.selected = (state.fuzzy_search.selected + 1).min(len - 1);
            }
            adjust_fuzzy_scroll(state, list_height);
            runtime.request_preview_for_selection(state);
            true
        }
        KeyEvent {
            code: KeyCode::PageUp,
            ..
        } => {
            state.fuzzy_search.selected = state
                .fuzzy_search
                .selected
                .saturating_sub(list_height.max(1));
            adjust_fuzzy_scroll(state, list_height);
            runtime.request_preview_for_selection(state);
            true
        }
        KeyEvent {
            code: KeyCode::PageDown,
            ..
        } => {
            let len = fuzzy_results_len(state);
            if len > 0 {
                state.fuzzy_search.selected =
                    (state.fuzzy_search.selected + list_height.max(1)).min(len - 1);
            }
            adjust_fuzzy_scroll(state, list_height);
            runtime.request_preview_for_selection(state);
            true
        }
        KeyEvent {
            code: KeyCode::Home,
            ..
        } => {
            state.fuzzy_search.selected = 0;
            adjust_fuzzy_scroll(state, list_height);
            runtime.request_preview_for_selection(state);
            true
        }
        KeyEvent {
            code: KeyCode::End, ..
        } => {
            let len = fuzzy_results_len(state);
            if len > 0 {
                state.fuzzy_search.selected = len - 1;
            }
            adjust_fuzzy_scroll(state, list_height);
            runtime.request_preview_for_selection(state);
            true
        }
        KeyEvent {
            code: KeyCode::Char(c),
            modifiers,
            ..
        } if (modifiers.is_empty() || *modifiers == KeyModifiers::SHIFT) && !c.is_control() => {
            state.fuzzy_search.query.push(*c);
            runtime.preview_model = None;
            runtime.last_preview_key = None;
            runtime.run_search_for_mode(state);
            true
        }
        _ => true,
    }
}

pub(super) fn fuzzy_results_len(state: &AppState) -> usize {
    match state.fuzzy_search.mode {
        SearchMode::Files => state.fuzzy_search.file_results.len(),
        SearchMode::Content => state.fuzzy_search.match_results.len(),
    }
}

pub(super) fn delete_last_word(query: &mut String) {
    while query.chars().last().is_some_and(|c| c.is_whitespace()) {
        query.pop();
    }
    while query.chars().last().is_some_and(|c| !c.is_whitespace()) {
        query.pop();
    }
}
