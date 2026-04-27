//! Runtime file-descriptor budget for the swarm.
//!
//! `MAX_SWARM_SIZE` is a static upper bound, but the practical ceiling is
//! whatever `RLIMIT_NOFILE` returns minus the headroom nit needs for itself.
//! macOS defaults to `ulimit -n 256`, which means the static 256-agent cap is
//! unreachable: each in-flight Codex/Claude exec turn opens 4 fds (stdin +
//! stdout + stderr + tmp out_file), so 64 concurrent agents alone would
//! exhaust the limit before the swarm even spins up.
//!
//! These helpers translate the runtime FD budget into a usable swarm size and
//! a corresponding "large swarm" warning threshold, surfaced to the operator
//! via the chat console when they request a roster size approaching the
//! ceiling.

use super::constants::{LARGE_SWARM_WARN_THRESHOLD, MAX_SWARM_SIZE};

/// Past this swarm size, lightweight planner models start producing
/// shallow / repetitive role assignments because nit doesn't do
/// hierarchical planning — the whole DAG is generated in one pass. Used
/// only for the operator advisory; doesn't affect what actually runs.
pub(crate) const LIGHT_PLANNER_SWARM_THRESHOLD: usize = 20;

/// Heuristic detector for "lightweight" planner models — token match
/// against the agent id, split on `-` / `.` / `_` / `/`. Bare substring
/// search misfires (e.g. "geMINI" contains "mini"), so we require the
/// tier marker to appear as its own segment. Picks up haiku / mini /
/// nano / flash-tier models across Anthropic, OpenAI, and Google.
/// Conservative: false positives just produce an extra advisory line,
/// never block the run.
pub(crate) fn is_light_planner(agent_id: &str) -> bool {
    let lower = agent_id.to_ascii_lowercase();
    // Strip swarm/chat-clone suffix so the suffix tokens (clone, swarm,
    // mis-NNN) can't influence detection.
    let base = lower.split('#').next().unwrap_or(lower.as_str());
    base.split(['-', '.', '_', '/'])
        .any(|tok| matches!(tok, "haiku" | "mini" | "nano" | "flash"))
}

/// Bulk template's per-dep budget collapses past ~10 proposers because
/// `SWARM_DEP_OUTPUT_TOTAL_MAX_CHARS_FULL` (240k) is split across deps —
/// at 12 proposers each gets ~20k chars (still meaningful), at 20 each
/// gets 12k, at 50 only 4.8k. Hard-cap at 12 with an advisory rather
/// than silently degrading proposal quality.
pub(crate) const BULK_PRACTICAL_MAX: usize = 12;

/// Per-agent file descriptor cost. Each Codex/Claude exec turn opens 4 fds
/// from nit's side (stdin/stdout/stderr pipes + the tmp out_file). Codex
/// MCP-mode is cheaper because all turns multiplex through one shared
/// connection, but we budget for the worst case so the ceiling is safe under
/// any backend mix.
const FDS_PER_AGENT: usize = 4;

/// Headroom reserved for nit itself: terminal IO, log file, MCP backchannel
/// socket, file watcher, and miscellaneous internal channels. Sized
/// generously so the budget calculation never starves the host runtime —
/// the actual baseline is closer to ~10–15 fds.
const NIT_BASELINE_FDS: usize = 32;

/// Read the process's current soft FD limit. On unix this is
/// `RLIMIT_NOFILE.rlim_cur`; on other platforms there's no FD-based limit so
/// we report a value high enough that `effective_max_swarm_size()` saturates
/// at `MAX_SWARM_SIZE`.
pub(crate) fn current_fd_soft_limit() -> usize {
    fd_soft_limit_impl()
}

#[cfg(unix)]
fn fd_soft_limit_impl() -> usize {
    // Safe wrapper over getrlimit(RLIMIT_NOFILE) — required because the
    // crate is `#![forbid(unsafe_code)]`.
    match rlimit::Resource::NOFILE.get() {
        Ok((soft, _hard)) => usize::try_from(soft).unwrap_or(usize::MAX),
        Err(_) => {
            // Failure is exceedingly rare; fall back to a value that makes
            // `effective_max_swarm_size` return `MAX_SWARM_SIZE` so the
            // user isn't artificially throttled because of a kernel quirk.
            permissive_fallback()
        }
    }
}

#[cfg(not(unix))]
fn fd_soft_limit_impl() -> usize {
    permissive_fallback()
}

const fn permissive_fallback() -> usize {
    // Tuned so `compute_effective_max_swarm_size` saturates at
    // `MAX_SWARM_SIZE` even with the baseline subtracted.
    MAX_SWARM_SIZE * FDS_PER_AGENT + NIT_BASELINE_FDS + 1
}

/// Effective swarm-size ceiling clamped to whatever the host's FD budget can
/// actually support. Always at least 1, never above `MAX_SWARM_SIZE`.
pub(crate) fn effective_max_swarm_size() -> usize {
    compute_effective_max_swarm_size(current_fd_soft_limit())
}

/// Pure helper split out for unit testing — given any FD limit, compute the
/// swarm-size ceiling. Saturates at `MAX_SWARM_SIZE` (the static upper
/// bound) and clamps to ≥ 1 even when `fd_limit < NIT_BASELINE_FDS` so a
/// degenerate limit doesn't deadlock the runtime.
pub(crate) fn compute_effective_max_swarm_size(fd_limit: usize) -> usize {
    let budget = fd_limit.saturating_sub(NIT_BASELINE_FDS) / FDS_PER_AGENT;
    budget.clamp(1, MAX_SWARM_SIZE)
}

/// Threshold at which a "large swarm" advisory is pushed to the mission
/// console. On a host with abundant fds this is the static
/// `LARGE_SWARM_WARN_THRESHOLD`; on a host with a tight `ulimit -n` it
/// drops to ~75% of the effective ceiling so the warning fires before
/// subprocess spawning starts hitting `EMFILE`.
pub(crate) fn large_swarm_warn_threshold() -> usize {
    compute_large_swarm_warn_threshold(current_fd_soft_limit())
}

pub(crate) fn compute_large_swarm_warn_threshold(fd_limit: usize) -> usize {
    let ceiling = compute_effective_max_swarm_size(fd_limit);
    if ceiling < LARGE_SWARM_WARN_THRESHOLD {
        // FD-bound host: warn at ~75% of the ceiling so the operator sees
        // the advisory with headroom to back off.
        (ceiling * 3 / 4).max(1)
    } else {
        LARGE_SWARM_WARN_THRESHOLD
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ceiling_saturates_at_max_when_fds_abundant() {
        assert_eq!(compute_effective_max_swarm_size(65_536), MAX_SWARM_SIZE);
        assert_eq!(compute_effective_max_swarm_size(usize::MAX), MAX_SWARM_SIZE);
    }

    #[test]
    fn ceiling_scales_with_macos_default_ulimit() {
        // macOS default `ulimit -n` is 256. (256 - 32) / 4 = 56.
        assert_eq!(compute_effective_max_swarm_size(256), 56);
    }

    #[test]
    fn ceiling_scales_with_linux_default_ulimit() {
        // Linux default is 1024. (1024 - 32) / 4 = 248.
        assert_eq!(compute_effective_max_swarm_size(1024), 248);
    }

    #[test]
    fn ceiling_clamps_to_one_for_degenerate_limits() {
        assert_eq!(compute_effective_max_swarm_size(0), 1);
        assert_eq!(compute_effective_max_swarm_size(NIT_BASELINE_FDS - 1), 1);
        assert_eq!(compute_effective_max_swarm_size(NIT_BASELINE_FDS), 1);
        assert_eq!(compute_effective_max_swarm_size(NIT_BASELINE_FDS + 1), 1);
    }

    #[test]
    fn warn_threshold_fires_below_static_when_fd_bound() {
        // ulimit -n 256 → ceiling 56 → warn at 42 (75%).
        assert_eq!(compute_large_swarm_warn_threshold(256), 42);
    }

    #[test]
    fn warn_threshold_uses_static_when_fds_abundant() {
        // ulimit -n 4096 → ceiling 256 (saturated) → static threshold 64.
        assert_eq!(
            compute_large_swarm_warn_threshold(4096),
            LARGE_SWARM_WARN_THRESHOLD
        );
    }

    #[test]
    fn warn_threshold_never_zero() {
        // Even with degenerate limits, we still emit the advisory rather
        // than silently swallow it.
        assert_eq!(compute_large_swarm_warn_threshold(0), 1);
    }

    #[test]
    fn current_soft_limit_is_positive() {
        // Smoke test: on any host this is supposed to run on, the soft
        // limit must be > the baseline.
        assert!(current_fd_soft_limit() > NIT_BASELINE_FDS);
    }

    #[test]
    fn is_light_planner_matches_known_lightweight_tiers() {
        // Anthropic.
        assert!(is_light_planner("claude-haiku-4-5"));
        assert!(is_light_planner("claude-haiku-3-5"));
        // OpenAI lightweight variants.
        assert!(is_light_planner("gpt-5-mini"));
        assert!(is_light_planner("gpt-5-nano"));
        assert!(is_light_planner("o4-mini"));
        // Google.
        assert!(is_light_planner("gemini-2.5-flash"));
        // Case-insensitive.
        assert!(is_light_planner("Claude-HAIKU-4-5"));
        assert!(is_light_planner("GPT-5-MINI"));
    }

    #[test]
    fn is_light_planner_excludes_heavy_tiers() {
        assert!(!is_light_planner("claude-opus-4-7"));
        assert!(!is_light_planner("claude-sonnet-4-6"));
        assert!(!is_light_planner("gpt-5"));
        assert!(!is_light_planner("gpt-5.4"));
        assert!(!is_light_planner("gemini-2.5-pro"));
        // Empty / odd inputs don't crash and report not-light.
        assert!(!is_light_planner(""));
        assert!(!is_light_planner("custom-model"));
    }

    #[test]
    fn is_light_planner_strips_clone_suffix() {
        // Swarm clone suffix shouldn't change the answer — substring match
        // catches the base model name regardless of suffix.
        assert!(is_light_planner("claude-haiku-4-5#swarm-mis-001-clone-01"));
        assert!(!is_light_planner("claude-opus-4-7#swarm-mis-001-clone-01"));
    }
}
