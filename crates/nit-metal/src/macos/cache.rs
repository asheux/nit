//! On-disk cache for benchmarked batch execution policies.
//!
//! Stores and retrieves GPU benchmark results keyed by device name and
//! payload signature, avoiding redundant benchmarks across runs.

use crate::{BatchPolicyCacheEntryInfo, BatchPolicyCacheSnapshot};
use nit_utils::{fs::write_atomic, hashing::stable_hash_bytes, paths::cache_dir};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

pub(crate) const POLICY_CACHE_SCHEMA_VERSION: u32 = 1;

// ---------------------------------------------------------------------------
// Policy cache entry (persisted to disk as JSON)
// ---------------------------------------------------------------------------

/// On-disk representation of a benchmarked batch policy result.
///
/// Persisted as JSON with a schema version guard so that future format
/// changes can be detected and stale entries discarded automatically.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct PolicyCacheEntry {
    pub(crate) schema_version: u32,
    pub(crate) device_name: String,
    pub(crate) payload_signature: String,
    pub(crate) matches_per_batch_cap: usize,
    pub(crate) inflight_batches: usize,
}

impl PolicyCacheEntry {
    /// Validates that this entry matches the expected device, signature,
    /// schema version, and contains non-zero policy values.
    fn is_valid_for(&self, device_name: &str, sig: &str) -> bool {
        self.schema_version == POLICY_CACHE_SCHEMA_VERSION
            && self.device_name == device_name
            && self.payload_signature == sig
            && self.matches_per_batch_cap > 0
            && self.inflight_batches > 0
    }
}

// ---------------------------------------------------------------------------
// Cache path helpers
// ---------------------------------------------------------------------------

/// Replaces non-alphanumeric characters with underscores for filesystem safety.
fn sanitize_cache_component(raw: &str) -> String {
    let mut cleaned = String::with_capacity(raw.len());
    let mut after_separator = false;

    for ch in raw.chars() {
        if ch.is_ascii_alphanumeric() {
            cleaned.push(ch.to_ascii_lowercase());
            after_separator = false;
            continue;
        }
        if !after_separator {
            cleaned.push('_');
            after_separator = true;
        }
    }

    cleaned.trim_matches('_').to_string()
}

/// Root directory for Metal policy cache files.
pub(super) fn policy_cache_root() -> Option<PathBuf> {
    cache_dir().map(|base| base.join("games").join("metal-policy"))
}

/// Deterministic cache key combining device name and payload signature.
pub(super) fn policy_cache_key(device_name: &str, sig: &str) -> String {
    let device_slug = sanitize_cache_component(device_name);
    let content_hash = stable_hash_bytes(format!("{device_name}:{sig}").as_bytes());
    format!("{device_slug}_{content_hash}")
}

/// Full filesystem path for a cache entry.
pub(super) fn policy_cache_path(root: &Path, device_name: &str, sig: &str) -> PathBuf {
    root.join(format!(
        "{}_v{}.json",
        policy_cache_key(device_name, sig),
        POLICY_CACHE_SCHEMA_VERSION
    ))
}

// ---------------------------------------------------------------------------
// Cache CRUD operations
// ---------------------------------------------------------------------------

/// Loads a cached policy entry if it passes validation checks.
///
/// Returns `None` on I/O errors, parse failures, schema mismatches,
/// or when the stored device/signature do not match the request.
pub(crate) fn load_cached_policy_from_dir(
    root: &Path,
    device_name: &str,
    sig: &str,
) -> Option<PolicyCacheEntry> {
    let raw_json = fs::read(policy_cache_path(root, device_name, sig)).ok()?;
    let entry: PolicyCacheEntry = serde_json::from_slice(&raw_json).ok()?;
    entry.is_valid_for(device_name, sig).then_some(entry)
}

/// Loads a cached policy using the default cache root.
pub(super) fn load_cached_policy(device_name: &str, sig: &str) -> Option<PolicyCacheEntry> {
    let root = policy_cache_root()?;
    load_cached_policy_from_dir(&root, device_name, sig)
}

/// Atomically writes a policy entry to disk.
pub(crate) fn persist_cached_policy_from_dir(root: &Path, entry: &PolicyCacheEntry) {
    if fs::create_dir_all(root).is_err() {
        return;
    }
    let target = policy_cache_path(root, &entry.device_name, &entry.payload_signature);
    let _ = write_atomic(&target, |writer| {
        serde_json::to_writer(writer, entry).map_err(std::io::Error::other)
    });
}

/// Persists a policy entry using the default cache root.
pub(super) fn persist_cached_policy(entry: &PolicyCacheEntry) {
    let Some(root) = policy_cache_root() else {
        return;
    };
    persist_cached_policy_from_dir(&root, entry);
}

/// Tries to parse a single directory entry into a cache info record.
///
/// Returns `None` for non-files, unreadable entries, parse errors,
/// or schema version mismatches — callers simply skip these.
fn try_parse_cache_file(dir_entry: &fs::DirEntry) -> Option<BatchPolicyCacheEntryInfo> {
    if !dir_entry.file_type().ok()?.is_file() {
        return None;
    }
    let path = dir_entry.path();
    let raw = fs::read(&path).ok()?;
    let parsed: PolicyCacheEntry = serde_json::from_slice(&raw).ok()?;
    if parsed.schema_version != POLICY_CACHE_SCHEMA_VERSION {
        return None;
    }
    Some(BatchPolicyCacheEntryInfo {
        key: policy_cache_key(&parsed.device_name, &parsed.payload_signature),
        path: path.to_string_lossy().into_owned(),
        device_name: parsed.device_name,
        payload_signature: parsed.payload_signature,
        matches_per_batch: parsed.matches_per_batch_cap,
        inflight_batches: parsed.inflight_batches,
    })
}

/// Reads all valid cache entries from a directory into a snapshot.
///
/// Entries with wrong schema versions or unparseable JSON are silently
/// skipped. The returned list is sorted by key for deterministic output.
pub(crate) fn snapshot_policy_cache_from_dir(
    root: &Path,
) -> Result<BatchPolicyCacheSnapshot, String> {
    let dir_listing = match fs::read_dir(root) {
        Ok(listing) => listing,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return Ok(BatchPolicyCacheSnapshot {
                root: Some(root.to_string_lossy().into_owned()),
                entries: Vec::new(),
            });
        }
        Err(err) => {
            return Err(format!(
                "failed to read Metal policy cache {}: {err}",
                root.display()
            ));
        }
    };

    let mut entries = Vec::new();
    for result in dir_listing {
        let dir_entry = result.map_err(|err| {
            format!(
                "failed to enumerate Metal policy cache {}: {err}",
                root.display()
            )
        })?;
        if let Some(info) = try_parse_cache_file(&dir_entry) {
            entries.push(info);
        }
    }

    entries.sort_by(|a, b| {
        a.key
            .cmp(&b.key)
            .then_with(|| a.payload_signature.cmp(&b.payload_signature))
            .then_with(|| a.path.cmp(&b.path))
    });

    Ok(BatchPolicyCacheSnapshot {
        root: Some(root.to_string_lossy().into_owned()),
        entries,
    })
}

/// Deletes a single cache entry, validating it lives under the root.
pub(crate) fn clear_policy_cache_entry_in_root(root: &Path, target: &Path) -> Result<bool, String> {
    if !target.starts_with(root) {
        return Err(format!(
            "refusing to delete Metal cache entry outside {}",
            root.display()
        ));
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

/// Removes all cached policy entries under a root directory.
pub(crate) fn clear_policy_cache_in_root(root: &Path) -> Result<usize, String> {
    let current = snapshot_policy_cache_from_dir(root)?;
    let mut removed_count = 0usize;
    for entry in current.entries {
        if clear_policy_cache_entry_in_root(root, Path::new(&entry.path))? {
            removed_count += 1;
        }
    }
    Ok(removed_count)
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Returns a snapshot of all cached batch policies.
pub fn batch_policy_cache_snapshot() -> Result<BatchPolicyCacheSnapshot, String> {
    let Some(root) = policy_cache_root() else {
        return Ok(BatchPolicyCacheSnapshot::default());
    };
    snapshot_policy_cache_from_dir(&root)
}

/// Deletes a single cached policy entry by path.
pub fn clear_batch_policy_cache_entry(entry_path: &str) -> Result<bool, String> {
    let Some(root) = policy_cache_root() else {
        return Ok(false);
    };
    clear_policy_cache_entry_in_root(&root, Path::new(entry_path))
}

/// Clears all cached policy entries.
pub fn clear_batch_policy_cache() -> Result<usize, String> {
    let Some(root) = policy_cache_root() else {
        return Ok(0);
    };
    clear_policy_cache_in_root(&root)
}
