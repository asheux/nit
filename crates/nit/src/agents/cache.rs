//! On-disk cache of the backend model probes (`claude models --json`,
//! `gemini --models`). The probes are slow — each one spawns a subprocess
//! that boots a Node CLI and may hit the network for auth/API. Without
//! caching, every `nit` launch repeats them serially.
//!
//! The cache lives at `<cache_dir>/agents_cache.json` (XDG cache on Linux,
//! `~/Library/Caches/dev.arcxlab.nit/` on macOS). It stores the model list
//! plus the resolved binary path for each backend; the path doubles as a
//! cheap invalidation key (an upgrade that changes `which claude` ⇒ cache
//! miss).
//!
//! TTL is 24h to recover from server-side model changes without forcing
//! a manual refresh.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

const CACHE_FILENAME: &str = "agents_cache.json";
const CACHE_TTL_SECS: u64 = 60 * 60 * 24;
pub(super) const SCHEMA_VERSION: u32 = 1;

#[derive(Serialize, Deserialize, Debug, Clone)]
pub(super) struct ProbeCache {
    pub schema_version: u32,
    pub probed_at_unix: u64,
    pub claude_binary_path: Option<String>,
    pub claude_models: Vec<String>,
    pub claude_models_error: Option<String>,
    pub gemini_binary_path: Option<String>,
    pub gemini_models: Vec<String>,
    pub gemini_models_error: Option<String>,
}

impl ProbeCache {
    pub(super) fn new(
        claude_binary_path: Option<&Path>,
        claude_models: Vec<String>,
        claude_models_error: Option<String>,
        gemini_binary_path: Option<&Path>,
        gemini_models: Vec<String>,
        gemini_models_error: Option<String>,
    ) -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            probed_at_unix: now_unix(),
            claude_binary_path: claude_binary_path.map(path_to_string),
            claude_models,
            claude_models_error,
            gemini_binary_path: gemini_binary_path.map(path_to_string),
            gemini_models,
            gemini_models_error,
        }
    }

    /// `true` when the cache was written within the TTL window and the
    /// schema version matches the current binary. Stale or schema-mismatched
    /// caches are treated as a miss.
    pub(super) fn is_fresh(&self, now_unix: u64) -> bool {
        if self.schema_version != SCHEMA_VERSION {
            return false;
        }
        now_unix.saturating_sub(self.probed_at_unix) < CACHE_TTL_SECS
    }

    /// `true` when the cached binary paths still match the resolved paths
    /// for `claude` / `gemini` on the current PATH. An upgrade that moved
    /// the binary (or removed one) invalidates the cache.
    pub(super) fn binaries_match(
        &self,
        claude_path: Option<&Path>,
        gemini_path: Option<&Path>,
    ) -> bool {
        self.claude_binary_path == claude_path.map(path_to_string)
            && self.gemini_binary_path == gemini_path.map(path_to_string)
    }
}

fn path_to_string(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

pub(super) fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn cache_file_path() -> Option<PathBuf> {
    nit_utils::paths::cache_dir().map(|dir| dir.join(CACHE_FILENAME))
}

/// Best-effort cache load. Returns `None` if the cache directory is
/// inaccessible, the file is missing, the JSON is malformed, or the schema
/// version is wrong. Callers must treat absence as "cache miss" and probe.
pub(super) fn load() -> Option<ProbeCache> {
    let path = cache_file_path()?;
    let text = std::fs::read_to_string(&path).ok()?;
    let cache: ProbeCache = serde_json::from_str(&text).ok()?;
    if cache.schema_version != SCHEMA_VERSION {
        return None;
    }
    Some(cache)
}

/// Best-effort cache save via atomic write. Failures (no cache dir, I/O
/// error, etc.) are logged and swallowed — the cache is a re-derivable
/// optimization, not a source of truth.
pub(super) fn save(cache: &ProbeCache) {
    if let Err(err) = try_save(cache) {
        tracing::debug!(
            "agents cache: failed to persist probe results: {err}; \
             next launch will re-probe"
        );
    }
}

fn try_save(cache: &ProbeCache) -> std::io::Result<()> {
    let path =
        cache_file_path().ok_or_else(|| std::io::Error::other("no cache directory available"))?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    nit_utils::fs::write_atomic(&path, |writer| {
        serde_json::to_writer_pretty(writer, cache).map_err(std::io::Error::other)
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_cache(now: u64) -> ProbeCache {
        ProbeCache {
            schema_version: SCHEMA_VERSION,
            probed_at_unix: now,
            claude_binary_path: Some("/opt/homebrew/bin/claude".into()),
            claude_models: vec!["claude-opus-4-7".into()],
            claude_models_error: None,
            gemini_binary_path: Some("/opt/homebrew/bin/gemini".into()),
            gemini_models: vec!["gemini-2.0-flash".into()],
            gemini_models_error: None,
        }
    }

    #[test]
    fn fresh_within_ttl() {
        let now = 1_000_000;
        let cache = sample_cache(now - 60);
        assert!(cache.is_fresh(now));
    }

    #[test]
    fn stale_past_ttl() {
        let now = 1_000_000;
        let cache = sample_cache(now - CACHE_TTL_SECS - 1);
        assert!(!cache.is_fresh(now));
    }

    #[test]
    fn schema_mismatch_treated_as_stale() {
        let now = 1_000_000;
        let mut cache = sample_cache(now);
        cache.schema_version = SCHEMA_VERSION + 1;
        assert!(!cache.is_fresh(now));
    }

    #[test]
    fn binary_path_match() {
        let cache = sample_cache(0);
        let claude = PathBuf::from("/opt/homebrew/bin/claude");
        let gemini = PathBuf::from("/opt/homebrew/bin/gemini");
        assert!(cache.binaries_match(Some(&claude), Some(&gemini)));

        let other = PathBuf::from("/usr/local/bin/claude");
        assert!(!cache.binaries_match(Some(&other), Some(&gemini)));
        assert!(!cache.binaries_match(None, Some(&gemini)));
    }
}
