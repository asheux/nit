use super::*;
use crate::{buffer::Buffer, io, lab::AppKind, prompt::Prompt};

mod games_inspect;
mod helpers;

pub(super) use helpers::{apply_protocol_selection, apply_rule_selection};
use helpers::{is_help_command_tokens, lab_from_tokens, log_rule_list, log_rule_overview};

/// `:q` semantics, launch-mode-aware. Returns `true` when the caller
/// should exit the app, `false` when control stays in the editor
/// (either a confirmation prompt was raised or a buffer was closed in
/// place).
///
/// - **File-launch (`nit foo.rs`)**: quit immediately when clean; raise
///   `ConfirmQuit` when any buffer is dirty.
/// - **Directory-launch (`nit src/`)**: behaviour depends on whether
///   the active buffer represents an actual file. If it does, close
///   that buffer (confirming first if dirty). If the active buffer is
///   *untitled* — which is the state right after closing the last
///   file, and the default state of NITTree-only sessions — there's
///   nothing meaningful to close, so `:q` quits the app (with the same
///   dirty-check across all buffers as the file-launch path).
///
/// Diverges from `Action::Quit` (Ctrl-Q), which always quits the app
/// regardless of launch mode — Ctrl-Q is the global "exit nit"
/// shortcut and shouldn't surprise users by hiding the only file
/// instead of exiting.
pub(super) fn quit_or_close_buffer(state: &mut AppState) -> bool {
    if state.launched_with_file_path {
        return if state.has_unsaved_editor_buffers() {
            state.prompt = Some(Prompt::ConfirmQuit);
            false
        } else {
            true
        };
    }

    // Directory-launch path.
    let active_is_untitled = state.editor_buffer().path().is_none();
    if active_is_untitled {
        // No file to close → user is asking to exit nit. Honour the
        // dirty-buffer guard so unsaved work in OTHER buffers isn't
        // silently discarded.
        if state.has_unsaved_editor_buffers() {
            state.prompt = Some(Prompt::ConfirmQuit);
            false
        } else {
            true
        }
    } else if state.editor_buffer().is_dirty() {
        state.prompt = Some(Prompt::ConfirmCloseBuffer);
        false
    } else {
        close_active_editor_buffer(state);
        false
    }
}

/// Save the active editor buffer to its on-disk path. Returns `true` on
/// success, `false` when the buffer has no path or the write fails (with
/// the reason surfaced in `state.status`). Mirrors `Action::Save`'s
/// effects so `:w` and `<leader>w` behave identically.
fn save_active_buffer(state: &mut AppState) -> bool {
    let buf = state.editor_buffer_mut();
    if buf.path().is_none() {
        state.status = Some("No path to save".into());
        return false;
    }
    if let Err(e) = io::save_buffer(buf) {
        state.status = Some(format!("Save failed: {e}"));
        return false;
    }
    buf.mark_clean();
    state.status = Some("Saved".into());
    if let Some(file_path) = state.editor_buffer().path().cloned() {
        state.genome_save_eval_pending = Some(file_path);
    }
    true
}

/// Close the active editor buffer, vim-`:bd`-style. If another non-notes
/// buffer is still open, switch to the highest-indexed one (= the most
/// recently added) and remove the closed buffer from the vec. If the
/// active buffer was the only file open, replace it with an untitled
/// blank and pop open NITTree so the user has a place to land — that's
/// the "nothing in the buffers → NITTree" fallback.
///
/// Never removes the notes buffer; only buffers other than
/// `state.notes_buffer_id` are eligible to be active or to be closed.
///
/// Exposed `pub(super)` so `action_apply::ConfirmCloseBufferYes` can
/// share this exact implementation — keeps `:q` and `:wq`'s
/// directory-launch close behaviour byte-identical.
pub(super) fn close_active_editor_buffer(state: &mut AppState) {
    let active = state.active_editor_buffer_id;
    let notes = state.notes_buffer_id;

    // Defensive: `:wq` should only fire on the editor pane, but if the
    // active id ever points at notes, do nothing rather than nuke notes.
    if active == notes {
        return;
    }

    // Highest-indexed non-notes, non-active buffer = "last buffer".
    // None when the active buffer is the only editor buffer open.
    let next_active = (0..state.buffers.len())
        .rev()
        .find(|&i| i != active && i != notes);

    match next_active {
        Some(idx) => {
            // Remove the active slot; trailing indices shift down by 1.
            state.buffers.remove(active);
            if state.notes_buffer_id > active {
                state.notes_buffer_id -= 1;
            }
            let new_active = if idx > active { idx - 1 } else { idx };
            state.active_editor_buffer_id = new_active;
            state.focus = PaneId::Editor;
            state.mode = Mode::Normal;
            let label = state
                .editor_buffer()
                .path()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| "untitled".into());
            state.status = Some(format!("Buffer closed; switched to {label}"));
        }
        None => {
            // No fallback buffer — replace the just-closed slot with an
            // untitled blank (so `active_editor_buffer_id` stays valid)
            // and open NITTree so the user has somewhere to go.
            state.buffers[active] = Buffer::empty("untitled", None);
            state.file_tree.root = state.workspace_root.clone();
            state.file_tree.open = true;
            // File tree is a sidebar over the Editor pane; focus stays
            // on Editor while the tree handles arrow keys (matches the
            // existing `:tree` command's behaviour).
            state.focus = PaneId::Editor;
            state.mode = Mode::Normal;
            state.status = Some("Buffer closed; NITTree opened".into());
        }
    }
}

/// Open a file at `path` in the editor. Mirrors `Action::OpenFile`:
/// reuses an existing buffer if one already holds the same path, swaps
/// in place when the current editor buffer is clean, or appends a new
/// buffer when the current is dirty. Path is interpreted relative to
/// `workspace_root` when not already absolute.
fn open_file_at_path(state: &mut AppState, raw_path: &str) {
    let trimmed = raw_path.trim();
    if trimmed.is_empty() {
        state.status = Some("Usage: :e <path>".into());
        return;
    }
    let path = std::path::PathBuf::from(trimmed);
    let absolute = if path.is_absolute() {
        path
    } else {
        state.workspace_root.join(path)
    };

    // Push the live cursor before the buffer swap so `:e other` is
    // reachable via `Ctrl-O` exactly like the NITTree / Ctrl-P paths.
    let pre_open = state.current_jump_entry();
    if let Some(buffer_id) = state.find_editor_buffer_by_path(&absolute) {
        if buffer_id != state.active_editor_buffer_id {
            state.jumplist.push(pre_open);
        }
        state.active_editor_buffer_id = buffer_id;
        state.focus = PaneId::Editor;
        state.mode = Mode::Normal;
        state.visualizer.pending_reseed = true;
        state.status = Some(format!("Opened {}", absolute.display()));
        return;
    }
    match io::load_to_string(&absolute) {
        Ok(content) => {
            let name = absolute
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| "untitled".into());
            let buf = Buffer::from_str(name, &content, Some(absolute.clone()));
            // Mirrors `Action::OpenFile`: only the never-touched untitled
            // buffer is safe to overwrite in place; a clean real file
            // stays in `buffers` so Ctrl-O can return to it.
            let active_is_initial_blank =
                state.editor_buffer().path().is_none() && !state.editor_buffer().is_dirty();
            state.jumplist.push(pre_open);
            if active_is_initial_blank {
                state.buffers[state.active_editor_buffer_id] = buf;
            } else {
                state.buffers.push(buf);
                state.active_editor_buffer_id = state.buffers.len() - 1;
            }
            state.focus = PaneId::Editor;
            state.mode = Mode::Normal;
            state.visualizer.pending_reseed = true;
            state.status = Some(format!("Opened {}", absolute.display()));
        }
        Err(err) => {
            state.status = Some(format!("Open failed: {err}"));
        }
    }
}

/// Extract the argument to a single-keyword command (e.g. `:e foo/bar`)
/// from the raw trimmed input. Returns the slice after the first
/// whitespace, with surrounding whitespace stripped. `None` when the
/// command had no argument. Case-preserving — necessary for filesystems
/// where `Foo.RS` and `foo.rs` are distinct files.
fn extract_command_arg<'a>(trimmed: &'a str, keyword: &str) -> Option<&'a str> {
    let without_colon = trimmed.trim_start_matches(':');
    let rest = without_colon
        .strip_prefix(keyword)
        .filter(|r| r.starts_with(char::is_whitespace) || r.is_empty())?;
    let arg = rest.trim();
    (!arg.is_empty()).then_some(arg)
}

pub(super) fn handle_command_line(state: &mut AppState, input: &str) -> bool {
    let trimmed = input.trim();
    let cmd = trimmed.trim_start_matches(':').trim().to_lowercase();
    if cmd.is_empty() {
        return false;
    }
    let normalized = cmd
        .split_whitespace()
        .map(|token| token.trim_matches(':'))
        .filter(|token| !token.is_empty())
        .collect::<Vec<_>>();
    let tokens: Vec<&str> = normalized;
    if let Some(target_lab) = lab_from_tokens(&tokens) {
        if target_lab != state.app_kind {
            state.status = Some(format!(
                "{} lab not active (current: {}). Use --lab {} to start.",
                target_lab.label(),
                state.app_kind.label(),
                target_lab
            ));
            return false;
        }
    }
    if is_help_command_tokens(&tokens) {
        state.show_help = true;
        state.help_scroll = 0;
        state.status = Some("Help opened".into());
        return false;
    }
    // Vim `:<N>` jump-to-line. Bare-number command, no other tokens.
    if tokens.len() == 1 {
        if let Ok(line) = tokens[0].parse::<usize>() {
            if let Some(buf) = state.focused_buffer_mut() {
                buf.go_to_line(line);
                buf.ensure_visible();
            }
            state.status = Some(format!("Line {line}"));
            return false;
        }
    }
    match tokens.as_slice() {
        ["substrate"] | ["sub"] | ["sig"] | ["signals"] => {
            state.show_substrate_overlay = true;
            state.substrate_overlay_tab = SubstrateOverlayTab::Signals;
            state.substrate_overlay_scroll = 0;
            false
        }
        ["claims"] => {
            state.show_substrate_overlay = true;
            state.substrate_overlay_tab = SubstrateOverlayTab::Claims;
            state.substrate_overlay_scroll = 0;
            false
        }
        ["assumptions"] | ["asm"] => {
            state.show_substrate_overlay = true;
            state.substrate_overlay_tab = SubstrateOverlayTab::Assumptions;
            state.substrate_overlay_scroll = 0;
            false
        }
        ["q"] | ["quit"] | ["exit"] => quit_or_close_buffer(state),
        ["w"] | ["write"] => {
            save_active_buffer(state);
            false
        }
        ["e"] | ["edit"] => {
            state.status = Some("Usage: :e <path>".into());
            false
        }
        _ if tokens.first() == Some(&"e") || tokens.first() == Some(&"edit") => {
            // Re-extract the path from the case-preserving `trimmed`
            // string (tokens are lowercased; filesystems are not).
            let keyword = tokens[0];
            if let Some(arg) = extract_command_arg(trimmed, keyword) {
                open_file_at_path(state, arg);
            } else {
                state.status = Some("Usage: :e <path>".into());
            }
            false
        }
        ["wq"] | ["x"] => {
            if !save_active_buffer(state) {
                // Save failed (no path / write error). Don't quit/close —
                // the user needs to see the error and decide what to do.
                return false;
            }
            if state.launched_with_file_path {
                // `nit foo.rs` style launch — `:wq` quits the editor,
                // matching vim's single-file ergonomics.
                true
            } else {
                // `nit` / `nit src/` style launch — close the buffer in
                // place, stay in the editor pane on an untitled buffer.
                // Explicitly NOT jumping to NITTree or another buffer.
                close_active_editor_buffer(state);
                false
            }
        }
        ["tree"] | ["nittree"] | ["explore"] => {
            state.file_tree.open = !state.file_tree.open;
            if state.file_tree.open {
                state.file_tree.root = state.workspace_root.clone();
                state.focus = PaneId::Editor;
                state.mode = Mode::Normal;
                state.status = Some("NITTree opened".into());
            } else {
                state.status = Some("NITTree closed".into());
            }
            false
        }
        ["find"] | ["ff"] => {
            state
                .fuzzy_search
                .open(SearchMode::Files, state.workspace_root.clone());
            state.status = Some("Search: files".into());
            false
        }
        ["grep"] | ["rg"] | ["search"] => {
            state
                .fuzzy_search
                .open(SearchMode::Content, state.workspace_root.clone());
            state.status = Some("Search: content".into());
            false
        }
        ["close"] => {
            if state.fuzzy_search.open {
                state.fuzzy_search.close();
                state.status = Some("Search closed".into());
            }
            false
        }
        ["run"] => match state.app_kind {
            AppKind::Gol => {
                state.visualizer.pending_run = true;
                state.visualizer.pending_snapshot = true;
                state.status = Some("Petri dish queued".into());
                false
            }
            AppKind::Games => {
                state.games.pending_run_override = None;
                state.games.pending_family_run = None;
                state.games.family_building = false;
                state.games.pending_run = true;
                state.status = Some("Games tournament queued".into());
                false
            }
        },
        ["gol", "run"] | ["run", "gol"] | ["life", "run"] | ["gol", "start"] | ["run", "life"] => {
            state.visualizer.pending_run = true;
            state.visualizer.pending_snapshot = true;
            state.status = Some("Petri dish queued".into());
            false
        }
        _ if tokens.first() == Some(&"games")
            && tokens.get(1) == Some(&"run")
            && tokens.len() > 2 =>
        {
            let (force, family) = if tokens.get(2) == Some(&"force") {
                match tokens.get(3).copied() {
                    Some(family) => (true, family),
                    None => {
                        state.status = Some(
                            "Usage: :games run force <fsm|ca|tm> {params} (e.g. :games run force fsm {3, 2})"
                                .into(),
                        );
                        return false;
                    }
                }
            } else {
                (false, tokens[2])
            };

            if state.games.family_building {
                state.status = Some("Family run preparation already in progress".into());
                return false;
            }

            match build_family_run_override(state, family, trimmed, force) {
                Ok(request) => {
                    state.games.pending_run_override = None;
                    state.games.pending_run = false;
                    state.games.pending_family_run = Some(request);
                    state.games.family_building = true;
                    let mode = if force { "forced, " } else { "" };
                    state.status = Some(format!("Preparing family run ({mode}{family})..."));
                }
                Err(err) => {
                    state.games.pending_family_run = None;
                    state.games.family_building = false;
                    state.status = Some(err)
                }
            }
            false
        }
        ["games", "run"] | ["run", "games"] => {
            state.games.pending_run_override = None;
            state.games.pending_family_run = None;
            state.games.family_building = false;
            state.games.pending_run = true;
            state.status = Some("Games tournament queued".into());
            false
        }
        ["gol", "hide"] | ["hide", "gol"] => {
            state.visualizer.pending_hide = true;
            state.status = Some("Petri dish hiding".into());
            false
        }
        ["gol", "show"] | ["show", "gol"] => {
            state.visualizer.pending_show = true;
            state.status = Some("Petri dish showing".into());
            false
        }
        ["gol", "stop"] | ["life", "stop"] => {
            state.visualizer.pending_close = true;
            state.status = Some("Petri dish closing".into());
            false
        }
        ["run", "stop"] if state.app_kind == AppKind::Gol => {
            state.visualizer.pending_close = true;
            state.status = Some("Petri dish closing".into());
            false
        }
        ["games", "hide"] | ["hide", "games"] => {
            state.games.pending_hide = true;
            state.status = Some("Games tournament hiding".into());
            false
        }
        ["games", "show"] | ["show", "games"] => {
            state.games.pending_show = true;
            state.status = Some("Games tournament showing".into());
            false
        }
        ["games", "stop"] | ["stop", "games"] => {
            state.games.pending_close = true;
            state.status = Some("Games tournament closing".into());
            false
        }
        ["games", "status"] => {
            state.status = Some(format!("Games status: {:?}", state.games.status));
            false
        }
        ["games", "runs"] | ["games", "browse"] | ["games", "browser"] => {
            state.games.replay.open = false;
            state.games.match_history.open = false;
            state.games.run_browser.open = true;
            state.games.run_browser.loading = true;
            state.games.run_browser.last_error = None;
            state.games.run_browser.entries.clear();
            state.games.run_browser.selected = 0;
            state.games.run_browser.scroll_offset = 0;
            state.games.pending_run_browser = true;
            state.status = Some("Games run browser opened".into());
            false
        }
        ["games", "replay"] => {
            if state.games.last_run.is_none() {
                state.status = Some("No run loaded for replay".into());
            } else {
                state.games.run_browser.open = false;
                state.games.match_history.open = false;
                state.games.replay.open = true;
                state.games.replay.loading = false;
                state.games.replay.last_error = None;
                state.games.replay.selected_pair = None;
                state.games.replay.selected_index = 0;
                state.games.replay.title = None;
                state.games.replay.lines.clear();
                state.games.replay.scroll_offset = 0;
                state.games.replay.cycle = None;
                state.status = Some("Games replay opened".into());
            }
            false
        }
        ["games", "history"] | ["games", "hist"] | ["games", "plot"] | ["games", "plots"] => {
            open_games_history_popup(state);
            false
        }
        ["history"] | ["hist"] | ["plot"] | ["plots"] if state.app_kind == AppKind::Games => {
            open_games_history_popup(state);
            false
        }
        _ if tokens.first() == Some(&"games") && tokens.get(1) == Some(&"inspect") => {
            games_inspect::handle_games_inspect(state, &tokens, trimmed);
            false
        }
        ["games", "strategy"]
        | ["games", "strategies"]
        | ["games", "strategy", "run"]
        | ["games", "strategies", "run"] => {
            games_inspect::handle_games_strategy_run(state);
            false
        }
        ["games", "strategy", "all"]
        | ["games", "strategies", "all"]
        | ["games", "strategy", "config"]
        | ["games", "strategies", "config"] => {
            games_inspect::handle_games_strategy_config(state);
            false
        }
        _ if tokens.first() == Some(&"games") && tokens.get(1) == Some(&"tm") => {
            games_inspect::handle_games_tm(state, &tokens, trimmed);
            false
        }
        _ if tokens.first() == Some(&"games") && tokens.get(1) == Some(&"ca") => {
            games_inspect::handle_games_ca(state, &tokens, trimmed);
            false
        }
        _ if tokens.first() == Some(&"games")
            && matches!(tokens.get(1), Some(&"analyze") | Some(&"analyse")) =>
        {
            games_inspect::handle_games_analyze(state, trimmed);
            false
        }
        ["games", "export"] => {
            state.games.pending_export = true;
            false
        }
        ["gol", "seed"] => {
            state.visualizer.seed_view = state.visualizer.seed_view.next();
            state.status = Some(format!("Seed view: {}", state.visualizer.seed_view.label()));
            false
        }
        ["seed", "view"] if state.app_kind == AppKind::Gol => {
            state.visualizer.seed_view = state.visualizer.seed_view.next();
            state.status = Some(format!("Seed view: {}", state.visualizer.seed_view.label()));
            false
        }
        ["gol", "encoder"] => {
            state.visualizer.seed_encoder = state.visualizer.seed_encoder.next();
            state.visualizer.pending_reseed = true;
            state.status = Some(format!(
                "Encoder: {}",
                state.visualizer.seed_encoder.label()
            ));
            false
        }
        ["gol", "encoder", name] => {
            if let Some(id) = SeedEncoderId::from_str_name(name) {
                state.visualizer.seed_encoder = id;
                state.visualizer.pending_reseed = true;
                state.status = Some(format!("Encoder: {}", id.label()));
            } else {
                state.status = Some(format!("Unknown encoder: {name}"));
            }
            false
        }
        ["seed", "encoder"] if state.app_kind == AppKind::Gol => {
            state.visualizer.seed_encoder = state.visualizer.seed_encoder.next();
            state.visualizer.pending_reseed = true;
            state.status = Some(format!(
                "Encoder: {}",
                state.visualizer.seed_encoder.label()
            ));
            false
        }
        ["seed", "encoder", name] if state.app_kind == AppKind::Gol => {
            if let Some(id) = SeedEncoderId::from_str_name(name) {
                state.visualizer.seed_encoder = id;
                state.visualizer.pending_reseed = true;
                state.status = Some(format!("Encoder: {}", id.label()));
            } else {
                state.status = Some(format!("Unknown encoder: {name}"));
            }
            false
        }
        _ if tokens.first() == Some(&"gol") && tokens.get(1) == Some(&"rule") => {
            if tokens.len() == 2 {
                log_rule_overview(state);
            } else {
                let selector = trimmed
                    .split_whitespace()
                    .skip(2)
                    .collect::<Vec<_>>()
                    .join(" ");
                match state.rule_catalog.select(&selector) {
                    Ok(selected) => apply_rule_selection(state, selected, true),
                    Err(err) => {
                        state.status = Some(format!(
                            "Invalid GoL rule '{selector}': {err}. Try B3/S23 or 'conway'."
                        ));
                    }
                }
            }
            false
        }
        _ if tokens.first() == Some(&"gol") && tokens.get(1) == Some(&"rules") => {
            log_rule_list(state);
            false
        }
        ["petri", "hide"] | ["hide", "petri"] if state.app_kind == AppKind::Gol => {
            state.visualizer.pending_hide = true;
            state.status = Some("Petri dish hiding".into());
            false
        }
        ["petri", "show"] | ["show", "petri"] if state.app_kind == AppKind::Gol => {
            state.visualizer.pending_show = true;
            state.status = Some("Petri dish showing".into());
            false
        }
        other => {
            state.status = Some(format!("Unknown command: {}", other.join(" ")));
            false
        }
    }
}
