//! OS-shell PTY: spawn/read/write/resize/kill. T6 owns; T7 consumes the
//! public API only. One reader thread feeds a `vt100::Parser` behind an
//! `Arc<Mutex>`; the UI thread locks it per render and pushes keystrokes
//! through `encode_key`. cwd is caller-side policy so this module stays pure.

use std::io::{self, Read, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::JoinHandle;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize as PtyDims};

/// Rows retained above the live grid for scrollback. Generous so a long build
/// log stays reachable by mouse-wheel; vt100 caps its history at this many rows.
const SCROLLBACK_LINES: usize = 10_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PtySize {
    pub rows: u16,
    pub cols: u16,
}

pub struct PtySession {
    master: Box<dyn MasterPty + Send>,
    child: Box<dyn Child + Send + Sync>,
    writer: Mutex<Box<dyn Write + Send>>,
    parser: Arc<Mutex<vt100::Parser>>,
    reader: Option<JoinHandle<()>>,
    exited: Arc<AtomicBool>,
    size: Mutex<PtySize>,
}

impl PtySession {
    /// Spawn `$SHELL` (fallback `/bin/sh`; `%COMSPEC%` on Windows) in `cwd`.
    pub fn spawn(cwd: &Path, size: PtySize) -> io::Result<Self> {
        Self::spawn_program(&default_shell(), &[], cwd, size)
    }

    /// Spawn an explicit program — used by tests to drive a deterministic
    /// command instead of an interactive shell.
    pub(crate) fn spawn_program(
        program: &str,
        args: &[&str],
        cwd: &Path,
        size: PtySize,
    ) -> io::Result<Self> {
        let pty_system = native_pty_system();
        let pair = pty_system.openpty(pty_dims(size)).map_err(to_io)?;
        let mut cmd = CommandBuilder::new(program);
        for arg in args {
            cmd.arg(arg);
        }
        cmd.cwd(cwd);
        // vt100 speaks xterm; advertising it keeps shell prompts/colours sane.
        cmd.env("TERM", "xterm-256color");
        let child = pair.slave.spawn_command(cmd).map_err(to_io)?;
        // Drop the slave handle so the master read sees EOF once the child dies.
        drop(pair.slave);
        let reader = pair.master.try_clone_reader().map_err(to_io)?;
        let writer = pair.master.take_writer().map_err(to_io)?;

        let parser = Arc::new(Mutex::new(vt100::Parser::new(
            size.rows,
            size.cols,
            SCROLLBACK_LINES,
        )));
        let exited = Arc::new(AtomicBool::new(false));
        let handle = spawn_reader(reader, parser.clone(), exited.clone());

        Ok(Self {
            master: pair.master,
            child,
            writer: Mutex::new(writer),
            parser,
            reader: Some(handle),
            exited,
            size: Mutex::new(size),
        })
    }

    /// Forward already-encoded operator bytes to the shell stdin.
    pub fn write_input(&self, bytes: &[u8]) -> io::Result<()> {
        // Typing snaps the viewport back to the live bottom, matching how a
        // real terminal behaves when you start typing while scrolled up.
        lock(&self.parser).screen_mut().set_scrollback(0);
        let mut writer = self
            .writer
            .lock()
            .map_err(|_| io::Error::other("pty writer poisoned"))?;
        writer.write_all(bytes)?;
        writer.flush()
    }

    /// Scroll the viewport `lines` rows toward older output. vt100 clamps the
    /// offset to the oldest retained row, so over-scrolling is a no-op.
    pub fn scroll_up(&self, lines: usize) {
        let mut parser = lock(&self.parser);
        let offset = parser.screen().scrollback();
        parser
            .screen_mut()
            .set_scrollback(offset.saturating_add(lines));
    }

    /// Scroll the viewport `lines` rows toward the live bottom (offset 0).
    pub fn scroll_down(&self, lines: usize) {
        let mut parser = lock(&self.parser);
        let offset = parser.screen().scrollback();
        parser
            .screen_mut()
            .set_scrollback(offset.saturating_sub(lines));
    }

    /// Push winsize to the pty + parser on resize; no-op when unchanged so a
    /// per-tick caller can blindly resync without thrashing the slave.
    pub fn resize(&self, size: PtySize) -> io::Result<()> {
        {
            let mut current = lock(&self.size);
            if *current == size {
                return Ok(());
            }
            *current = size;
        }
        self.master.resize(pty_dims(size)).map_err(to_io)?;
        lock(&self.parser)
            .screen_mut()
            .set_size(size.rows, size.cols);
        Ok(())
    }

    /// Lock the parsed screen grid for one render pass.
    pub fn screen(&self) -> MutexGuard<'_, vt100::Parser> {
        lock(&self.parser)
    }

    /// True once the child exited (reader EOF) — caller reverts pane / closes popup.
    pub fn has_exited(&self) -> bool {
        self.exited.load(Ordering::SeqCst)
    }

    /// OS pid of the spawned shell, or `None` if the platform backend
    /// doesn't expose one. Used by the popup title bar to poll the
    /// shell's live cwd via sysinfo so `cd` updates the header.
    pub fn process_id(&self) -> Option<u32> {
        self.child.process_id()
    }

    /// Idempotent kill + reader-thread join. Called by `Drop` AND quit teardown.
    pub fn shutdown(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Some(handle) = self.reader.take() {
            let _ = handle.join();
        }
        self.exited.store(true, Ordering::SeqCst);
    }
}

impl Drop for PtySession {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn spawn_reader(
    mut reader: Box<dyn Read + Send>,
    parser: Arc<Mutex<vt100::Parser>>,
    exited: Arc<AtomicBool>,
) -> JoinHandle<()> {
    std::thread::spawn(move || {
        let mut buf = [0u8; 4096];
        loop {
            match reader.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => lock(&parser).process(&buf[..n]),
            }
        }
        exited.store(true, Ordering::SeqCst);
    })
}

/// `Ctrl+\` toggles the chat pane to/from the terminal. Crossterm maps the
/// `0x1C` byte to `'\'` + CONTROL on legacy terminals; the raw C0 form is
/// matched too for terminals that deliver it directly.
pub fn is_terminal_toggle_key(key: &KeyEvent) -> bool {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    matches!(key.code, KeyCode::Char('\\') if ctrl) || matches!(key.code, KeyCode::Char('\u{1c}'))
}

/// The single crossterm-key → PTY-bytes encoder shared by T6 and T7. Returns
/// `None` for keys with no terminal byte sequence (the caller drops them).
pub fn encode_key(key: KeyEvent) -> Option<Vec<u8>> {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let alt = key.modifiers.contains(KeyModifiers::ALT);
    let base = match key.code {
        KeyCode::Char(c) => encode_char(c, ctrl)?,
        KeyCode::Enter => vec![b'\r'],
        KeyCode::Tab => vec![b'\t'],
        KeyCode::BackTab => b"\x1b[Z".to_vec(),
        KeyCode::Backspace => vec![0x7f],
        KeyCode::Esc => vec![0x1b],
        KeyCode::Left => b"\x1b[D".to_vec(),
        KeyCode::Right => b"\x1b[C".to_vec(),
        KeyCode::Up => b"\x1b[A".to_vec(),
        KeyCode::Down => b"\x1b[B".to_vec(),
        KeyCode::Home => b"\x1b[H".to_vec(),
        KeyCode::End => b"\x1b[F".to_vec(),
        KeyCode::PageUp => b"\x1b[5~".to_vec(),
        KeyCode::PageDown => b"\x1b[6~".to_vec(),
        KeyCode::Delete => b"\x1b[3~".to_vec(),
        KeyCode::Insert => b"\x1b[2~".to_vec(),
        KeyCode::F(n) => function_key(n)?,
        _ => return None,
    };
    if alt {
        // Meta/Alt is the ESC prefix in xterm-style encodings.
        let mut out = Vec::with_capacity(base.len() + 1);
        out.push(0x1b);
        out.extend_from_slice(&base);
        Some(out)
    } else {
        Some(base)
    }
}

fn encode_char(c: char, ctrl: bool) -> Option<Vec<u8>> {
    if !ctrl {
        let mut buf = [0u8; 4];
        return Some(c.encode_utf8(&mut buf).as_bytes().to_vec());
    }
    let byte = match c {
        ' ' | '@' | '2' => 0x00,
        'a'..='z' => c as u8 - b'a' + 1,
        'A'..='Z' => c as u8 - b'A' + 1,
        '[' => 0x1b,
        '\\' => 0x1c,
        ']' => 0x1d,
        '^' | '6' => 0x1e,
        '_' | '/' | '?' => 0x1f,
        _ => return None,
    };
    Some(vec![byte])
}

fn function_key(n: u8) -> Option<Vec<u8>> {
    let seq: &[u8] = match n {
        1 => b"\x1bOP",
        2 => b"\x1bOQ",
        3 => b"\x1bOR",
        4 => b"\x1bOS",
        5 => b"\x1b[15~",
        6 => b"\x1b[17~",
        7 => b"\x1b[18~",
        8 => b"\x1b[19~",
        9 => b"\x1b[20~",
        10 => b"\x1b[21~",
        11 => b"\x1b[23~",
        12 => b"\x1b[24~",
        _ => return None,
    };
    Some(seq.to_vec())
}

fn default_shell() -> String {
    #[cfg(windows)]
    {
        std::env::var("COMSPEC").unwrap_or_else(|_| "cmd.exe".to_string())
    }
    #[cfg(not(windows))]
    {
        std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string())
    }
}

fn pty_dims(size: PtySize) -> PtyDims {
    PtyDims {
        rows: size.rows,
        cols: size.cols,
        pixel_width: 0,
        pixel_height: 0,
    }
}

// portable-pty surfaces `anyhow::Error`; staying generic keeps nit-tui off a
// direct anyhow dependency while still flattening to `io::Error`.
fn to_io<E: std::fmt::Display>(err: E) -> io::Error {
    io::Error::other(err.to_string())
}

// Poison recovery: a panicked render/reader thread must not wedge the PTY —
// the grid is plain data, so reusing the inner value is safe.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
#[path = "tests/pty.rs"]
mod tests;
