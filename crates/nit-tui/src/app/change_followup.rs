use super::{followup_mission, prepare_swarm_followup_draft, selection_text};
use crate::{swarm::SwarmRuntime, widgets::change_review_view::interaction};
use nit_core::{followups::Source, AgentConsoleTab, AppState, Mode, PaneId};

pub(super) fn focused(state: &AppState) -> bool {
    state.change_review_active()
        && state.focus == PaneId::Visualizer
        && !state.terminal_popup.visible
        && state.command_line.is_none()
        && state.prompt.is_none()
        && state.search_prompt.is_none()
        && !state.fuzzy_search.open
        && !state.show_help
        && !state.protocol_picker.open
        && !state.rule_picker.open
        && state.definition_popup.is_none()
        && !state.agents.artifacts_popup_open
        && !state.agents.global_archive_open
        && !state.agents.show_multiway_popup
        && !state.show_substrate_overlay
}

pub(super) fn draft(state: &mut AppState, swarm: &SwarmRuntime, pasted: Option<&str>) {
    let Some((path, revision, hunk)) = interaction::context(state) else {
        state.status = Some("Choose a file or commit to discuss.".into());
        return;
    };
    // Git reports paths from the repository root. Agent tasks may have been
    // started in a subdirectory, so preserve the real location on handoff.
    let path = if path.is_empty() || !state.change_review.root.is_absolute() {
        path
    } else {
        let absolute = state.change_review.root.join(&path);
        absolute
            .strip_prefix(&state.workspace_root)
            .unwrap_or(&absolute)
            .to_string_lossy()
            .into_owned()
    };
    let source = Source {
        path,
        revision,
        hunk,
    };
    let excerpt = interaction::selected(state)
        .map(|s| selection_text(&state.change_review.preview, s))
        .unwrap_or_default();
    let mut context = source
        .revision
        .as_ref()
        .map(|id| format!("Git commit {id}"))
        .unwrap_or_else(|| "Working tree".into());
    if !source.path.is_empty() {
        context.push_str(" · ");
        context.push_str(&source.path.escape_debug().to_string());
    }
    if let Some((old, line)) = interaction::source_line(state) {
        context.push_str(&format!(
            " · {} line {line}",
            if old { "old" } else { "new" }
        ));
    }
    if let Some(hunk) = &source.hunk {
        context.push('\n');
        context.push_str(hunk);
    }
    if !excerpt.is_empty() {
        context.push('\n');
        context.push_str(&excerpt);
    }
    if let Some(mid) = followup_mission(state, swarm).map(str::to_owned) {
        let suggested = if source.revision.is_none() && !source.path.is_empty() {
            swarm
                .operator_followup_task_for_path(state, &mid, source.path.as_ref())
                .ok()
        } else {
            None
        };
        if let Err(err) =
            prepare_swarm_followup_draft(state, &mid, suggested.as_deref(), Some(source))
        {
            state.status = Some(err);
            return;
        }
    } else {
        state.terminal_pane_active = false;
        state.agents.console_tab = AgentConsoleTab::Thread;
        state.select_station_center(nit_core::StationCenterTab::Chat);
        state.mode = Mode::Normal;
    }
    let draft = &mut state.agents.chat_input;
    if !draft.is_empty() && !draft.ends_with('\n') {
        draft.push('\n');
    }
    draft.push_str(&context);
    draft.push('\n');
    if let Some(text) = pasted {
        draft.push_str(text);
    }
    state.agents.chat_input_cursor = draft.chars().count();
    state.agents.chat_input_selection_anchor = None;
    state.agents.chat_input_scroll = usize::MAX;
    state.status = Some("Changes context added to the chat draft.".into());
}

pub(super) fn key(
    state: &mut AppState,
    swarm: &SwarmRuntime,
    key: crossterm::event::KeyEvent,
    clipboard: &mut Option<arboard::Clipboard>,
) -> bool {
    use crossterm::event::{KeyCode, KeyModifiers};
    if !focused(state) {
        return false;
    }
    match (key.modifiers, key.code) {
        (KeyModifiers::NONE, KeyCode::Char('f')) => draft(state, swarm, None),
        (KeyModifiers::CONTROL, KeyCode::Char('v')) => {
            let text = clipboard
                .as_mut()
                .and_then(|c| c.get_text().ok())
                .or_else(|| state.yank.clone());
            if let Some(text) = text.filter(|s| !s.is_empty()) {
                draft(state, swarm, Some(&text));
            }
        }
        (KeyModifiers::NONE, KeyCode::Char('y')) | (KeyModifiers::CONTROL, KeyCode::Char('c')) => {
            if let Some(sel) = interaction::selected(state) {
                let text = selection_text(&state.change_review.preview, sel);
                if !text.is_empty() {
                    state.yank_kind = if text.contains('\n') {
                        nit_core::YankKind::Line
                    } else {
                        nit_core::YankKind::Char
                    };
                    if let Some(c) = clipboard.as_mut() {
                        let _ = c.set_text(text.clone());
                    }
                    state.yank = Some(text);
                    state.status = Some("Copied patch selection".into());
                }
            }
        }
        _ => return false,
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::widgets::change_review_view::interaction::{horizontal, patch_area, position};
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use nit_core::{state::ChangedFile, Buffer, UiSelection, UiSelectionPane};
    use ratatui::layout::{Position, Rect};

    fn state() -> AppState {
        let mut state = AppState::new(
            ".".into(),
            Buffer::empty("e", None),
            Buffer::empty("n", None),
        );
        state.show_change_review = true;
        state.focus = PaneId::Visualizer;
        let r = &mut state.change_review;
        r.files.push(ChangedFile {
            path: "src/lib.rs".into(),
            index: ' ',
            worktree: 'M',
        });
        r.preview_path = Some("src/lib.rs".into());
        r.preview = [
            "Unstaged",
            "@@ -18,2 +30,3 @@ fn result()",
            " unchanged",
            "-old",
            "+\tλ界new",
            "+extra",
            " subsequent source that extends beyond the right side of the narrow terminal",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect();
        state
    }

    #[test]
    fn character_selection_copies_verbatim_and_paste_appends_unsent_context() {
        let mut state = state();
        let swarm = SwarmRuntime::default();
        let area = Rect::new(3, 4, 45, 19);
        let patch = patch_area(&state.change_review, area);
        assert_eq!(
            position(
                &state.change_review,
                area,
                Position::new(patch.x + 7, patch.y + 4),
                false
            ),
            Some((4, 3))
        );
        horizontal(&mut state.change_review, area, 8);
        assert_eq!(
            position(
                &state.change_review,
                area,
                Position::new(patch.x, patch.y + 4),
                false
            ),
            Some((4, 4))
        );
        state.ui_selection = Some(UiSelection {
            pane: UiSelectionPane::ChangesPatch,
            start_line: 4,
            start_col: 4,
            end_line: 4,
            end_col: 1,
        });
        assert!(key(
            &mut state,
            &swarm,
            KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE),
            &mut None
        ));
        assert_eq!(state.yank.as_deref(), Some("\tλ界"));
        state.agents.chat_input = "Earlier question".into();
        draft(&mut state, &swarm, Some("Can we simplify this?"));
        assert_eq!(state.focus, PaneId::Notes);
        assert_eq!(state.agents.chat_input,"Earlier question\nWorking tree · src/lib.rs · new line 31\n@@ -18,2 +30,3 @@ fn result()\n\tλ界\nCan we simplify this?");
        assert!(state.agents.active_turns.is_empty());
        assert_eq!(state.change_review.preview[4], "+\tλ界new");
    }

    #[test]
    fn paths_crossing_the_workspace_boundary_retain_their_actual_location() {
        let mut state = state();
        let project = std::env::temp_dir().join("nit-draft-project");
        state.workspace_root = project.join("module");
        state.change_review.root = project.clone();
        draft(&mut state, &SwarmRuntime::default(), None);
        let outside = project.join("src/lib.rs");
        let rendered = outside.to_string_lossy().escape_debug().to_string();
        assert!(state
            .agents
            .chat_input
            .contains(&format!("Working tree · {rendered}")));
        state.agents.chat_input.clear();
        state.change_review.files[0].path = "module/nested.rs".into();
        draft(&mut state, &SwarmRuntime::default(), None);
        assert!(state.agents.chat_input.contains("Working tree · nested.rs"));
    }

    #[test]
    fn historical_deletion_uses_old_path_and_revision_and_ignores_source_like_headers() {
        let mut state = state();
        let r = &mut state.change_review;
        r.files.clear();
        r.commits.push(nit_core::state::ReviewCommit {
            id: "abc123".into(),
            subject: "Remove file".into(),
            author: "".into(),
            date: "".into(),
        });
        r.preview_commit = Some("abc123".into());
        r.preview = [
            "diff --git a/old b/old",
            "--- \"a/old\\040file\"",
            "+++ /dev/null",
            "@@ -7,2 +0,0 @@",
            "--- a/fake",
            "-rest",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect();
        r.history_patch_open = true;
        state.ui_selection = Some(UiSelection {
            pane: UiSelectionPane::ChangesPatch,
            start_line: 5,
            start_col: 0,
            end_line: 5,
            end_col: 5,
        });
        assert_eq!(
            interaction::context(&state),
            Some((
                "old file".into(),
                Some("abc123".into()),
                Some("@@ -7,2 +0,0 @@".into())
            ))
        );
        assert_eq!(interaction::source_line(&state), Some((true, 8)));
        state.change_review.preview_commit = Some("stale".into());
        assert!(interaction::selected(&state).is_none());
        assert_eq!(
            interaction::context(&state),
            Some(("".into(), Some("abc123".into()), None))
        );
    }
}
