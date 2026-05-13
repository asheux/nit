//! v1 cache layout: directory constants + the path encoder shared by
//! `cache` (writes) and `migrations` (gc).
//!
//! Layout: `<workspace>/.nit/genome/v1/<shard>/<encoded_path>-<8hex>.json`.
//! `shard` is the lower 8 bits of `stable_hash_bytes(path)` (256 buckets) and
//! the trailing `-<8hex>` suffix prevents the path-encoding collision between
//! e.g. `a/b/foo` and `a__b/foo` (both flatten to `a__b__foo`).

use std::path::{Path, PathBuf};

use nit_utils::hashing::stable_hash_bytes;

pub(super) const NIT_DIR_NAME: &str = ".nit";
pub(super) const GENOME_DIR_NAME: &str = "genome";
pub(super) const SCHEMA_VERSION: &str = "v1";
pub(super) const REPORT_EXTENSION: &str = "json";
pub(super) const MAX_CACHE_AGE_SECS: u64 = 60 * 60 * 24 * 30;
pub(super) const MAX_CACHE_BYTES: u64 = 256 * 1024 * 1024;

pub(super) fn genome_dir(workspace_root: &Path) -> PathBuf {
    workspace_root
        .join(NIT_DIR_NAME)
        .join(GENOME_DIR_NAME)
        .join(SCHEMA_VERSION)
}

pub(super) fn report_path(workspace_root: &Path, file_path: &Path) -> PathBuf {
    let raw = file_path.to_string_lossy();
    let hash = stable_hash_bytes(raw.as_bytes());
    let bucket = (hash & 0xff) as u8;
    let suffix = hash as u32;
    // Replace path separators AND Windows-reserved filename characters
    // (`< > : " | ? *`) with `__`, since the flattened source path is used
    // verbatim as a filename — e.g. `C:\Users\...` would otherwise produce
    // a basename containing `:` and Windows rejects the write.
    let flattened = raw.replace(['/', '\\', ':', '<', '>', '"', '|', '?', '*'], "__");
    let basename = format!("{flattened}-{suffix:08x}.{REPORT_EXTENSION}");
    genome_dir(workspace_root)
        .join(format!("{bucket:02x}"))
        .join(basename)
}
