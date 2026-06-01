use std::time::Instant;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use nit_core::{actions::Action, AppState, PaneId};

use super::*;

pub(super) fn handle_normal_chords(
    key: &KeyEvent,
    state: &AppState,
    input: &mut InputState,
) -> Option<Action> {
    if !is_motion_mode(state) {
        input.normal_last_char = None;
        return None;
    }

    if !key.modifiers.is_empty() && key.modifiers != KeyModifiers::SHIFT {
        input.normal_last_char = None;
        return None;
    }

    let now = Instant::now();
    match key.code {
        KeyCode::Char('g') => {
            if input.chord_normal('g', now) {
                Some(Action::GoToTop)
            } else {
                None
            }
        }
        KeyCode::Char('y') => {
            if is_normal_mode(state) && input.chord_normal('y', now) {
                Some(Action::YankLine)
            } else {
                None
            }
        }
        KeyCode::Char('d') => {
            // `gd`: a pending `g` half-chord still inside the chord window opens
            // the goto-definition popup before `d` becomes the delete operator.
            if is_normal_mode(state)
                && input.normal_last_char == Some('g')
                && now.duration_since(input.normal_last_time) <= CHORD_TIMEOUT
            {
                input.normal_last_char = None;
                return Some(Action::GotoDefinition);
            }
            // `d` opens an operator-pending state: the next key picks the
            // motion (`w`/`W`/`e`/`E`/`b`/`B`/`d`/`$`). The chord half-key
            // tracker is cleared so a stale `g`/`y` doesn't accidentally
            // pair with this `d`.
            if is_normal_mode(state) {
                input.normal_last_char = None;
                input.pending_editor_op = Some(PendingEditorOp::Delete);
            }
            None
        }
        KeyCode::Char('c') => {
            // `c` mirrors `d` but the resolved action also switches to Insert
            // mode (see Change* arms in `action_apply`). Capital `C` is the
            // standalone `c$` and is handled in `key_dispatch`.
            if is_normal_mode(state) {
                input.normal_last_char = None;
                input.pending_editor_op = Some(PendingEditorOp::Change);
            }
            None
        }
        _ => {
            input.normal_last_char = None;
            None
        }
    }
}

/// Intercept the next keystroke when a vim chord op is waiting for its argument
/// (e.g. the `<c>` after `r`, `f`, `F`, `t`, `T`, or the second key of `zz`/`zt`/`zb`).
/// Only consumes keys while the editor is in motion mode.
pub(super) fn handle_editor_pending_op(
    key: &KeyEvent,
    state: &AppState,
    input: &mut InputState,
) -> Option<Action> {
    if !is_motion_mode(state) {
        input.pending_editor_op = None;
        return None;
    }
    let op = input.pending_editor_op?;

    if matches!(key.code, KeyCode::Esc) {
        // Cancel the pending op silently; stay in normal/visual mode.
        input.pending_editor_op = None;
        return None;
    }

    let plain_or_shift = key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT;

    match (op, key.code) {
        (PendingEditorOp::Replace, KeyCode::Char(c)) if plain_or_shift => {
            input.pending_editor_op = None;
            Some(Action::ReplaceChar(c))
        }
        (PendingEditorOp::FindForward, KeyCode::Char(c)) if plain_or_shift => {
            input.pending_editor_op = None;
            input.last_find = Some((c, true, false));
            Some(Action::FindChar(c, true, false))
        }
        (PendingEditorOp::FindBack, KeyCode::Char(c)) if plain_or_shift => {
            input.pending_editor_op = None;
            input.last_find = Some((c, false, false));
            Some(Action::FindChar(c, false, false))
        }
        (PendingEditorOp::TillForward, KeyCode::Char(c)) if plain_or_shift => {
            input.pending_editor_op = None;
            input.last_find = Some((c, true, true));
            Some(Action::FindChar(c, true, true))
        }
        (PendingEditorOp::TillBack, KeyCode::Char(c)) if plain_or_shift => {
            input.pending_editor_op = None;
            input.last_find = Some((c, false, true));
            Some(Action::FindChar(c, false, true))
        }
        (PendingEditorOp::ZMotion, KeyCode::Char('z')) if plain_or_shift => {
            input.pending_editor_op = None;
            Some(Action::CenterViewportOnCursor)
        }
        (PendingEditorOp::ZMotion, KeyCode::Char('t')) if plain_or_shift => {
            input.pending_editor_op = None;
            Some(Action::ViewportTopOnCursor)
        }
        (PendingEditorOp::ZMotion, KeyCode::Char('b')) if plain_or_shift => {
            input.pending_editor_op = None;
            Some(Action::ViewportBottomOnCursor)
        }
        // `d` waiting for its motion target. Each arm matches both the
        // plain character and the shifted form so `dW`/`dE`/`dB` work
        // regardless of how the terminal reports the modifier mask.
        (PendingEditorOp::Delete, KeyCode::Char('w')) if plain_or_shift => {
            input.pending_editor_op = None;
            Some(Action::DeleteWordForward)
        }
        (PendingEditorOp::Delete, KeyCode::Char('e')) if plain_or_shift => {
            input.pending_editor_op = None;
            Some(Action::DeleteWordEnd)
        }
        (PendingEditorOp::Delete, KeyCode::Char('b')) if plain_or_shift => {
            input.pending_editor_op = None;
            Some(Action::DeleteWordBack)
        }
        (PendingEditorOp::Delete, KeyCode::Char('W')) if plain_or_shift => {
            input.pending_editor_op = None;
            Some(Action::DeleteBigWordForward)
        }
        (PendingEditorOp::Delete, KeyCode::Char('E')) if plain_or_shift => {
            input.pending_editor_op = None;
            Some(Action::DeleteBigWordEnd)
        }
        (PendingEditorOp::Delete, KeyCode::Char('B')) if plain_or_shift => {
            input.pending_editor_op = None;
            Some(Action::DeleteBigWordBack)
        }
        (PendingEditorOp::Delete, KeyCode::Char('d')) if plain_or_shift => {
            input.pending_editor_op = None;
            Some(Action::DeleteLine)
        }
        (PendingEditorOp::Delete, KeyCode::Char('$')) if plain_or_shift => {
            input.pending_editor_op = None;
            Some(Action::DeleteToEnd)
        }
        // `c` operator. Vim quirk: `cw` and `cW` route to the *end-of-word*
        // change (mirrors `ce`/`cE`), not to the start of the next word —
        // changing past trailing whitespace would push that whitespace into
        // the user's new text. See `:h cw`.
        (PendingEditorOp::Change, KeyCode::Char('w')) if plain_or_shift => {
            input.pending_editor_op = None;
            Some(Action::ChangeWordEnd)
        }
        (PendingEditorOp::Change, KeyCode::Char('e')) if plain_or_shift => {
            input.pending_editor_op = None;
            Some(Action::ChangeWordEnd)
        }
        (PendingEditorOp::Change, KeyCode::Char('b')) if plain_or_shift => {
            input.pending_editor_op = None;
            Some(Action::ChangeWordBack)
        }
        (PendingEditorOp::Change, KeyCode::Char('W')) if plain_or_shift => {
            input.pending_editor_op = None;
            Some(Action::ChangeBigWordEnd)
        }
        (PendingEditorOp::Change, KeyCode::Char('E')) if plain_or_shift => {
            input.pending_editor_op = None;
            Some(Action::ChangeBigWordEnd)
        }
        (PendingEditorOp::Change, KeyCode::Char('B')) if plain_or_shift => {
            input.pending_editor_op = None;
            Some(Action::ChangeBigWordBack)
        }
        (PendingEditorOp::Change, KeyCode::Char('c')) if plain_or_shift => {
            input.pending_editor_op = None;
            Some(Action::ChangeLine)
        }
        (PendingEditorOp::Change, KeyCode::Char('$')) if plain_or_shift => {
            input.pending_editor_op = None;
            Some(Action::ChangeToEnd)
        }
        _ => {
            input.pending_editor_op = None;
            None
        }
    }
}

pub(super) fn handle_insert_chords(
    key: &KeyEvent,
    state: &AppState,
    input: &mut InputState,
) -> Option<Action> {
    if !is_insert_editing(state) || state.focus != PaneId::Editor {
        input.pending_insert = None;
        return None;
    }

    if !key.modifiers.is_empty() && key.modifiers != KeyModifiers::SHIFT {
        input.pending_insert = None;
        return None;
    }

    if let Some((pending, _)) = input.pending_insert {
        match key.code {
            KeyCode::Char('j') => {
                input.pending_insert = None;
                return Some(Action::SaveAndNormal);
            }
            _ => {
                input.deferred_key = Some(*key);
                let c = input.take_pending_insert().unwrap_or(pending);
                return Some(Action::InsertChar(c));
            }
        }
    }

    match key.code {
        KeyCode::Char('j') => {
            input.pending_insert = Some(('j', Instant::now()));
            None
        }
        _ => None,
    }
}

pub(super) fn visualizer_ctrl_action(key: &KeyEvent, state: &AppState) -> Option<Action> {
    let petri_visible = state.visualizer.running && !state.visualizer.petri_hidden;
    if state.focus != PaneId::Visualizer && !petri_visible {
        return None;
    }
    if !key.modifiers.contains(KeyModifiers::CONTROL) {
        return None;
    }
    if key.modifiers.contains(KeyModifiers::SHIFT) {
        return match key.code {
            KeyCode::Char('v') | KeyCode::Char('V') => Some(Action::VisualizerCycleSeedOverlays),
            _ => None,
        };
    }
    match key.code {
        KeyCode::Char('v') | KeyCode::Char('V') => Some(Action::VisualizerToggleSeedView),
        KeyCode::Char('r') | KeyCode::Char('R') => Some(Action::VisualizerCycleSeedView),
        KeyCode::Char('e') | KeyCode::Char('E') => Some(Action::VisualizerCycleEncoder),
        KeyCode::Char('a') | KeyCode::Char('A') => Some(Action::VisualizerApply),
        KeyCode::Char('g') | KeyCode::Char('G') => Some(Action::VisualizerToggleSearch),
        KeyCode::Char('y') | KeyCode::Char('Y') => Some(Action::VisualizerToggleSeedSource),
        KeyCode::Char('n') | KeyCode::Char('N') => Some(Action::VisualizerSnapshot),
        KeyCode::Char('m') | KeyCode::Char('M') => Some(Action::VisualizerCycleRenderMode),
        KeyCode::Char('j') | KeyCode::Char('J') => Some(Action::VisualizerToggleAgeShading),
        KeyCode::Char('k') | KeyCode::Char('K') => Some(Action::VisualizerToggleTrails),
        KeyCode::Char('b') | KeyCode::Char('B') => Some(Action::VisualizerToggleBBox),
        KeyCode::Char('h') | KeyCode::Char('H') => Some(Action::VisualizerToggleHeat),
        KeyCode::Char('l') | KeyCode::Char('L') => Some(Action::VisualizerToggleScanlines),
        KeyCode::Char('s') | KeyCode::Char('S') => Some(Action::VisualizerCycleSymmetry),
        _ => None,
    }
}

pub(super) fn visualizer_inspector_action(
    key: &KeyEvent,
    state: &AppState,
    input: &mut InputState,
) -> Option<Action> {
    if input.visualizer_jump.is_some() {
        match key.code {
            KeyCode::Char(c) if c.is_ascii_digit() && key.modifiers.is_empty() => {
                input.push_visualizer_digit(c as u8 - b'0');
                return None;
            }
            KeyCode::Backspace => {
                input.pop_visualizer_digit();
                return None;
            }
            KeyCode::Enter => {
                let value = input
                    .visualizer_jump
                    .as_ref()
                    .map(|jump| jump.value)
                    .unwrap_or(0);
                input.visualizer_jump = None;
                return Some(Action::VisualizerInspectJump(value));
            }
            _ => {
                input.visualizer_jump = None;
                return None;
            }
        }
    }

    if !state.visualizer.inspector_enabled {
        if matches!(key.code, KeyCode::Char('i') | KeyCode::Char('I'))
            && (key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT)
        {
            return Some(Action::VisualizerInspectToggle);
        }
        return None;
    }

    match key.code {
        KeyCode::Home => return Some(Action::VisualizerInspectHome),
        KeyCode::End => return Some(Action::VisualizerInspectEnd),
        _ => {}
    }

    if key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT {
        match key.code {
            KeyCode::Left | KeyCode::Char('h') | KeyCode::Char('H') => {
                return Some(Action::VisualizerInspectLeft)
            }
            KeyCode::Right | KeyCode::Char('l') | KeyCode::Char('L') => {
                return Some(Action::VisualizerInspectRight)
            }
            KeyCode::Up | KeyCode::Char('k') | KeyCode::Char('K') => {
                return Some(Action::VisualizerInspectUp)
            }
            KeyCode::Down | KeyCode::Char('j') | KeyCode::Char('J') => {
                return Some(Action::VisualizerInspectDown)
            }
            KeyCode::Char('0') => return Some(Action::VisualizerInspectHome),
            KeyCode::Char('$') => return Some(Action::VisualizerInspectEnd),
            KeyCode::Char('c') | KeyCode::Char('C') => {
                return Some(Action::VisualizerInspectCenter)
            }
            KeyCode::Char('i') | KeyCode::Char('I') => {
                return Some(Action::VisualizerInspectToggle)
            }
            KeyCode::Char('g') | KeyCode::Char('G') => {
                input.start_visualizer_jump();
                return None;
            }
            _ => {}
        }
    }
    None
}
