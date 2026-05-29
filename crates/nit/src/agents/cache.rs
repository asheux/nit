//! On-disk cache of the backend model probes (`claude models --json`,
//! `gemini --models`). The probes are slow — each one spawns a subprocess
//! that boots a Node CLI and may hit the network for auth/API. Without
//! caching, every `nit` launch repeats them serially.
//!
//! The cache lives at `<cache_dir>/agents_cache.json` (XDG cache on Linux,
//! `~/Library/Caches/dev.arcxlab.nit/` on macOS). For each backend it
//! stores the model list, the resolved binary path, AND the binary's
//! mtime. Path + mtime together are the invalidation key — an upgrade
//! that replaces the binary at the same path (the common case for
//! `npm i -g @anthropic-ai/claude-code` or `brew upgrade claude`)
//! changes the mtime, which trips a cache miss and forces a re-probe.
//! This is what closes the operator-reported gap where an in-place
//! `claude` upgrade left nit serving stale models for up to 24h.
//!
//! TTL is 24h as a safety net for server-side model changes that don't
//! re-stamp the binary.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

const CACHE_FILENAME: &str = "agents_cache.json";
const CACHE_TTL_SECS: u64 = 60 * 60 * 24;
/// Bumped to 2 when binary mtime tracking was added. Old v1 caches don't
/// carry mtime fields and the safest fallback is to treat them as a miss
/// — a single one-time re-probe per host after the upgrade lands.
pub(super) const SCHEMA_VERSION: u32 = 2;

#[derive(Serialize, Deserialize, Debug, Clone)]
pub(super) struct ProbeCache {
    pub schema_version: u32,
    pub probed_at_unix: u64,
    pub claude_binary_path: Option<String>,
    /// mtime (unix seconds) of the resolved Claude binary at probe time.
    /// `None` only if the stat failed (permissions, race with deletion,
    /// or a filesystem that doesn't surface mtime). Compared against
    /// the current binary's mtime on cache load — a mismatch invalidates
    /// the cache so an in-place upgrade gets picked up immediately.
    #[serde(default)]
    pub claude_binary_mtime_unix: Option<u64>,
    pub claude_models: Vec<String>,
    pub claude_models_error: Option<String>,
    pub gemini_binary_path: Option<String>,
    #[serde(default)]
    pub gemini_binary_mtime_unix: Option<u64>,
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
            claude_binary_mtime_unix: claude_binary_path.and_then(binary_mtime_unix),
            claude_models,
            claude_models_error,
            gemini_binary_path: gemini_binary_path.map(path_to_string),
            gemini_binary_mtime_unix: gemini_binary_path.and_then(binary_mtime_unix),
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

    /// `true` when the cached binary paths AND mtimes still match the
    /// current resolution for `claude` / `gemini` on the current PATH.
    /// Either a path change (upgrade moved the binary) or an mtime change
    /// (upgrade replaced the binary in place) invalidates the cache, so
    /// the next launch re-probes and picks up whatever the new binary
    /// reports.
    ///
    /// mtime semantics: `metadata()` follows symlinks, so binaries
    /// reached via `~/.npm-global/bin/claude → claude.js` correctly
    /// reflect the target's mtime. An unreadable mtime (permission
    /// denied, filesystem without mtime support) returns `None` from
    /// `binary_mtime_unix`; both sides being `None` is treated as a
    /// match (best-effort; the path check still applies), but a
    /// `Some` vs `None` mismatch invalidates so we err on the side of
    /// re-probing.
    pub(super) fn binaries_match(
        &self,
        claude_path: Option<&Path>,
        gemini_path: Option<&Path>,
    ) -> bool {
        self.claude_binary_path == claude_path.map(path_to_string)
            && self.gemini_binary_path == gemini_path.map(path_to_string)
            && self.claude_binary_mtime_unix == claude_path.and_then(binary_mtime_unix)
            && self.gemini_binary_mtime_unix == gemini_path.and_then(binary_mtime_unix)
    }
}

fn path_to_string(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

/// Resolve the binary's modification time as unix seconds. `metadata()`
/// follows symlinks deliberately — `which claude` typically points to a
/// shim/symlink whose target is the file an in-place upgrade rewrites,
/// and we want to see the target's mtime. Returns `None` when the stat
/// fails or the filesystem doesn't surface mtime (rare; pre-EXT4 / FAT).
fn binary_mtime_unix(path: &Path) -> Option<u64> {
    let metadata = std::fs::metadata(path).ok()?;
    let modified = metadata.modified().ok()?;
    modified
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|d| d.as_secs())
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
            claude_binary_mtime_unix: Some(42),
            claude_models: vec!["claude-opus-4-7".into()],
            claude_models_error: None,
            gemini_binary_path: Some("/opt/homebrew/bin/gemini".into()),
            gemini_binary_mtime_unix: Some(99),
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

    // Helper: stamp a real file in the system temp dir and snapshot its
    // mtime. Real files needed because `binaries_match` stats them via
    // `std::fs::metadata`, which can't be faked without a stat shim.
    fn tmp_binary(suffix: &str) -> (PathBuf, u64) {
        use std::io::Write;
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let path = std::env::temp_dir().join(format!(
            "nit-cache-test-{}-{nanos}-{suffix}",
            std::process::id()
        ));
        let mut f = std::fs::File::create(&path).expect("create temp binary");
        f.write_all(b"#!/bin/sh\n").expect("write temp binary");
        let mtime = binary_mtime_unix(&path).expect("mtime readable");
        (path, mtime)
    }

    #[test]
    fn binaries_match_path_and_mtime() {
        let (claude_path, claude_mtime) = tmp_binary("claude");
        let (gemini_path, gemini_mtime) = tmp_binary("gemini");
        let mut cache = sample_cache(0);
        cache.claude_binary_path = Some(claude_path.to_string_lossy().into());
        cache.claude_binary_mtime_unix = Some(claude_mtime);
        cache.gemini_binary_path = Some(gemini_path.to_string_lossy().into());
        cache.gemini_binary_mtime_unix = Some(gemini_mtime);

        assert!(
            cache.binaries_match(Some(&claude_path), Some(&gemini_path)),
            "same path + same mtime should match"
        );
        let _ = std::fs::remove_file(&claude_path);
        let _ = std::fs::remove_file(&gemini_path);
    }

    #[test]
    fn binaries_mismatch_on_path_change() {
        let (claude_path, claude_mtime) = tmp_binary("claude-orig");
        let mut cache = sample_cache(0);
        cache.claude_binary_path = Some(claude_path.to_string_lossy().into());
        cache.claude_binary_mtime_unix = Some(claude_mtime);
        // gemini absent
        cache.gemini_binary_path = None;
        cache.gemini_binary_mtime_unix = None;

        let other = PathBuf::from("/usr/local/bin/claude-different");
        assert!(
            !cache.binaries_match(Some(&other), None),
            "different path should mismatch even when gemini is None on both sides"
        );
        let _ = std::fs::remove_file(&claude_path);
    }

    #[test]
    fn binaries_mismatch_on_in_place_upgrade_via_mtime() {
        // Operator-reported failure mode: `npm i -g @anthropic-ai/claude-code`
        // replaces the binary in place, so the path is unchanged. The
        // cached mtime is from before the upgrade; the current binary's
        // mtime is after. The mismatch is what forces a re-probe so the
        // updated model list / new version becomes visible without a
        // manual cache nuke.
        let (claude_path, original_mtime) = tmp_binary("claude-upgrade");
        let mut cache = sample_cache(0);
        cache.claude_binary_path = Some(claude_path.to_string_lossy().into());
        cache.claude_binary_mtime_unix = Some(original_mtime);
        cache.gemini_binary_path = None;
        cache.gemini_binary_mtime_unix = None;

        // Simulate an in-place upgrade by overwriting + advancing mtime.
        // Most filesystems only have whole-second mtime resolution, so
        // sleeping ≥1s guarantees the new mtime is distinct.
        std::thread::sleep(std::time::Duration::from_millis(1100));
        std::fs::write(&claude_path, b"#!/bin/sh\n# upgraded\n").expect("rewrite binary");
        let new_mtime = binary_mtime_unix(&claude_path).expect("mtime after upgrade");
        assert!(
            new_mtime > original_mtime,
            "fs must surface a newer mtime after the in-place rewrite ({new_mtime} > {original_mtime})"
        );

        assert!(
            !cache.binaries_match(Some(&claude_path), None),
            "in-place upgrade (same path, newer mtime) must invalidate the cache; \
             before-fix this stayed a hit and the operator saw stale models for up to 24h"
        );
        let _ = std::fs::remove_file(&claude_path);
    }

    #[test]
    fn binaries_match_when_both_sides_lack_mtime() {
        // Edge case: cache and current both have `None` for a backend's
        // mtime (binary not present, or stat failed). Treat as a match
        // for that backend — the path comparison still applies. This
        // matters for hosts that don't have Gemini installed; we don't
        // want the absence to falsely invalidate Claude's cache.
        let cache = ProbeCache {
            schema_version: SCHEMA_VERSION,
            probed_at_unix: 0,
            claude_binary_path: None,
            claude_binary_mtime_unix: None,
            claude_models: Vec::new(),
            claude_models_error: None,
            gemini_binary_path: None,
            gemini_binary_mtime_unix: None,
            gemini_models: Vec::new(),
            gemini_models_error: None,
        };
        assert!(cache.binaries_match(None, None));
    }
}
