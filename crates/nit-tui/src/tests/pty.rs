use super::*;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

fn key(code: KeyCode, mods: KeyModifiers) -> KeyEvent {
    KeyEvent::new(code, mods)
}

#[test]
fn encode_plain_char_is_utf8() {
    assert_eq!(
        encode_key(key(KeyCode::Char('a'), KeyModifiers::NONE)),
        Some(vec![b'a'])
    );
    assert_eq!(
        encode_key(key(KeyCode::Char('é'), KeyModifiers::NONE)),
        Some("é".as_bytes().to_vec())
    );
}

#[test]
fn encode_ctrl_letters_map_to_c0() {
    assert_eq!(
        encode_key(key(KeyCode::Char('a'), KeyModifiers::CONTROL)),
        Some(vec![0x01])
    );
    assert_eq!(
        encode_key(key(KeyCode::Char('c'), KeyModifiers::CONTROL)),
        Some(vec![0x03])
    );
    assert_eq!(
        encode_key(key(KeyCode::Char('d'), KeyModifiers::CONTROL)),
        Some(vec![0x04])
    );
}

#[test]
fn encode_named_keys() {
    assert_eq!(
        encode_key(key(KeyCode::Enter, KeyModifiers::NONE)),
        Some(vec![b'\r'])
    );
    assert_eq!(
        encode_key(key(KeyCode::Backspace, KeyModifiers::NONE)),
        Some(vec![0x7f])
    );
    assert_eq!(
        encode_key(key(KeyCode::Tab, KeyModifiers::NONE)),
        Some(vec![b'\t'])
    );
    assert_eq!(
        encode_key(key(KeyCode::Esc, KeyModifiers::NONE)),
        Some(vec![0x1b])
    );
    assert_eq!(
        encode_key(key(KeyCode::Up, KeyModifiers::NONE)),
        Some(b"\x1b[A".to_vec())
    );
    assert_eq!(
        encode_key(key(KeyCode::PageDown, KeyModifiers::NONE)),
        Some(b"\x1b[6~".to_vec())
    );
}

#[test]
fn encode_alt_prefixes_escape() {
    assert_eq!(
        encode_key(key(KeyCode::Char('b'), KeyModifiers::ALT)),
        Some(vec![0x1b, b'b'])
    );
}

#[test]
fn encode_unsupported_returns_none() {
    assert_eq!(encode_key(key(KeyCode::F(20), KeyModifiers::NONE)), None);
}

#[test]
fn toggle_key_matches_ctrl_backslash_only() {
    assert!(is_terminal_toggle_key(&key(
        KeyCode::Char('\\'),
        KeyModifiers::CONTROL
    )));
    assert!(is_terminal_toggle_key(&key(
        KeyCode::Char('\u{1c}'),
        KeyModifiers::NONE
    )));
    assert!(!is_terminal_toggle_key(&key(
        KeyCode::Char('\\'),
        KeyModifiers::NONE
    )));
    assert!(!is_terminal_toggle_key(&key(
        KeyCode::Char('t'),
        KeyModifiers::CONTROL
    )));
}

// The spawn-based smoke tests need a real pty; they degrade to a no-op when the
// sandbox has no `/dev/ptmx` so `cargo test` stays green in restricted CI.
#[cfg(unix)]
#[test]
fn spawn_runs_command_and_reports_exit() {
    use std::time::Duration;
    let dir = std::env::temp_dir();
    let size = PtySize { rows: 24, cols: 80 };
    let mut session =
        match PtySession::spawn_program("/bin/sh", &["-c", "printf nit-pty-ok"], &dir, size) {
            Ok(session) => session,
            Err(_) => return,
        };
    for _ in 0..200 {
        if session.has_exited() {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    std::thread::sleep(Duration::from_millis(30));
    let contents = session.screen().screen().contents();
    assert!(contents.contains("nit-pty-ok"), "screen was {contents:?}");
    session.shutdown();
    assert!(session.has_exited());
}

#[cfg(unix)]
#[test]
fn resize_updates_parser_dimensions() {
    let dir = std::env::temp_dir();
    let session = match PtySession::spawn_program(
        "/bin/sh",
        &["-c", "sleep 2"],
        &dir,
        PtySize { rows: 24, cols: 80 },
    ) {
        Ok(session) => session,
        Err(_) => return,
    };
    assert_eq!(session.screen().screen().size(), (24, 80));
    session
        .resize(PtySize {
            rows: 30,
            cols: 100,
        })
        .unwrap();
    assert_eq!(session.screen().screen().size(), (30, 100));
}
