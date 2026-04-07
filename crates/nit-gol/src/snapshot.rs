//! Snapshot persistence: writing grid state to disk as RLE + JSON.
//!
//! Handles atomic file writes, metadata serialization, timestamp
//! generation, and snapshot pruning. RLE encoding logic lives in
//! the [`rle`](crate::rle) module; this module re-exports it for
//! backward compatibility.

use std::fs::{self, File};
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};

use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

use crate::{attractor::AttractorEvent, Grid, Rule};

// Re-export RLE encoding functions from their dedicated module.
pub use crate::rle::{encode_rle, write_rle, write_rle_bits};

/// Metadata written alongside each snapshot as a JSON sidecar file.
#[derive(Clone, Debug, serde::Serialize)]
pub struct SnapshotMetadata {
    pub timestamp: String,
    pub workspace_root: Option<String>,
    pub file_path: Option<String>,
    pub seed_source: String,
    pub seed_hash: u64,
    pub rule: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rule_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub protocol: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub protocol_hash: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub protocol_phase_idx: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub protocol_step_in_phase: Option<u32>,
    pub generation: u64,
    pub alive_count: usize,
    pub period: Option<u64>,
    pub score: Option<f32>,
    pub wrap_mode: String,
    pub tick_ms: u64,
    pub attractor: Option<AttractorEvent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub encoder_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub encoder_params: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params_fingerprint: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_hash: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seed_density: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seed_components: Option<usize>,
}

/// Paths to the RLE and JSON files produced by a snapshot write.
#[derive(Clone, Debug)]
pub struct SnapshotPaths {
    pub rle_path: PathBuf,
    pub json_path: PathBuf,
}

/// Write a complete snapshot (RLE grid + JSON metadata) to `dir`.
pub fn write_snapshot(
    dir: &Path,
    name_base: &str,
    grid: &Grid,
    rule: Rule,
    meta: &SnapshotMetadata,
) -> io::Result<SnapshotPaths> {
    snapshot_debug(|| {
        format!(
            "start name={} dir={} grid={}x{} rule={}",
            name_base,
            dir.display(),
            grid.width(),
            grid.height(),
            rule
        )
    });
    ensure_dir(dir)?;
    let rle_path = dir.join(format!("{name_base}.rle"));
    let json_path = dir.join(format!("{name_base}.json"));
    write_atomic(&rle_path, |writer| write_rle(writer, grid, rule))?;
    write_metadata_atomic(&json_path, meta)?;
    snapshot_debug(|| {
        format!(
            "done rle_path={} json_path={}",
            rle_path.display(),
            json_path.display()
        )
    });
    Ok(SnapshotPaths {
        rle_path,
        json_path,
    })
}

/// Build a default snapshot file-name stem from rule, generation, and hash.
pub fn default_name(rule: Rule, generation: u64, hash: u64) -> String {
    let timestamp = OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .unwrap_or_else(|_| "unknown-time".into())
        .replace(':', "-");
    format!(
        "{timestamp}__rule-{}__gen-{generation:05}__hash-{hash:08x}",
        rule.to_string().replace('/', "")
    )
}

/// Current UTC time as an ISO 8601 / RFC 3339 string.
pub fn now_iso8601() -> String {
    OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .unwrap_or_else(|_| "unknown-time".into())
}

/// Delete the oldest `.rle` (and companion `.json`) files in `dir`
/// until at most `max_files` remain.
pub fn prune_oldest(dir: &Path, max_files: usize) -> io::Result<()> {
    if max_files == 0 {
        return Ok(());
    }
    let mut entries: Vec<_> = fs::read_dir(dir)?
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let path = e.path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("rle") {
                return None;
            }
            let meta = e.metadata().ok()?;
            let modified = meta.modified().ok()?;
            Some((modified, path))
        })
        .collect();
    if entries.len() <= max_files {
        return Ok(());
    }
    entries.sort_by_key(|(time, _)| *time);
    let remove_count = entries.len().saturating_sub(max_files);
    for (_, path) in entries.into_iter().take(remove_count) {
        let _ = fs::remove_file(&path);
        let json_path = path.with_extension("json");
        let _ = fs::remove_file(json_path);
    }
    Ok(())
}

/// Atomically write an RLE file from packed grid bits.
pub fn write_rle_bits_atomic(
    path: &Path,
    width: u16,
    height: u16,
    rule: &str,
    bits: &[u64],
) -> io::Result<()> {
    write_atomic(path, |writer| {
        crate::rle::write_rle_bits(writer, width, height, rule, bits)
    })
}

/// Atomically write JSON metadata to a sidecar file.
pub fn write_metadata_atomic(path: &Path, meta: &SnapshotMetadata) -> io::Result<()> {
    write_atomic(path, |writer| {
        serde_json::to_writer(writer, meta).map_err(io::Error::other)
    })
}

// ── Internal utilities ──────────────────────────────────────────────

/// Ensure `dir` exists, creating it if necessary.
///
/// Rejects symlinks to prevent following links to unintended locations.
pub(crate) fn ensure_dir(dir: &Path) -> io::Result<()> {
    if let Ok(meta) = fs::symlink_metadata(dir) {
        if meta.file_type().is_symlink() {
            return Err(io::Error::other("snapshot dir is a symlink"));
        }
        if meta.is_dir() {
            return Ok(());
        }
    }
    fs::create_dir_all(dir)
}

/// Write to a temporary file then atomically rename into place.
fn write_atomic<F>(path: &Path, write_fn: F) -> io::Result<()>
where
    F: FnOnce(&mut BufWriter<File>) -> io::Result<()>,
{
    let tmp_path = path.with_extension("tmp");
    let file = File::create(&tmp_path)?;
    let mut writer = BufWriter::new(file);
    write_fn(&mut writer)?;
    writer.flush()?;
    writer.get_ref().sync_all()?;
    fs::rename(tmp_path, path)?;
    Ok(())
}

/// Emit debug output when `NIT_SNAPSHOT_DEBUG` is set.
fn snapshot_debug<F>(msg: F)
where
    F: FnOnce() -> String,
{
    if std::env::var_os("NIT_SNAPSHOT_DEBUG").is_none() {
        return;
    }
    eprintln!("[nit snapshot] {}", msg());
}
