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
fn spawn_shell_command_runs_without_typed_input() {
    use std::time::Duration;
    let dir = std::env::temp_dir();
    let size = PtySize { rows: 24, cols: 80 };
    let mut session = match PtySession::spawn_shell_command(&dir, size, "printf nit-direct-command")
    {
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
    assert!(
        contents.contains("nit-direct-command"),
        "screen was {contents:?}"
    );
    session.shutdown();
}

#[cfg(unix)]
#[test]
fn foreground_program_can_publish_a_terminal_title() {
    use std::time::Duration;
    let dir = std::env::temp_dir();
    let session = match PtySession::spawn_shell_command(
        &dir,
        PtySize { rows: 24, cols: 80 },
        "printf '\\033]2;fraction reduction · cycle 4146\\007'; sleep 2",
    ) {
        Ok(session) => session,
        Err(_) => return,
    };

    for _ in 0..200 {
        if session.title().is_some() {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }

    assert_eq!(
        session.title().as_deref(),
        Some("fraction reduction · cycle 4146")
    );
}

#[test]
fn adjacent_pty_chunks_are_collected_before_the_parser_is_published() {
    use std::sync::atomic::AtomicBool;
    use std::sync::{mpsc, Arc, Mutex};

    let parser = Arc::new(Mutex::new(TerminalParser::new_with_callbacks(
        4,
        40,
        0,
        TerminalMetadata::default(),
    )));
    let exited = Arc::new(AtomicBool::new(false));
    let (tx, rx) = mpsc::channel();
    let worker = spawn_parser(rx, parser.clone(), exited.clone());

    tx.send(b"old frame".to_vec()).unwrap();
    std::thread::sleep(PARSE_BATCH_DELAY * 2);
    tx.send(b"\x1b[H\x1b[2J".to_vec()).unwrap();
    tx.send(b"new frame".to_vec()).unwrap();
    drop(tx);
    worker.join().unwrap();

    let contents = lock(&parser).screen().contents();
    assert!(contents.contains("new frame"), "screen was {contents:?}");
    assert!(!contents.contains("old frame"), "screen was {contents:?}");
    assert!(exited.load(Ordering::SeqCst));
}

#[cfg(unix)]
#[test]
fn scroll_up_enters_scrollback_and_input_snaps_to_bottom() {
    use std::time::Duration;
    let dir = std::env::temp_dir();
    // Emit far more lines than the 10-row grid so scrollback fills, then idle
    // so the session stays alive for write_input.
    let session = match PtySession::spawn_program(
        "/bin/sh",
        &["-c", "for i in $(seq 1 200); do echo line$i; done; sleep 5"],
        &dir,
        PtySize { rows: 10, cols: 40 },
    ) {
        Ok(session) => session,
        Err(_) => return,
    };
    let mut ready = false;
    for _ in 0..200 {
        if session.screen().screen().contents().contains("line200") {
            ready = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    if !ready {
        return; // shell too slow / unavailable — degrade to no-op.
    }
    // Live view sits at the bottom.
    assert_eq!(session.screen().screen().scrollback(), 0);
    session.scroll_up(5);
    assert_eq!(session.screen().screen().scrollback(), 5);
    session.scroll_down(2);
    assert_eq!(session.screen().screen().scrollback(), 3);
    // Typing snaps the viewport back to the live bottom.
    let _ = session.write_input(b"");
    assert_eq!(session.screen().screen().scrollback(), 0);
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
