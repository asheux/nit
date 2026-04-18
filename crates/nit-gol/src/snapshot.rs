//! Snapshot persistence: writing grid state to disk as RLE + JSON.
//!
//! Handles atomic file writes, metadata serialization, timestamp
//! generation, and snapshot pruning. RLE encoding logic lives in
//! the [`rle`](crate::rle) module.

use std::fs::{self, File};
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

use crate::{attractor::AttractorEvent, Grid, Rule};

pub use crate::rle::{encode_rle, write_rle, write_rle_bits};

/// Metadata written alongside each snapshot as a JSON sidecar file.
#[derive(Clone, Debug, Default, serde::Serialize)]
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
    ensure_dir(dir)?;
    let paths = SnapshotPaths {
        rle_path: dir.join(format!("{name_base}.rle")),
        json_path: dir.join(format!("{name_base}.json")),
    };
    snapshot_debug(|| {
        let (cols, rows) = (grid.width(), grid.height());
        format!(
            "start name={name_base} dir={} geometry={cols}x{rows} rule={rule}",
            dir.display()
        )
    });
    write_atomic(&paths.rle_path, |sink| write_rle(sink, grid, rule))?;
    write_metadata_atomic(&paths.json_path, meta)?;
    snapshot_debug(|| {
        format!(
            "done rle={} json={}",
            paths.rle_path.display(),
            paths.json_path.display()
        )
    });
    Ok(paths)
}

/// Build a default snapshot file-name stem (timestamp + rule + generation + hash).
pub fn default_name(rule: Rule, generation: u64, hash: u64) -> String {
    format_name_stem(None, &now_iso8601(), &rule.to_string(), generation, hash)
}

/// Shared filename-stem formatter for snapshot writes.
///
/// Colons in the timestamp are replaced with `-`, and rule slashes are
/// stripped so the stem is safe on every common filesystem. The byte
/// layout is part of the on-disk contract — older snapshots must still
/// parse after any change here. The hash is formatted `:08x` so small
/// values pad to 8 chars, while larger u64s naturally widen.
pub(crate) fn format_name_stem(
    prefix: Option<&str>,
    timestamp: &str,
    rule: &str,
    generation: u64,
    hash: u64,
) -> String {
    let timestamp = timestamp.replace(':', "-");
    let rule_tag = rule.replace('/', "");
    let tail = format!("{timestamp}__rule-{rule_tag}__gen-{generation:05}__hash-{hash:08x}");
    match prefix {
        Some(prefix) => format!("{prefix}__{tail}"),
        None => tail,
    }
}

/// Current UTC time as an ISO 8601 / RFC 3339 string.
pub fn now_iso8601() -> String {
    OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .unwrap_or_else(|_| "unknown-time".into())
}

/// Delete the oldest `.rle` files (and their `.json` sidecars) in `dir`
/// until at most `max_files` remain.
pub fn prune_oldest(dir: &Path, max_files: usize) -> io::Result<()> {
    if max_files == 0 {
        return Ok(());
    }
    let mut rle_files = rle_entries_by_mtime(dir)?;
    if rle_files.len() <= max_files {
        return Ok(());
    }
    rle_files.sort_by_key(|(mtime, _)| *mtime);
    let excess = rle_files.len().saturating_sub(max_files);
    for (_, stale) in rle_files.into_iter().take(excess) {
        let _ = fs::remove_file(&stale);
        let _ = fs::remove_file(stale.with_extension("json"));
    }
    Ok(())
}

fn rle_entries_by_mtime(dir: &Path) -> io::Result<Vec<(SystemTime, PathBuf)>> {
    let listing = fs::read_dir(dir)?
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let candidate = entry.path();
            if candidate.extension().and_then(|ext| ext.to_str()) != Some("rle") {
                return None;
            }
            let mtime = entry.metadata().ok()?.modified().ok()?;
            Some((mtime, candidate))
        })
        .collect();
    Ok(listing)
}

/// Atomically write an RLE file from a packed grid bitset.
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
pub(crate) fn write_metadata_atomic(path: &Path, meta: &SnapshotMetadata) -> io::Result<()> {
    write_atomic(path, |writer| {
        serde_json::to_writer(writer, meta).map_err(io::Error::other)
    })
}

/// Ensure `dir` exists; reject symlinks so a hostile or stale link
/// cannot redirect snapshot writes outside the intended directory.
pub(crate) fn ensure_dir(dir: &Path) -> io::Result<()> {
    let Ok(existing) = fs::symlink_metadata(dir) else {
        return fs::create_dir_all(dir);
    };
    if existing.file_type().is_symlink() {
        return Err(io::Error::other("snapshot dir is a symlink"));
    }
    if existing.is_dir() {
        return Ok(());
    }
    fs::create_dir_all(dir)
}

/// Write to a `.tmp` sibling then rename into place.
///
/// The `flush` + `sync_all` + `rename` sequence is the durability
/// contract — readers see either the prior file or the new one,
/// never a partially written state. Concurrent writers targeting
/// the same path race on the temp file; callers must serialize.
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

fn snapshot_debug<F>(msg: F)
where
    F: FnOnce() -> String,
{
    if std::env::var_os("NIT_SNAPSHOT_DEBUG").is_none() {
        return;
    }
    eprintln!("[nit snapshot] {}", msg());
}
