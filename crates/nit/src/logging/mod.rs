use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};

use nit_utils::hashing::stable_hash_bytes;
use nit_utils::paths;

mod channel_writer;

use channel_writer::{LogWriter, SharedFile};

pub(crate) fn init_tracing(
    tx: mpsc::Sender<String>,
    log_path: Option<PathBuf>,
) -> anyhow::Result<()> {
    let file: Option<SharedFile> = log_path
        .as_ref()
        .and_then(|p| open_log_file(p).ok().map(|f| Arc::new(Mutex::new(f))));

    tracing_subscriber::fmt()
        .with_writer(LogWriter::new(tx, file))
        .with_ansi(false)
        .with_env_filter("info,nit_syntax::tree_sitter_engine=error")
        .try_init()
        .ok(); // tolerate a subscriber already installed in tests

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
