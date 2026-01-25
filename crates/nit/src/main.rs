#![forbid(unsafe_code)]

use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc;

use anyhow::Context;
use clap::Parser;
use nit_core::{io as core_io, Buffer, Mode};
use nit_tui::{run, Theme};
use nit_utils::hashing::stable_hash_bytes;
use tracing_subscriber::fmt::MakeWriter;

#[derive(Parser, Debug)]
#[command(name = "nit", version, about = "Neural Interface Terminal")]
struct Cli {
    /// File or directory to open
    path: Option<PathBuf>,
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let (workspace_root, editor) = open_target(cli.path.as_deref())?;
    let notes = Buffer::empty("notes", None);

    let theme_path = find_theme();
    let theme = Theme::load(theme_path.as_deref());

    let (log_tx, log_rx) = mpsc::channel::<String>();
    init_tracing(log_tx)?;

    let mut state = nit_core::AppState::new(workspace_root, editor, notes);
    let seed = stable_hash_bytes(state.editor_buffer().content_as_string().as_bytes());
    state.visualizer.seed = seed;
    state.mode = Mode::Insert;

    run(state, theme, log_rx)?;
    Ok(())
}

fn open_target(path: Option<&Path>) -> anyhow::Result<(PathBuf, Buffer)> {
    match path {
        Some(p) if p.is_file() => {
            let content = core_io::load_to_string(p)
                .with_context(|| format!("failed to read {}", p.display()))?;
            let name = p
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| "untitled".into());
            let buffer = Buffer::from_str(name, &content, Some(p.to_path_buf()));
            let root = p
                .parent()
                .map(|p| p.to_path_buf())
                .unwrap_or(std::env::current_dir()?);
            Ok((root, buffer))
        }
        Some(p) if p.is_dir() => {
            let root = p.to_path_buf();
            let buffer = Buffer::empty("untitled", None);
            Ok((root, buffer))
        }
        None => {
            let root = std::env::current_dir()?;
            let buffer = Buffer::empty("untitled", None);
            Ok((root, buffer))
        }
        Some(other) => anyhow::bail!("path does not exist: {}", other.display()),
    }
}

fn find_theme() -> Option<PathBuf> {
    let cwd = std::env::current_dir().ok()?;
    let local = cwd.join("assets/themes/devs.toml");
    if local.exists() {
        return Some(local);
    }
    None
}

fn init_tracing(tx: mpsc::Sender<String>) -> anyhow::Result<()> {
    let writer = LogWriter { tx };
    tracing_subscriber::fmt()
        .with_writer(writer)
        .with_ansi(false)
        .with_env_filter("info")
        .try_init()
        .ok();
    Ok(())
}

#[derive(Clone)]
struct LogWriter {
    tx: mpsc::Sender<String>,
}

impl<'a> MakeWriter<'a> for LogWriter {
    type Writer = ChannelWriter;

    fn make_writer(&'a self) -> Self::Writer {
        ChannelWriter {
            tx: self.tx.clone(),
            buf: Vec::new(),
        }
    }
}

struct ChannelWriter {
    tx: mpsc::Sender<String>,
    buf: Vec<u8>,
}

impl Write for ChannelWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.buf.extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        if !self.buf.is_empty() {
            let msg = String::from_utf8_lossy(&self.buf).to_string();
            let _ = self.tx.send(msg);
            self.buf.clear();
        }
        Ok(())
    }
}
