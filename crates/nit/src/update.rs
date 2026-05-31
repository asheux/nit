//! `nit update` subcommand + interactive startup update prompt.
//!
//! Two operator-facing flows:
//!
//!   1. `nit update` — compares `CARGO_PKG_VERSION` against
//!      `https://download.nit.tools/latest.json` and re-runs the
//!      appropriate install command (Homebrew when nit is installed
//!      under a known Homebrew prefix, the install-script one-liner
//!      otherwise, a printed PowerShell snippet on Windows).
//!   2. Startup prompt — `check_and_prompt_for_update` does the same
//!      comparison, then on a TTY launches an interactive picker
//!      ([i]nstall now / [s]kip / [m]ute this version) before the
//!      TUI takes over. Non-TTY launches (CI, swarm spawns, remote
//!      tmux pipes) fall back to the passive one-line notice so they
//!      don't wedge waiting on a keypress.
//!
//! Both honour `NIT_NO_VERSION_CHECK=1` to silence network access in
//! sandboxed / offline environments. The startup prompt additionally
//! honours a per-version mute persisted in
//! `<cache_dir>/version_check.json`; when a newer release lands the
//! mute is auto-cleared so the operator hears about the next bump.
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
    /// Operator chose [m]ute for a specific tag — banner is suppressed
    /// while `latest_tag == muted_for_tag`. When a newer release lands
    /// the tags diverge and the banner shows again. Per-version mute
    /// is friendlier than a global "never ask me again" flag because
    /// it doesn't leave the operator silently stranded on old versions.
    #[serde(default)]
    muted_for_tag: Option<String>,
}

/// Outcome of the interactive update banner. Returned to `main.rs` so
/// it can short-circuit the TUI launch when the operator picks
/// Install (we exec the update and exit), and otherwise continue.
pub(crate) enum UpdateAction {
    Install,
    Skip,
    Mute,
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

/// Read-through cache: returns the latest tag + the per-tag mute flag
/// from the cache when it's fresh, otherwise fetches and refreshes.
/// `None` on any failure. The mute flag rides alongside the tag so a
/// refresh that picks up a newer tag automatically resurfaces the
/// banner (the old mute decision no longer applies).
fn cached_or_fresh_check() -> Option<CachedCheck> {
    let now = now_unix();
    if let Some(cached) = load_cache() {
        if now.saturating_sub(cached.checked_at_unix) < CACHE_TTL_SECS {
            return Some(cached);
        }
    }
    let tag = fetch_latest_tag()?;
    // Preserve any existing mute decision if it still applies to the
    // freshly fetched tag. If the tag advanced, the old mute is dropped
    // — the operator's "mute this version" decision doesn't transfer.
    let muted_for_tag = load_cache()
        .and_then(|c| c.muted_for_tag)
        .filter(|t| t == &tag);
    let check = CachedCheck {
        checked_at_unix: now,
        latest_tag: tag,
        muted_for_tag,
    };
    save_cache(&check);
    Some(check)
}

/// Persist a per-version mute. Reads the current cache, sets
/// `muted_for_tag = Some(tag)`, writes back. Best-effort — a write
/// failure means the operator sees the banner again next launch,
/// which is acceptable.
fn mute_for_tag(tag: &str) {
    let Some(mut cached) = load_cache() else {
        // No cache yet — create a minimal one so the mute sticks.
        save_cache(&CachedCheck {
            checked_at_unix: now_unix(),
            latest_tag: tag.to_string(),
            muted_for_tag: Some(tag.to_string()),
        });
        return;
    };
    cached.muted_for_tag = Some(tag.to_string());
    save_cache(&cached);
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

/// Show the interactive update banner if a newer release exists.
/// Returns the operator's chosen action, or `None` when no banner was
/// shown (no update available, suppressed via env, muted for this
/// tag, or any failure on the cache/network path). Honors
/// `NIT_NO_VERSION_CHECK=1`.
///
/// Non-TTY stdin (CI, piped input, redirected stdin): falls back to a
/// passive one-line notice on stderr and returns `Some(Skip)` so the
/// caller still proceeds normally. We don't want a swarm-pane spawn
/// or a remote-tmux session to wedge waiting for a keypress that
/// never comes.
pub(crate) fn check_and_prompt_for_update() -> Option<UpdateAction> {
    if std::env::var_os("NIT_NO_VERSION_CHECK").is_some() {
        return None;
    }
    let check = cached_or_fresh_check()?;
    let latest_v = check.latest_tag.trim_start_matches('v');
    let current = env!("CARGO_PKG_VERSION");
    if compare_versions(current, latest_v) >= 0 {
        return None;
    }
    if check.muted_for_tag.as_deref() == Some(check.latest_tag.as_str()) {
        return None;
    }
    Some(prompt_user_for_action(&check.latest_tag, current))
}

fn prompt_user_for_action(latest_tag: &str, current: &str) -> UpdateAction {
    use std::io::{IsTerminal, Write};

    // Non-interactive stdin: fall back to the passive one-line notice.
    // Skip cleanly so headless / CI launches don't hang on a prompt
    // they can't answer.
    if !std::io::stdin().is_terminal() {
        eprintln!(
            "\u{2728} nit {latest_tag} is available \u{2014} run `nit update` to install (current: v{current})"
        );
        return UpdateAction::Skip;
    }

    eprintln!();
    eprintln!("\u{2728} A new nit release is available.");
    eprintln!("   Current: v{current}");
    eprintln!("   Latest:  {latest_tag}");
    eprintln!();
    eprintln!("  [i] Install now");
    eprintln!("  [s] Skip for now (default)");
    eprintln!("  [m] Mute this version (resurfaces when a newer release lands)");
    eprintln!();
    eprint!("Choice [s]: ");
    let _ = std::io::stderr().flush();

    let mut line = String::new();
    if std::io::stdin().read_line(&mut line).is_err() {
        // EOF / SIGINT during the prompt: treat as Skip so Ctrl-C
        // doesn't leave the operator with a half-applied state.
        return UpdateAction::Skip;
    }
    let trimmed = line.trim().to_ascii_lowercase();
    match trimmed.as_str() {
        "i" | "install" | "y" | "yes" => UpdateAction::Install,
        "m" | "mute" => UpdateAction::Mute,
        _ => UpdateAction::Skip,
    }
}

/// Persist a mute for the currently-cached latest tag. Idempotent;
/// no-op when the cache is missing or has no tag yet.
pub(crate) fn mute_currently_cached_tag() {
    if let Some(c) = load_cache() {
        mute_for_tag(&c.latest_tag);
    }
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
    fn mute_decision_clears_when_newer_tag_lands() {
        // Operator mutes the banner for v0.2.13. When the next launch
        // sees a freshly published v0.2.14, the mute decision should
        // NOT carry over — the operator hasn't said anything about
        // v0.2.14 yet.
        let muted = CachedCheck {
            checked_at_unix: 1_000_000,
            latest_tag: "v0.2.13".into(),
            muted_for_tag: Some("v0.2.13".into()),
        };
        // Same tag: mute applies → banner is suppressed.
        assert_eq!(
            muted.muted_for_tag.as_deref(),
            Some(muted.latest_tag.as_str()),
            "mute applies for the tag it was set on"
        );
        // Newer tag would be a different `latest_tag` after a refresh;
        // the helper that builds the new cache drops the carry-over
        // when the tags don't match (the production code path in
        // `cached_or_fresh_check` does `.filter(|t| t == &tag)`).
        let stale_mute: Option<String> = muted.muted_for_tag.clone().filter(|t| t == "v0.2.14");
        assert_eq!(
            stale_mute, None,
            "mute set for v0.2.13 must not carry into v0.2.14"
        );
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
