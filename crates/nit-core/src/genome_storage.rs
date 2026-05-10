//! On-disk genome report cache under `<workspace>/.nit/genome/v1/`.
//!
//! The strategy is "file-per-report-hardened": one JSON per source file under
//! a 256-way sharded directory (`v1/<shard>/<encoded>-<8hex>.json`) written
//! atomically (tmp + rename). The submodule split mirrors the three concerns:
//! [`schema`] owns layout constants and the path encoder, [`cache`] is the
//! read/write hot path, [`migrations`] handles legacy-layout cleanup and
//! age/byte-ceiling enforcement. [`errors`] holds the internal `CacheError`
//! the persist path uses; the public functions remain best-effort.

mod cache;
mod errors;
mod migrations;
mod schema;

pub use cache::{delete_genome_report, load_genome_reports, persist_genome_report};
pub use migrations::gc_genome_cache;

#[cfg(test)]
#[path = "tests/genome_storage.rs"]
mod tests;
