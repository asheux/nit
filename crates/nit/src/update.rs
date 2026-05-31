//! `nit update` subcommand + startup version-check banner.
//!
//! Two operator-facing flows:
//!
//!   1. `nit update` — compares `CARGO_PKG_VERSION` against
//!      `https://download.nit.tools/latest.json` and re-runs the
//!      appropriate install command (Homebrew when nit is installed
//!      under a known Homebrew prefix, the install-script one-liner
//!      otherwise, a printed PowerShell snippet on Windows).
//!   2. Startup banner — `print_update_notice_if_newer` does the same
//!      comparison but only prints a one-line stderr notice and
//!      caches the result for 24h. Called from `main.rs` before the
//!      TUI takes over the terminal.
//!
//! Both honour `NIT_NO_VERSION_CHECK=1` to silence network access in
//! sandboxed / offline environments.
//!
//! Network access is via `curl` subprocess (already a transitive
//! requirement: install.sh / install.ps1 both depend on it). Avoids
//! adding an HTTP client crate just for this. Failures are
//! best-effort: a network blip on startup is silent, and `nit update`
//! returns an `anyhow::Error` the caller surfaces.

use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

const REMOTE_URL: &str = "https://download.nit.tools/latest.json";
const CACHE_FILENAME: &str = "version_check.json";
const CACHE_TTL_SECS: u64 = 60 * 60 * 24; // 24h
/// Hard cap on the latest.json fetch. Default 1s keeps startup snappy on
/// flaky networks — a missed banner is far better than a 5s nit launch.
const FETCH_TIMEOUT_SECS: u32 = 1;

#[derive(Serialize, Deserialize, Debug, Clone)]
struct CachedCheck {
    checked_at_unix: u64,
    latest_tag: String,
}

#[derive(Deserialize)]
struct RemoteLatest {
    tag: String,
}

fn cache_path() -> Option<std::path::PathBuf> {
    nit_utils::paths::cache_dir().map(|d| d.join(CACHE_FILENAME))
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Fetches the latest release tag from the CDN. Returns `None` if curl
/// fails, times out, or the JSON doesn't parse. Network failures here
/// are non-fatal — every caller treats `None` as "no update info".
fn fetch_latest_tag() -> Option<String> {
    let output = std::process::Command::new("curl")
        .args([
            "-fsSL",
            "--max-time",
            &FETCH_TIMEOUT_SECS.to_string(),
            REMOTE_URL,
        ])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let body = String::from_utf8(output.stdout).ok()?;
    let parsed: RemoteLatest = serde_json::from_str(&body).ok()?;
    Some(parsed.tag)
}

fn load_cache() -> Option<CachedCheck> {
    let path = cache_path()?;
    let text = std::fs::read_to_string(&path).ok()?;
    serde_json::from_str(&text).ok()
}

fn save_cache(check: &CachedCheck) {
    let Some(path) = cache_path() else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(json) = serde_json::to_string(check) {
        let _ = std::fs::write(&path, json);
    }
}

/// Read-through cache: returns the latest tag from the cache when it's
/// fresh, otherwise fetches and refreshes. `None` on any failure.
fn cached_or_fresh_latest_tag() -> Option<String> {
    let now = now_unix();
    if let Some(cached) = load_cache() {
        if now.saturating_sub(cached.checked_at_unix) < CACHE_TTL_SECS {
            return Some(cached.latest_tag);
        }
    }
    let tag = fetch_latest_tag()?;
    save_cache(&CachedCheck {
        checked_at_unix: now,
        latest_tag: tag.clone(),
    });
    Some(tag)
}

/// Compare two `MAJOR.MINOR.PATCH` strings. Returns negative if `a < b`,
/// zero if equal, positive if `a > b`. Trailing pre-release suffixes
/// (e.g. `0.2.13-rc1`) are dropped before comparison — close enough for
/// nit's update-banner semantics; a true semver crate would be overkill
/// for one comparison.
fn compare_versions(a: &str, b: &str) -> i32 {
    fn parts(v: &str) -> Vec<u32> {
        let core = v.split('-').next().unwrap_or(v);
        core.split('.').filter_map(|s| s.parse().ok()).collect()
    }
    let a_parts = parts(a);
    let b_parts = parts(b);
    for (a, b) in a_parts.iter().zip(b_parts.iter()) {
        match a.cmp(b) {
            std::cmp::Ordering::Less => return -1,
            std::cmp::Ordering::Greater => return 1,
            std::cmp::Ordering::Equal => continue,
        }
    }
    match a_parts.len().cmp(&b_parts.len()) {
        std::cmp::Ordering::Less => -1,
        std::cmp::Ordering::Greater => 1,
        std::cmp::Ordering::Equal => 0,
    }
}

/// Print a one-line stderr notice if a newer release exists. Honors
/// `NIT_NO_VERSION_CHECK=1`. Best-effort: silent on any failure, so a
/// missing/broken cache or unreachable CDN never blocks startup.
pub(crate) fn print_update_notice_if_newer() {
    if std::env::var_os("NIT_NO_VERSION_CHECK").is_some() {
        return;
    }
    let Some(latest_tag) = cached_or_fresh_latest_tag() else {
        return;
    };
    let latest_v = latest_tag.trim_start_matches('v');
    let current = env!("CARGO_PKG_VERSION");
    if compare_versions(current, latest_v) >= 0 {
        return;
    }
    eprintln!("\u{2728} nit {latest_tag} is available \u{2014} run `nit update` to install (current: v{current})");
}

/// Detect whether the running `nit` binary lives under a known Homebrew
/// prefix. Used to pick `brew upgrade` over `install.sh` so we don't
/// end up with two installations stomping on each other's $PATH order.
///
/// Prefixes covered:
///   * `/opt/homebrew/`        — Apple Silicon default
///   * `/usr/local/Cellar/`    — Intel macOS Homebrew Cellar
///   * `/usr/local/opt/`       — Intel macOS Homebrew opt symlinks
///   * `/home/linuxbrew/.linuxbrew/` — Linuxbrew default
fn installed_via_homebrew() -> bool {
    let Ok(exe) = std::env::current_exe() else {
        return false;
    };
    // `canonicalize` resolves the symlink chain so Homebrew's
    // `/opt/homebrew/bin/nit → ../Cellar/nit/<ver>/bin/nit` is detected
    // even though the user's PATH points at the bin symlink.
    let resolved = std::fs::canonicalize(&exe).unwrap_or(exe);
    [
        "/opt/homebrew/",
        "/usr/local/Cellar/",
        "/usr/local/opt/",
        "/home/linuxbrew/.linuxbrew/",
    ]
    .iter()
    .any(|prefix| resolved.starts_with(prefix))
}

/// Run the `nit update` subcommand. Detects install method, prints the
/// version delta, then exec's the appropriate upgrade command.
pub(crate) fn run_update() -> anyhow::Result<()> {
    let current = env!("CARGO_PKG_VERSION");
    let Some(latest) = fetch_latest_tag() else {
        anyhow::bail!(
            "could not reach {REMOTE_URL} \u{2014} check your network connection \
             (or set NIT_NO_VERSION_CHECK=1 to disable update checks)"
        );
    };
    let latest_v = latest.trim_start_matches('v');

    eprintln!("Current version: v{current}");
    eprintln!("Latest release:  {latest}");
    eprintln!();

    if compare_versions(current, latest_v) >= 0 {
        eprintln!("Already up to date.");
        return Ok(());
    }

    eprintln!("Updating to {latest}...");
    eprintln!();

    if cfg!(target_os = "windows") {
        // Can't reliably replace a running .exe on Windows from the same
        // process. The install.ps1 script handles the post-launch
        // replacement cleanly; we surface it for the operator to run.
        eprintln!("Run this in PowerShell to install the new version:");
        eprintln!();
        eprintln!("  irm https://download.nit.tools/install.ps1 | iex");
        eprintln!();
        return Ok(());
    }

    if installed_via_homebrew() {
        eprintln!("Detected Homebrew install \u{2014} running: brew upgrade asheux/tap/nit");
        eprintln!();
        let status = std::process::Command::new("brew")
            .args(["upgrade", "asheux/tap/nit"])
            .status()?;
        if !status.success() {
            anyhow::bail!("brew upgrade exited with status {status}");
        }
    } else {
        eprintln!("Running: curl -fsSL https://download.nit.tools/install.sh | bash");
        eprintln!();
        let status = std::process::Command::new("sh")
            .args([
                "-c",
                "curl -fsSL https://download.nit.tools/install.sh | bash",
            ])
            .status()?;
        if !status.success() {
            anyhow::bail!("install script exited with status {status}");
        }
    }

    eprintln!();
    eprintln!("Done. Restart any running nit sessions to use the new version.");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compare_versions_orders_semver_segments() {
        assert!(compare_versions("0.2.10", "0.2.11") < 0);
        assert!(compare_versions("0.2.11", "0.2.10") > 0);
        assert_eq!(compare_versions("0.2.11", "0.2.11"), 0);
        assert!(compare_versions("0.2.9", "0.2.10") < 0); // string-cmp regression
        assert!(compare_versions("0.3.0", "0.2.99") > 0);
    }

    #[test]
    fn compare_versions_strips_prerelease_suffix() {
        // The retry banner doesn't need full semver semantics — pre-release
        // tags compare on the numeric prefix only. Two releases at the
        // same numeric version (one tagged -rc1, one final) compare equal
        // here, which is fine: the banner is informational, not a gate.
        assert_eq!(compare_versions("0.2.12-rc1", "0.2.12"), 0);
        assert!(compare_versions("0.2.12-rc1", "0.2.13") < 0);
    }

    #[test]
    fn installed_via_homebrew_classifies_known_prefixes() {
        // We can't intercept `current_exe` cleanly without a feature flag,
        // so directly exercise the path-prefix check via the same paths
        // the helper iterates over. Stays in sync with the production
        // list.
        let prefixes = [
            "/opt/homebrew/",
            "/usr/local/Cellar/",
            "/usr/local/opt/",
            "/home/linuxbrew/.linuxbrew/",
        ];
        let homebrew_paths = [
            "/opt/homebrew/bin/nit",
            "/usr/local/Cellar/nit/0.2.12/bin/nit",
            "/usr/local/opt/nit/bin/nit",
            "/home/linuxbrew/.linuxbrew/bin/nit",
        ];
        let non_homebrew = [
            "/usr/local/bin/nit",
            "/Users/nitrika/Downloads/nit",
            "/opt/nit/bin/nit",
        ];
        for p in homebrew_paths {
            let path = std::path::Path::new(p);
            assert!(
                prefixes.iter().any(|prefix| path.starts_with(prefix)),
                "{p} should be classified as Homebrew"
            );
        }
        for p in non_homebrew {
            let path = std::path::Path::new(p);
            assert!(
                !prefixes.iter().any(|prefix| path.starts_with(prefix)),
                "{p} should NOT be classified as Homebrew"
            );
        }
    }
}
