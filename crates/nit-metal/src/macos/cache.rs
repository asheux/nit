//! On-disk cache for benchmarked batch execution policies.

use crate::{BatchPolicyCacheEntryInfo, BatchPolicyCacheSnapshot};
use nit_utils::{fs::write_atomic, hashing::stable_hash_bytes, paths::cache_dir};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

pub(crate) const POLICY_CACHE_SCHEMA_VERSION: u32 = 1;

type PolicyResult<T> = Result<T, String>;

/// On-disk representation of a benchmarked batch policy result.
///
/// Schema-versioned so stale entries are discarded on format changes.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct PolicyCacheEntry {
    pub(crate) schema_version: u32,
    pub(crate) device_name: String,
    pub(crate) payload_signature: String,
    pub(crate) matches_per_batch_cap: usize,
    pub(crate) inflight_batches: usize,
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
        let derived_key = policy_cache_key(&device_name, &payload_signature);
        BatchPolicyCacheEntryInfo {
            key: derived_key,
            path: location.to_string_lossy().into_owned(),
            device_name,
            payload_signature,
            matches_per_batch: matches_per_batch_cap,
            inflight_batches,
        }
    }
}

/// Lowercases alphanumeric chars, replaces non-alnum runs with a single underscore.
fn sanitize_cache_component(raw_name: &str) -> String {
    raw_name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect::<String>()
        .split('_')
        .filter(|segment| !segment.is_empty())
        .collect::<Vec<_>>()
        .join("_")
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

pub(crate) fn load_cached_policy_from_dir(
    root: &Path,
    device_name: &str,
    sig: &str,
) -> Option<PolicyCacheEntry> {
    let json_bytes = fs::read(policy_cache_path(root, device_name, sig)).ok()?;
    let deserialized: PolicyCacheEntry = serde_json::from_slice(&json_bytes).ok()?;
    deserialized
        .is_valid_for(device_name, sig)
        .then_some(deserialized)
}

pub(super) fn load_cached_policy(device_name: &str, sig: &str) -> Option<PolicyCacheEntry> {
    let root = policy_cache_root()?;
    load_cached_policy_from_dir(&root, device_name, sig)
}

pub(crate) fn persist_cached_policy_from_dir(cache_root: &Path, record: &PolicyCacheEntry) {
    if fs::create_dir_all(cache_root).is_err() {
        return;
    }
    let destination = policy_cache_path(cache_root, &record.device_name, &record.payload_signature);
    let _ = write_atomic(&destination, |writer| {
        serde_json::to_writer(writer, record).map_err(std::io::Error::other)
    });
}

pub(super) fn persist_cached_policy(entry: &PolicyCacheEntry) {
    let Some(root) = policy_cache_root() else {
        return;
    };
    persist_cached_policy_from_dir(&root, entry);
}

fn parse_dir_entry_as_policy(fs_entry: &fs::DirEntry) -> Option<BatchPolicyCacheEntryInfo> {
    let metadata = fs_entry.file_type().ok()?;
    if !metadata.is_file() {
        return None;
    }
    let file_location = fs_entry.path();
    let json_bytes = fs::read(&file_location).ok()?;
    let deserialized: PolicyCacheEntry = serde_json::from_slice(&json_bytes).ok()?;
    if deserialized.schema_version != POLICY_CACHE_SCHEMA_VERSION {
        return None;
    }
    Some(deserialized.into_cache_info(file_location))
}

/// Sorted by key for deterministic output; invalid entries silently skipped.
pub(crate) fn snapshot_policy_cache_from_dir(
    target_dir: &Path,
) -> PolicyResult<BatchPolicyCacheSnapshot> {
    let root_label = target_dir.to_string_lossy().into_owned();
    let make_error = |verb: &str, io_err: std::io::Error| -> String {
        format!(
            "failed to {verb} Metal policy cache {}: {io_err}",
            target_dir.display()
        )
    };

    let directory_listing = match fs::read_dir(target_dir) {
        Ok(listing) => listing,
        Err(io_err) if io_err.kind() == std::io::ErrorKind::NotFound => {
            return Ok(BatchPolicyCacheSnapshot {
                root: Some(root_label),
                entries: Vec::new(),
            });
        }
        Err(io_err) => return Err(make_error("read", io_err)),
    };

    let enumerated_files: Vec<fs::DirEntry> = directory_listing
        .collect::<Result<Vec<_>, _>>()
        .map_err(|io_err| make_error("enumerate", io_err))?;

    let mut discovered_policies: Vec<BatchPolicyCacheEntryInfo> = enumerated_files
        .iter()
        .filter_map(parse_dir_entry_as_policy)
        .collect();

    discovered_policies.sort_by(|first, second| {
        first
            .key
            .cmp(&second.key)
            .then_with(|| first.payload_signature.cmp(&second.payload_signature))
            .then_with(|| first.path.cmp(&second.path))
    });

    Ok(BatchPolicyCacheSnapshot {
        root: Some(root_label),
        entries: discovered_policies,
    })
}

/// Validates that `target` lives under `cache_root` before deleting.
pub(crate) fn clear_policy_cache_entry_in_root(
    cache_root: &Path,
    target: &Path,
) -> PolicyResult<bool> {
    if !target.starts_with(cache_root) {
        return Err(format!(
            "refusing to delete Metal cache entry outside {}",
            cache_root.display()
        ));
    }
    match fs::remove_file(target) {
        Ok(()) => Ok(true),
        Err(io_err) if io_err.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(io_err) => Err(format!(
            "failed to delete Metal cache entry {}: {io_err}",
            target.display()
        )),
    }
}

pub(crate) fn clear_policy_cache_in_root(cache_root: &Path) -> PolicyResult<usize> {
    let existing = snapshot_policy_cache_from_dir(cache_root)?;
    existing
        .entries
        .iter()
        .try_fold(0usize, |deleted, cached_entry| {
            let was_removed =
                clear_policy_cache_entry_in_root(cache_root, Path::new(&cached_entry.path))?;
            Ok(deleted + was_removed as usize)
        })
}

pub fn batch_policy_cache_snapshot() -> PolicyResult<BatchPolicyCacheSnapshot> {
    let Some(root) = policy_cache_root() else {
        return Ok(BatchPolicyCacheSnapshot::default());
    };
    snapshot_policy_cache_from_dir(&root)
}

pub fn clear_batch_policy_cache_entry(entry_path: &str) -> PolicyResult<bool> {
    let Some(root) = policy_cache_root() else {
        return Ok(false);
    };
    clear_policy_cache_entry_in_root(&root, Path::new(entry_path))
}

pub fn clear_batch_policy_cache() -> PolicyResult<usize> {
    let Some(root) = policy_cache_root() else {
        return Ok(0);
    };
    clear_policy_cache_in_root(&root)
}
