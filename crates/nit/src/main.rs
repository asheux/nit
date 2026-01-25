#![forbid(unsafe_code)]

use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::fs;

use anyhow::Context;
use clap::Parser;
use nit_core::{io as core_io, Buffer, Mode};
use nit_tui::{run, Theme};
use nit_utils::hashing::stable_hash_bytes;
use nit_utils::paths;
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
    let notes = load_notes(&workspace_root);

    let theme_path = find_theme();
    let theme = Theme::load(theme_path.as_deref());

    let (log_tx, log_rx) = mpsc::channel::<String>();
    let log_path = log_path_for_workspace(&workspace_root);
    init_tracing(log_tx, log_path)?;
    install_panic_hook();

    let mut state = nit_core::AppState::new(workspace_root, editor, notes);
    let seed = stable_hash_bytes(state.editor_buffer().content_as_string().as_bytes());
    state.visualizer.seed = seed;
    state.mode = Mode::Normal;

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

fn load_notes(workspace_root: &Path) -> Buffer {
    let Some(path) = notes_path_for_workspace(workspace_root) else {
        return Buffer::empty("notes", None);
    };
    if path.exists() {
        if let Ok(content) = core_io::load_to_string(&path) {
            return Buffer::from_str("notes", &content, Some(path));
        }
    }
    Buffer::empty("notes", Some(path))
}

fn notes_path_for_workspace(workspace_root: &Path) -> Option<PathBuf> {
    let base = paths::state_dir().or_else(paths::data_dir)?;
    let notes_dir = base.join("notes");
    let _ = fs::create_dir_all(&notes_dir);
    let key = workspace_root.to_string_lossy();
    let hash = stable_hash_bytes(key.as_bytes());
    let filename = format!("{:016x}.md", hash);
    Some(notes_dir.join(filename))
}

fn init_tracing(tx: mpsc::Sender<String>, log_path: Option<PathBuf>) -> anyhow::Result<()> {
    let file = log_path
        .as_ref()
        .and_then(|path| open_log_file(path).ok())
        .map(|file| Arc::new(Mutex::new(file)));
    let writer = LogWriter { tx, file };
    tracing_subscriber::fmt()
        .with_writer(writer)
        .with_ansi(false)
        .with_env_filter("info")
        .try_init()
        .ok();
    if let Some(path) = log_path {
        tracing::info!("Log file: {}", path.display());
    }
    Ok(())
}

fn install_panic_hook() {
    std::panic::set_hook(Box::new(|info| {
        tracing::error!("PANIC: {info}");
        let bt = std::backtrace::Backtrace::force_capture();
        tracing::error!("BACKTRACE: {bt:?}");
    }));
}

#[derive(Clone)]
struct LogWriter {
    tx: mpsc::Sender<String>,
    file: Option<Arc<Mutex<std::fs::File>>>,
}

impl<'a> MakeWriter<'a> for LogWriter {
    type Writer = ChannelWriter;

    fn make_writer(&'a self) -> Self::Writer {
        ChannelWriter {
            tx: self.tx.clone(),
            buf: Vec::new(),
            file: self.file.clone(),
        }
    }
}

struct ChannelWriter {
    tx: mpsc::Sender<String>,
    buf: Vec<u8>,
    file: Option<Arc<Mutex<std::fs::File>>>,
}

impl Write for ChannelWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.buf.extend_from_slice(buf);
        self.drain_lines();
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.drain_lines();
        if !self.buf.is_empty() {
            let msg = String::from_utf8_lossy(&self.buf).trim().to_string();
            if !msg.is_empty() {
                if let Some(file) = &self.file {
                    if let Ok(mut file) = file.lock() {
                        let _ = writeln!(file, "{}", msg);
                    }
                }
                let _ = self.tx.send(msg);
            }
            self.buf.clear();
        }
        Ok(())
    }
}

impl ChannelWriter {
    fn drain_lines(&mut self) {
        loop {
            let Some(pos) = self.buf.iter().position(|b| *b == b'\n') else {
                break;
            };
            let line_bytes: Vec<u8> = self.buf.drain(..=pos).collect();
            let line = String::from_utf8_lossy(&line_bytes).trim().to_string();
            if !line.is_empty() {
                if let Some(file) = &self.file {
                    if let Ok(mut file) = file.lock() {
                        let _ = writeln!(file, "{}", line);
                    }
                }
                let _ = self.tx.send(line);
            }
        }
    }
}

fn log_path_for_workspace(workspace_root: &Path) -> Option<PathBuf> {
    if let Ok(path) = std::env::var("NIT_LOG_PATH") {
        return Some(PathBuf::from(path));
    }
    let base = paths::state_dir().or_else(paths::data_dir)?;
    let logs_dir = base.join("logs");
    let _ = fs::create_dir_all(&logs_dir);
    let key = workspace_root.to_string_lossy();
    let hash = stable_hash_bytes(key.as_bytes());
    let filename = format!("{:016x}.log", hash);
    Some(logs_dir.join(filename))
}

fn open_log_file(path: &Path) -> io::Result<std::fs::File> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
}
