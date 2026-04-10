use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};

use nit_utils::hashing::stable_hash_bytes;
use nit_utils::paths;
use tracing_subscriber::fmt::MakeWriter;

type SharedFile = Arc<Mutex<fs::File>>;

pub(crate) fn init_tracing(
    tx: mpsc::Sender<String>,
    log_path: Option<PathBuf>,
) -> anyhow::Result<()> {
    let file: Option<SharedFile> = log_path
        .as_ref()
        .and_then(|p| open_log_file(p).ok())
        .map(|f| Arc::new(Mutex::new(f)));

    tracing_subscriber::fmt()
        .with_writer(LogWriter { tx, file })
        .with_ansi(false)
        .with_env_filter("info,nit_syntax::tree_sitter_engine=error")
        .try_init()
        .ok(); // Ignore if another subscriber is already installed.

    if let Some(path) = log_path {
        tracing::info!("log file: {}", path.display());
    }
    Ok(())
}

pub(crate) fn install_panic_hook() {
    std::panic::set_hook(Box::new(|info| {
        tracing::error!("PANIC: {info}");
        let bt = std::backtrace::Backtrace::force_capture();
        tracing::error!("BACKTRACE: {bt:?}");
    }));
}

#[derive(Clone)]
struct LogWriter {
    tx: mpsc::Sender<String>,
    file: Option<SharedFile>,
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
    file: Option<SharedFile>,
}

impl ChannelWriter {
    /// Emit complete lines from the buffer to both the log file and the TUI channel.
    fn drain_lines(&mut self) {
        while let Some(newline_pos) = self.buf.iter().position(|&b| b == b'\n') {
            let trimmed_line = String::from_utf8_lossy(&self.buf[..=newline_pos])
                .trim()
                .to_string();
            self.buf.drain(..=newline_pos);
            if trimmed_line.is_empty() {
                continue;
            }
            self.emit(&trimmed_line);
        }
    }

    fn emit(&self, log_line: &str) {
        let file_guard = self.file.as_ref().and_then(|f| f.lock().ok());
        if let Some(mut handle) = file_guard {
            let _ = writeln!(handle, "{log_line}");
        }
        let _ = self.tx.send(log_line.to_string());
    }
}

impl Write for ChannelWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.buf.extend_from_slice(buf);
        self.drain_lines();
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.drain_lines();
        let trailing = String::from_utf8_lossy(&self.buf).trim().to_string();
        if !trailing.is_empty() {
            self.emit(&trailing);
        }
        self.buf.clear();
        Ok(())
    }
}

pub(crate) fn log_path_for_workspace(workspace_root: &Path) -> Option<PathBuf> {
    if let Ok(path) = std::env::var("NIT_LOG_PATH") {
        return Some(PathBuf::from(path));
    }
    let base = paths::state_dir().or_else(paths::data_dir)?;
    let logs_dir = base.join("logs");
    let _ = fs::create_dir_all(&logs_dir);
    let hash = stable_hash_bytes(workspace_root.to_string_lossy().as_bytes());
    Some(logs_dir.join(format!("{hash:016x}.log")))
}

fn open_log_file(path: &Path) -> io::Result<fs::File> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::OpenOptions::new().create(true).append(true).open(path)
}
