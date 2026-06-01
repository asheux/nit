use std::path::Path;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use nit_core::TerminalPopupState;
use ratatui::layout::Rect;

use super::popup_rect;
use crate::app::popup_keys::{is_terminal_popup_toggle_key, terminal_popup_key, TerminalPopupKey};

fn key(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
    KeyEvent::new(code, modifiers)
}

#[test]
fn popup_rect_is_centered_and_55pct_wide() {
    let area = popup_rect(Rect::new(0, 0, 100, 40));
    assert_eq!(area.width, 55);
    assert_eq!(area.height, 28);
    assert_eq!(area.x, 22);
    assert_eq!(area.y, 6);
}

#[test]
fn popup_rect_never_overflows_a_cramped_screen() {
    let screen = Rect::new(0, 0, 12, 5);
    let area = popup_rect(screen);
    assert!(area.right() <= screen.right());
    assert!(area.bottom() <= screen.bottom());
}

#[test]
fn first_open_pins_cwd_and_shows() {
    let mut popup = TerminalPopupState::default();
    popup.apply_toggle(Path::new("/a"), false);
    assert!(popup.visible);
    assert_eq!(popup.cwd.as_deref(), Some(Path::new("/a")));
}

#[test]
fn toggle_while_visible_hides_and_keeps_cwd() {
    let mut popup = TerminalPopupState::default();
    popup.apply_toggle(Path::new("/a"), false);
    popup.apply_toggle(Path::new("/b"), false);
    assert!(!popup.visible);
    assert_eq!(popup.cwd.as_deref(), Some(Path::new("/a")));
}

#[test]
fn reopen_with_live_shell_does_not_repin_cwd() {
    let mut popup = TerminalPopupState::default();
    popup.apply_toggle(Path::new("/a"), false);
    popup.apply_toggle(Path::new("/b"), false);
    popup.apply_toggle(Path::new("/b"), false);
    assert!(popup.visible);
    assert_eq!(popup.cwd.as_deref(), Some(Path::new("/a")));
}

#[test]
fn reopen_after_shell_exit_repins_cwd() {
    let mut popup = TerminalPopupState::default();
    popup.apply_toggle(Path::new("/a"), false);
    popup.apply_toggle(Path::new("/b"), false);
    popup.apply_toggle(Path::new("/b"), true);
    assert!(popup.visible);
    assert_eq!(popup.cwd.as_deref(), Some(Path::new("/b")));
}

#[test]
fn close_chords_are_intercepted() {
    let toggle = key(
        KeyCode::Char('T'),
        KeyModifiers::CONTROL | KeyModifiers::SHIFT,
    );
    assert!(is_terminal_popup_toggle_key(&toggle));
    assert!(matches!(
        terminal_popup_key(&toggle),
        TerminalPopupKey::Close
    ));
}

#[test]
fn first_esc_is_forwarded_to_the_shell_only() {
    // First Esc must reach the PTY so vim / less / fzf can react.
    // Clear any tracker state left over by other tests to make the
    // assertion deterministic.
    crate::app::popup_keys::clear_popup_esc_state();
    let esc = key(KeyCode::Esc, KeyModifiers::NONE);
    assert!(matches!(
        terminal_popup_key(&esc),
        TerminalPopupKey::Forward(bytes) if bytes == b"\x1b".to_vec()
    ));
    crate::app::popup_keys::clear_popup_esc_state();
}

#[test]
fn second_esc_within_window_forwards_and_closes() {
    crate::app::popup_keys::clear_popup_esc_state();
    let esc = key(KeyCode::Esc, KeyModifiers::NONE);
    // Prime the tracker with a first Esc.
    assert!(matches!(
        terminal_popup_key(&esc),
        TerminalPopupKey::Forward(_)
    ));
    // Second Esc within the ~500ms window still reaches the shell
    // (so vim sees the chord) AND tells the runner to hide the popup.
    assert!(matches!(
        terminal_popup_key(&esc),
        TerminalPopupKey::ForwardAndClose(bytes) if bytes == b"\x1b".to_vec()
    ));
    crate::app::popup_keys::clear_popup_esc_state();
}

#[test]
fn typing_a_letter_between_escs_breaks_the_double_tap() {
    crate::app::popup_keys::clear_popup_esc_state();
    let esc = key(KeyCode::Esc, KeyModifiers::NONE);
    let letter = key(KeyCode::Char('a'), KeyModifiers::NONE);
    let _ = terminal_popup_key(&esc); // first Esc → tracker primed
    let _ = terminal_popup_key(&letter); // any other key resets tracker
    assert!(matches!(
        terminal_popup_key(&esc),
        TerminalPopupKey::Forward(_)
    ));
    crate::app::popup_keys::clear_popup_esc_state();
}

#[test]
fn other_keys_forward_their_bytes_to_the_shell() {
    let ctrl_c = key(KeyCode::Char('c'), KeyModifiers::CONTROL);
    assert!(matches!(
        terminal_popup_key(&ctrl_c),
        TerminalPopupKey::Forward(bytes) if bytes == vec![0x03]
    ));
    let plain = key(KeyCode::Char('a'), KeyModifiers::NONE);
    assert!(matches!(
        terminal_popup_key(&plain),
        TerminalPopupKey::Forward(bytes) if bytes == b"a".to_vec()
    ));
}

#[test]
fn ctrl_t_without_shift_is_not_the_toggle() {
    let ctrl_t = key(KeyCode::Char('t'), KeyModifiers::CONTROL);
    assert!(!is_terminal_popup_toggle_key(&ctrl_t));
    assert!(matches!(
        terminal_popup_key(&ctrl_t),
        TerminalPopupKey::Forward(_)
    ));
}
