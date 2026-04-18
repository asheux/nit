//! I/O worker thread: receives commands and writes snapshots to disk.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::thread::{self, JoinHandle};

use crossbeam_channel::Receiver;
use tracing::{info, warn};

use super::manager::SnapshotManagerInner;
use super::rule_log::{self, RuleLogEntry};
use super::types::{SnapshotRequest, IO_THREAD_STACK_BYTES, SNAPSHOT_FILENAME_PREFIX};
use crate::snapshot::{self, ensure_dir, write_metadata_atomic, write_rle_bits_atomic};

const WORKER_THREAD_NAME: &str = "nit-gol-io";

pub(super) enum IoCommand {
    Snapshot(Box<SnapshotRequest>),
    RecordRule(RuleLogEntry),
    Shutdown,
}

/// Spawn the background I/O worker. Pre-creates the snapshot directory
/// so the first write doesn't race against directory creation.
pub(super) fn spawn_worker(
    rx: Receiver<IoCommand>,
    inner: Arc<SnapshotManagerInner>,
) -> Option<JoinHandle<()>> {
    if let Err(err) = fs::create_dir_all(&inner.dir) {
        warn!("Snapshot dir init failed: {}", err);
    }
    let builder = thread::Builder::new()
        .name(WORKER_THREAD_NAME.into())
        .stack_size(IO_THREAD_STACK_BYTES);
    match builder.spawn(move || worker_loop(rx, inner)) {
        Ok(handle) => Some(handle),
        Err(err) => {
            warn!("Failed to spawn snapshot worker: {}", err);
            None
        }
    }
}

fn worker_loop(rx: Receiver<IoCommand>, inner: Arc<SnapshotManagerInner>) {
    while let Ok(cmd) = rx.recv() {
        if !handle_command(cmd, &inner) {
            break;
        }
    }
}

/// Dispatch one command. Returns `false` on `Shutdown` so the loop exits.
fn handle_command(cmd: IoCommand, inner: &SnapshotManagerInner) -> bool {
    match cmd {
        IoCommand::Snapshot(req) => report("Snapshot", handle_snapshot(*req, inner)),
        IoCommand::RecordRule(entry) => report("Snapshot rule log", rule_log::append(entry)),
        IoCommand::Shutdown => return false,
    }
    true
}

fn report(label: &str, result: io::Result<()>) {
    if let Err(err) = result {
        warn!("{} failed: {}", label, err);
    }
}

fn handle_snapshot(req: SnapshotRequest, inner: &SnapshotManagerInner) -> io::Result<()> {
    ensure_dir(&inner.dir)?;
    let rle_path = write_snapshot_files(&inner.dir, &req)?;
    record_write_success(inner, rle_path);
    let _ = snapshot::prune_oldest(&inner.dir, inner.max_files);
    Ok(())
}

fn write_snapshot_files(dir: &Path, req: &SnapshotRequest) -> io::Result<PathBuf> {
    let stem = build_filename_stem(req);
    let rle_path = dir.join(format!("{stem}.rle"));
    let json_path = dir.join(format!("{stem}.json"));
    write_rle_bits_atomic(&rle_path, req.width, req.height, &req.rule, &req.grid_bits)?;
    write_metadata_atomic(&json_path, &req.meta)?;
    Ok(rle_path)
}

/// The legacy stem embeds only the low 32 bits of the grid hash; widen
/// before calling the shared formatter so `:08x` produces the same 8-char
/// output that existing on-disk snapshots already use.
fn build_filename_stem(req: &SnapshotRequest) -> String {
    let hash_low32 = u64::from(req.grid_hash[0] as u32);
    snapshot::format_name_stem(
        Some(SNAPSHOT_FILENAME_PREFIX),
        &req.meta.timestamp,
        &req.rule,
        req.gen,
        hash_low32,
    )
}

fn record_write_success(inner: &SnapshotManagerInner, rle_path: PathBuf) {
    inner.written.fetch_add(1, Ordering::Relaxed);
    info!("Snapshot saved: {}", rle_path.display());
    *inner.last_path.lock().unwrap() = Some(rle_path);
}
