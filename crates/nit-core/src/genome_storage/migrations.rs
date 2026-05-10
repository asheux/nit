//! Garbage collection + legacy-layout cleanup for the v1 genome cache.
//!
//! Three phases run in order from `gc_genome_cache`:
//!   1. Sweep `<workspace>/.nit/genome/` for non-v1 detritus — pre-v1 flat
//!      `*.json` siblings at the genome root, plus any future `v0/` / `v2/`
//!      subtrees a different schema version may have left behind.
//!   2. Drop reports whose mtime is older than `MAX_CACHE_AGE_SECS` so the
//!      cache forgets stale evaluations of files that haven't changed.
//!   3. If the surviving total still exceeds `MAX_CACHE_BYTES`, evict by
//!      mtime-ascending until the cache fits the ceiling.
//!
//! The walk is O(reports). At 50K reports the whole sweep takes ≲ 100 ms; the
//! design is not intended to scale to 500K+ files without a sidecar index.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use super::schema::{
    genome_dir, GENOME_DIR_NAME, MAX_CACHE_AGE_SECS, MAX_CACHE_BYTES, NIT_DIR_NAME,
    REPORT_EXTENSION, SCHEMA_VERSION,
};

pub fn gc_genome_cache(workspace_root: &Path) {
    sweep_legacy_layout(workspace_root);
    let dir = genome_dir(workspace_root);
    let mut alive = collect_reports_with_meta(&dir);
    drop_expired(&mut alive);
    enforce_byte_ceiling(&mut alive);
}

fn sweep_legacy_layout(workspace_root: &Path) {
    let root = workspace_root.join(NIT_DIR_NAME).join(GENOME_DIR_NAME);
    let Ok(entries) = fs::read_dir(&root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        let kind = path.file_name().and_then(|s| s.to_str());
        if file_type.is_dir() && kind != Some(SCHEMA_VERSION) {
            let _ = fs::remove_dir_all(&path);
        } else if file_type.is_file()
            && path.extension().and_then(|e| e.to_str()) == Some(REPORT_EXTENSION)
        {
            let _ = fs::remove_file(&path);
        }
    }
}

struct ReportEntry {
    path: PathBuf,
    mtime_secs: u64,
    size: u64,
}

fn collect_reports_with_meta(version_root: &Path) -> Vec<ReportEntry> {
    let mut out = Vec::new();
    let Ok(shards) = fs::read_dir(version_root) else {
        return out;
    };
    for shard in shards.flatten() {
        if !shard.file_type().is_ok_and(|ft| ft.is_dir()) {
            continue;
        }
        let Ok(entries) = fs::read_dir(shard.path()) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some(REPORT_EXTENSION) {
                continue;
            }
            let Ok(meta) = entry.metadata() else {
                continue;
            };
            let mtime_secs = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map(|d| d.as_secs())
                .unwrap_or(0);
            out.push(ReportEntry {
                path,
                mtime_secs,
                size: meta.len(),
            });
        }
    }
    out
}

fn drop_expired(alive: &mut Vec<ReportEntry>) {
    let cutoff = match SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|now| now.as_secs().checked_sub(MAX_CACHE_AGE_SECS))
    {
        Some(c) => c,
        None => return,
    };
    alive.retain(|entry| {
        if entry.mtime_secs < cutoff {
            let _ = fs::remove_file(&entry.path);
            false
        } else {
            true
        }
    });
}

fn enforce_byte_ceiling(alive: &mut Vec<ReportEntry>) {
    let mut total: u64 = alive.iter().map(|e| e.size).sum();
    if total <= MAX_CACHE_BYTES {
        return;
    }
    alive.sort_by_key(|entry| entry.mtime_secs);
    for entry in alive.drain(..) {
        if total <= MAX_CACHE_BYTES {
            break;
        }
        if fs::remove_file(&entry.path).is_ok() {
            total = total.saturating_sub(entry.size);
        }
    }
}
