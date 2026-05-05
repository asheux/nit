//! On-disk cache for benchmarked batch execution policies.
//!
//! Entries are schema-versioned and silently dropped when the version bumps,
//! so refactors that change [`PolicyCacheEntry`]'s serde layout must also
//! bump [`POLICY_CACHE_SCHEMA_VERSION`].

use super::MetalResult;
use crate::{BatchPolicyCacheEntryInfo, BatchPolicyCacheSnapshot};
use nit_utils::{fs::write_atomic, hashing::stable_hash_bytes, paths::cache_dir};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

pub(super) const POLICY_CACHE_SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub(super) struct PolicyCacheEntry {
    pub(super) schema_version: u32,
    pub(super) device_name: String,
    pub(super) payload_signature: String,
    pub(super) matches_per_batch_cap: usize,
    pub(super) inflight_batches: usize,
}

impl PolicyCacheEntry {
    fn is_valid_for(&self, expected_device: &str, expected_sig: &str) -> bool {
        self.schema_version == POLICY_CACHE_SCHEMA_VERSION
            && self.device_name == expected_device
            && self.payload_signature == expected_sig
            && self.matches_per_batch_cap > 0
            && self.inflight_batches > 0
    }

    fn into_cache_info(self, location: PathBuf) -> BatchPolicyCacheEntryInfo {
        let Self {
            device_name,
            payload_signature,
            matches_per_batch_cap,
            inflight_batches,
            ..
        } = self;
        let key = policy_cache_key(&device_name, &payload_signature);
        BatchPolicyCacheEntryInfo {
            key,
            path: location.to_string_lossy().into_owned(),
            device_name,
            payload_signature,
            matches_per_batch: matches_per_batch_cap,
            inflight_batches,
        }
    }
}

/// Lowercases ASCII alphanumerics and collapses every other character run into
/// a single `_`. Used to form device-name slugs that are safe as filenames.
pub(super) fn sanitize_cache_component(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut prev_was_sep = true;
    for ch in raw.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            prev_was_sep = false;
        } else if !prev_was_sep {
            out.push('_');
            prev_was_sep = true;
        }
    }
    if out.ends_with('_') {
        out.pop();
    }
    out
}

pub(super) fn policy_cache_root() -> Option<PathBuf> {
    cache_dir().map(|base| base.join("games").join("metal-policy"))
}

pub(super) fn policy_cache_key(device_name: &str, sig: &str) -> String {
    let device_slug = sanitize_cache_component(device_name);
    let content_hash = stable_hash_bytes(format!("{device_name}:{sig}").as_bytes());
    format!("{device_slug}_{content_hash}")
}

pub(super) fn policy_cache_path(root: &Path, device_name: &str, sig: &str) -> PathBuf {
    root.join(format!(
        "{}_v{}.json",
        policy_cache_key(device_name, sig),
        POLICY_CACHE_SCHEMA_VERSION
    ))
}

pub(super) fn load_cached_policy_from_dir(
    root: &Path,
    device_name: &str,
    sig: &str,
) -> Option<PolicyCacheEntry> {
    let json_bytes = fs::read(policy_cache_path(root, device_name, sig)).ok()?;
    let entry: PolicyCacheEntry = serde_json::from_slice(&json_bytes).ok()?;
    entry.is_valid_for(device_name, sig).then_some(entry)
}

pub(super) fn load_cached_policy(device_name: &str, sig: &str) -> Option<PolicyCacheEntry> {
    let root = policy_cache_root()?;
    load_cached_policy_from_dir(&root, device_name, sig)
}

pub(super) fn persist_cached_policy_from_dir(cache_root: &Path, entry: &PolicyCacheEntry) {
    if fs::create_dir_all(cache_root).is_err() {
        return;
    }
    let destination = policy_cache_path(cache_root, &entry.device_name, &entry.payload_signature);
    let _ = write_atomic(&destination, |writer| {
        serde_json::to_writer(writer, entry).map_err(std::io::Error::other)
    });
}

pub(super) fn persist_cached_policy(entry: &PolicyCacheEntry) {
    let Some(root) = policy_cache_root() else {
        return;
    };
    persist_cached_policy_from_dir(&root, entry);
}

fn parse_dir_entry_as_policy(fs_entry: &fs::DirEntry) -> Option<BatchPolicyCacheEntryInfo> {
    let file_type = fs_entry.file_type().ok()?;
    if !file_type.is_file() {
        return None;
    }
    let path = fs_entry.path();
    let json_bytes = fs::read(&path).ok()?;
    let entry: PolicyCacheEntry = serde_json::from_slice(&json_bytes).ok()?;
    if entry.schema_version != POLICY_CACHE_SCHEMA_VERSION {
        return None;
    }
    Some(entry.into_cache_info(path))
}

fn cache_io_error(verb: &str, dir: &Path, err: std::io::Error) -> String {
    format!(
        "failed to {verb} Metal policy cache {}: {err}",
        dir.display()
    )
}

/// Sorted by key for deterministic UI output; unreadable or
/// schema-mismatched entries are silently skipped rather than failing.
pub(super) fn snapshot_policy_cache_from_dir(dir: &Path) -> MetalResult<BatchPolicyCacheSnapshot> {
    let root_label = dir.to_string_lossy().into_owned();

    let listing = match fs::read_dir(dir) {
        Ok(listing) => listing,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return Ok(BatchPolicyCacheSnapshot {
                root: Some(root_label),
                entries: Vec::new(),
            });
        }
        Err(err) => return Err(cache_io_error("read", dir, err)),
    };

    let dir_entries: Vec<fs::DirEntry> = listing
        .collect::<Result<Vec<_>, _>>()
        .map_err(|err| cache_io_error("enumerate", dir, err))?;

    let mut entries: Vec<BatchPolicyCacheEntryInfo> = dir_entries
        .iter()
        .filter_map(parse_dir_entry_as_policy)
        .collect();

    entries.sort_by(|a, b| {
        a.key
            .cmp(&b.key)
            .then_with(|| a.payload_signature.cmp(&b.payload_signature))
            .then_with(|| a.path.cmp(&b.path))
    });

    Ok(BatchPolicyCacheSnapshot {
        root: Some(root_label),
        entries,
    })
}

/// Security-adjacent guard: refuses to delete paths outside `cache_root`,
/// so callers can pass untrusted `target` strings without a path traversal.
/// Symlinks are rejected because the textual `starts_with` prefix check above
/// can be defeated by a symlink under `cache_root` that resolves elsewhere.
pub(super) fn clear_policy_cache_entry_in_root(
    cache_root: &Path,
    target: &Path,
) -> MetalResult<bool> {
    if !target.starts_with(cache_root) {
        return Err(format!(
            "refusing to delete Metal cache entry outside {}",
            cache_root.display()
        ));
    }
    match fs::symlink_metadata(target) {
        Ok(meta) if meta.file_type().is_symlink() => {
            return Err(format!(
                "refusing to delete symlinked Metal cache entry {}",
                target.display()
            ));
        }
        Ok(_) => {}
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(err) => {
            return Err(format!(
                "failed to inspect Metal cache entry {}: {err}",
                target.display()
            ));
        }
    }
    match fs::remove_file(target) {
        Ok(()) => Ok(true),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(err) => Err(format!(
            "failed to delete Metal cache entry {}: {err}",
            target.display()
        )),
    }
}

pub(super) fn clear_policy_cache_in_root(cache_root: &Path) -> MetalResult<usize> {
    let snapshot = snapshot_policy_cache_from_dir(cache_root)?;
    snapshot.entries.iter().try_fold(0usize, |deleted, entry| {
        let removed = clear_policy_cache_entry_in_root(cache_root, Path::new(&entry.path))?;
        Ok(deleted + removed as usize)
    })
}

pub fn batch_policy_cache_snapshot() -> MetalResult<BatchPolicyCacheSnapshot> {
    let Some(root) = policy_cache_root() else {
        return Ok(BatchPolicyCacheSnapshot::default());
    };
    snapshot_policy_cache_from_dir(&root)
}

pub fn clear_batch_policy_cache_entry(entry_path: &str) -> MetalResult<bool> {
    let Some(root) = policy_cache_root() else {
        return Ok(false);
    };
    clear_policy_cache_entry_in_root(&root, Path::new(entry_path))
}

pub fn clear_batch_policy_cache() -> MetalResult<usize> {
    let Some(root) = policy_cache_root() else {
        return Ok(0);
    };
    clear_policy_cache_in_root(&root)
}
