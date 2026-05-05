use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use nit_core::{AppKind, AppState, PaneId};

use super::input_state::FocusDir;

pub(super) fn map_focus_hotkey(key: &KeyEvent) -> Option<PaneId> {
    match key {
        KeyEvent {
            code: KeyCode::Char('1'),
            modifiers,
            ..
        } if modifiers.contains(KeyModifiers::CONTROL) => Some(PaneId::Editor),
        KeyEvent {
            code: KeyCode::Char('2'),
            modifiers,
            ..
        } if modifiers.contains(KeyModifiers::CONTROL) => Some(PaneId::JobOutput),
        KeyEvent {
            code: KeyCode::Char('3'),
            modifiers,
            ..
        } if modifiers.contains(KeyModifiers::CONTROL) => Some(PaneId::Notes),
        _ => None,
    }
}

pub(super) fn is_global_run_key(key: &KeyEvent) -> bool {
    matches!(
        key,
        KeyEvent {
            code: KeyCode::Enter | KeyCode::Char('\n') | KeyCode::Char('\r'),
            modifiers,
            ..
        } if modifiers.contains(KeyModifiers::CONTROL)
    )
}

pub(crate) fn is_global_quit_key(key: &KeyEvent) -> bool {
    matches!(
        key,
        KeyEvent {
            code: KeyCode::Char('q'),
            modifiers,
            ..
        } if modifiers.contains(KeyModifiers::CONTROL)
    )
}

pub(super) fn is_command_prompt_open_key(key: &KeyEvent) -> bool {
    match key {
        KeyEvent {
            code: KeyCode::Char(':'),
            ..
        } => true,
        KeyEvent {
            code: KeyCode::Char(';'),
            modifiers,
            ..
        } => modifiers.contains(KeyModifiers::SHIFT),
        _ => false,
    }
}

pub(super) fn is_help_toggle_key(key: &KeyEvent) -> bool {
    matches!(
        key,
        KeyEvent {
            code: KeyCode::F(1),
            ..
        }
    ) || matches!(
        key,
        KeyEvent {
            code: KeyCode::Char('?'),
            modifiers,
            ..
        } if modifiers.is_empty() || *modifiers == KeyModifiers::SHIFT
    )
}

pub(super) fn is_petri_show_key(key: &KeyEvent, state: &AppState) -> bool {
    match state.app_kind {
        AppKind::Gol => {
            if !state.visualizer.petri_hidden || !state.visualizer.running {
                return false;
            }
        }
        AppKind::Games => {
            if !state.games.petri_hidden || !games_petri_active(state) {
                return false;
            }
        }
    }
    match key {
        KeyEvent {
            code: KeyCode::Char('^') | KeyCode::Char('6'),
            modifiers,
            ..
        } if modifiers.contains(KeyModifiers::CONTROL) => true,
        KeyEvent {
            code: KeyCode::Char('\u{1e}'),
            modifiers,
            ..
        } if modifiers.is_empty() || modifiers.contains(KeyModifiers::CONTROL) => true,
        _ => false,
    }
}

pub(super) fn games_petri_active(state: &AppState) -> bool {
    state.games.running
        || matches!(
            state.games.status,
            nit_core::GamesStatus::Paused | nit_core::GamesStatus::Done
        )
        || !state.games.petri_lines.is_empty()
}

pub(super) fn is_games_history_open_key(key: &KeyEvent, state: &AppState) -> bool {
    if state.app_kind != AppKind::Games {
        return false;
    }
    if !state.games.running && state.games.last_run.is_none() {
        return false;
    }
    matches!(
        key,
        KeyEvent {
            code: KeyCode::Char('*') | KeyCode::Char('8'),
            modifiers,
            ..
        } if modifiers.contains(KeyModifiers::CONTROL)
    )
}

pub(super) fn games_petri_visible(state: &AppState) -> bool {
    state.app_kind == AppKind::Games && games_petri_active(state) && !state.games.petri_hidden
}

pub(super) fn current_games_config_result(
    state: &AppState,
) -> Option<&Result<nit_games::config::NormalizedConfig, nit_games::config::ConfigError>> {
    let version = state.editor_buffer().version();
    state
        .games
        .config_preview
        .as_ref()
        .and_then(|preview| (preview.version == version).then_some(&preview.result))
}

pub(super) fn games_modal_popup_open(state: &AppState) -> bool {
    if state.app_kind != AppKind::Games {
        return false;
    }
    state.games.analysis.open
        || state.games.run_browser.open
        || state.games.replay.open
        || state.games.strategy_inspect.open
        || state.games.tm_sim.open
        || state.games.ca_sim.open
        || state.games.match_history.open
}

pub(super) fn is_games_petri_control_key(key: &KeyEvent) -> bool {
    matches!(
        key.code,
        KeyCode::Char(' ')
            | KeyCode::Null
            | KeyCode::Char('\u{0}')
            | KeyCode::Enter
            | KeyCode::Char('\n')
            | KeyCode::Char('\r')
            | KeyCode::Char('+')
            | KeyCode::Char('=')
            | KeyCode::Char('-')
            | KeyCode::Char('_')
            | KeyCode::Tab
            | KeyCode::Left
            | KeyCode::Right
            | KeyCode::Up
            | KeyCode::Down
            | KeyCode::Char('c')
            | KeyCode::Char('C')
            | KeyCode::Char('h')
            | KeyCode::Char('H')
            | KeyCode::Char('r')
            | KeyCode::Char('R')
            | KeyCode::Char('x')
            | KeyCode::Char('X')
            | KeyCode::Char('y')
            | KeyCode::Char('Y')
            | KeyCode::Char('n')
            | KeyCode::Char('N')
    )
}

pub(super) fn is_job_pause_key(key: &KeyEvent) -> bool {
    match key.code {
        KeyCode::Char(' ') => key.modifiers.contains(KeyModifiers::CONTROL),
        KeyCode::Char('\u{0}') => key.modifiers.is_empty(),
        KeyCode::Null | KeyCode::F(6) => true,
        _ => false,
    }
}

pub(super) fn ctrl_nav_dir(key: &KeyEvent) -> Option<FocusDir> {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    if !ctrl || key.modifiers.contains(KeyModifiers::SHIFT) {
        return None;
    }
    match key.code {
        KeyCode::Char('h') if ctrl => Some(FocusDir::Left),
        KeyCode::Char('j') if ctrl => Some(FocusDir::Down),
        KeyCode::Char('k') if ctrl => Some(FocusDir::Up),
        KeyCode::Char('l') if ctrl => Some(FocusDir::Right),
        KeyCode::Backspace if ctrl => Some(FocusDir::Left),
        KeyCode::Enter if ctrl => Some(FocusDir::Down),
        KeyCode::Char('\u{8}') => Some(FocusDir::Left),
        KeyCode::Char('\n') => Some(FocusDir::Down),
        KeyCode::Char('\u{0b}') => Some(FocusDir::Up),
        KeyCode::Char('\u{0c}') => Some(FocusDir::Right),
        _ => None,
    }
}
