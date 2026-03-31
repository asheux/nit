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
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct PolicyCacheEntry {
    pub(crate) schema_version: u32,
    pub(crate) device_name: String,
    pub(crate) payload_signature: String,
    pub(crate) matches_per_batch_cap: usize,
    pub(crate) inflight_batches: usize,
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
pub(crate) fn load_cached_policy_from_dir(
    root: &Path,
    device_name: &str,
    sig: &str,
) -> Option<PolicyCacheEntry> {
    let cache_file = policy_cache_path(root, device_name, sig);
    let raw_json = fs::read(cache_file).ok()?;
    let entry: PolicyCacheEntry = serde_json::from_slice(&raw_json).ok()?;

    let version_matches = entry.schema_version == POLICY_CACHE_SCHEMA_VERSION;
    let device_matches = entry.device_name == device_name;
    let signature_matches = entry.payload_signature == sig;
    let values_valid = entry.matches_per_batch_cap > 0 && entry.inflight_batches > 0;

    if version_matches && device_matches && signature_matches && values_valid {
        Some(entry)
    } else {
        None
    }
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

/// Reads all valid cache entries from a directory into a snapshot.
pub(crate) fn snapshot_policy_cache_from_dir(
    root: &Path,
) -> Result<BatchPolicyCacheSnapshot, String> {
    let mut snapshot = BatchPolicyCacheSnapshot {
        root: Some(root.to_string_lossy().into_owned()),
        entries: Vec::new(),
    };

    let dir_listing = match fs::read_dir(root) {
        Ok(listing) => listing,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(snapshot),
        Err(err) => {
            return Err(format!(
                "failed to read Metal policy cache {}: {err}",
                root.display()
            ));
        }
    };

    for dir_result in dir_listing {
        let dir_entry = dir_result.map_err(|err| {
            format!(
                "failed to enumerate Metal policy cache {}: {err}",
                root.display()
            )
        })?;

        let is_regular_file = dir_entry
            .file_type()
            .map(|ft| ft.is_file())
            .unwrap_or(false);
        if !is_regular_file {
            continue;
        }

        let entry_path = dir_entry.path();
        let Ok(raw_json) = fs::read(&entry_path) else {
            continue;
        };
        let Ok(parsed): Result<PolicyCacheEntry, _> = serde_json::from_slice(&raw_json) else {
            continue;
        };
        if parsed.schema_version != POLICY_CACHE_SCHEMA_VERSION {
            continue;
        }

        snapshot.entries.push(BatchPolicyCacheEntryInfo {
            key: policy_cache_key(&parsed.device_name, &parsed.payload_signature),
            path: entry_path.to_string_lossy().into_owned(),
            device_name: parsed.device_name,
            payload_signature: parsed.payload_signature,
            matches_per_batch: parsed.matches_per_batch_cap,
            inflight_batches: parsed.inflight_batches,
        });
    }

    snapshot.entries.sort_by(|lhs, rhs| {
        lhs.key
            .cmp(&rhs.key)
            .then(lhs.payload_signature.cmp(&rhs.payload_signature))
            .then(lhs.path.cmp(&rhs.path))
    });

    Ok(snapshot)
}

/// Deletes a single cache entry, validating it lives under the root.
pub(crate) fn clear_policy_cache_entry_in_root(
    root: &Path,
    target: &Path,
) -> Result<bool, String> {
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
