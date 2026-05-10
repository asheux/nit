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

// Past this swarm size, lightweight planner models start producing shallow /
// repetitive role assignments because nit doesn't do hierarchical planning —
// the whole DAG is generated in one pass. Operator advisory only.
pub(crate) const LIGHT_PLANNER_SWARM_THRESHOLD: usize = 20;

// Bulk template's per-dep budget collapses past ~10 proposers because
// `SWARM_DEP_OUTPUT_TOTAL_MAX_CHARS_FULL` (240k) is split across deps —
// at 12 proposers each gets ~20k chars (still meaningful), at 20 each gets
// 12k, at 50 only 4.8k. Hard-cap at 12 with an advisory rather than silently
// degrading proposal quality.
pub(crate) const BULK_PRACTICAL_MAX: usize = 12;

// Per-agent file-descriptor cost. Each Codex/Claude exec turn opens 4 fds
// from nit's side (stdin/stdout/stderr pipes + the tmp out_file). MCP-mode
// is cheaper (one shared connection per backend) but we budget for the
// worst case so the ceiling is safe under any backend mix.
const FDS_PER_AGENT: usize = 4;

// Headroom reserved for nit itself: terminal IO, log file, MCP backchannel
// socket, file watcher, and miscellaneous internal channels. Sized
// generously — actual baseline is closer to ~10–15 fds.
pub(super) const NIT_BASELINE_FDS: usize = 32;

// Token-segment match against the agent id, split on `-` / `.` / `_` / `/`.
// Bare substring search misfires (e.g. "geMINI" contains "mini"), so the
// tier marker must appear as its own segment. Picks up haiku / mini / nano /
// flash-tier models across Anthropic, OpenAI, and Google. Conservative:
// false positives just produce an extra advisory line, never block the run.
pub(crate) fn is_light_planner(agent_id: &str) -> bool {
    let lower = agent_id.to_ascii_lowercase();
    // Strip the swarm/chat-clone suffix so its tokens (clone, swarm,
    // mis-NNN) can't influence detection.
    let base = lower.split('#').next().unwrap_or(lower.as_str());
    base.split(['-', '.', '_', '/'])
        .any(|tok| matches!(tok, "haiku" | "mini" | "nano" | "flash"))
}

// Pool sizing: a fraction of the effective swarm ceiling, so a tight
// `ulimit -n` drops pool size in lock-step.
const CLAUDE_POOL_DEFAULT_FLOOR: usize = 2;
const CLAUDE_POOL_DEFAULT_CEILING: usize = 8;
const CLAUDE_POOL_FRACTION_DIVISOR: usize = 4;

// On unix this is `RLIMIT_NOFILE.rlim_cur`; on other platforms there's no
// FD-based limit so we report a value high enough that the budget formula
// saturates at `MAX_SWARM_SIZE`.
pub(crate) fn current_fd_soft_limit() -> usize {
    // Tuned so `compute_effective_max_swarm_size` saturates at
    // `MAX_SWARM_SIZE` even after the baseline subtraction.
    const PERMISSIVE_FALLBACK: usize = MAX_SWARM_SIZE * FDS_PER_AGENT + NIT_BASELINE_FDS + 1;
    #[cfg(unix)]
    {
        // rlimit crate wraps getrlimit(RLIMIT_NOFILE); a direct call requires
        // unsafe, which is forbidden by `#![forbid(unsafe_code)]`.
        match rlimit::Resource::NOFILE.get() {
            Ok((soft, _hard)) => usize::try_from(soft).unwrap_or(usize::MAX),
            // Failure is exceedingly rare; fall back to a value that makes
            // `effective_max_swarm_size` saturate at `MAX_SWARM_SIZE` so the
            // user isn't artificially throttled by a kernel quirk.
            Err(_) => PERMISSIVE_FALLBACK,
        }
    }
    #[cfg(not(unix))]
    {
        PERMISSIVE_FALLBACK
    }
}

// Effective swarm-size ceiling clamped to whatever the host's FD budget can
// actually support. Always at least 1, never above `MAX_SWARM_SIZE`. `pub`
// because the `nit` binary calls this directly to scale multipane. When the
// Claude warm pool is enabled, `pool_size` slots also consume ~4 fds each;
// see CLAUDE.md "Swarm size limits".
pub fn effective_max_swarm_size() -> usize {
    compute_effective_max_swarm_size(current_fd_soft_limit())
}

// Pure helper split out for unit testing. Saturates at `MAX_SWARM_SIZE` and
// clamps to ≥ 1 even when `fd_limit < NIT_BASELINE_FDS` so a degenerate
// limit doesn't deadlock the runtime.
pub(crate) fn compute_effective_max_swarm_size(fd_limit: usize) -> usize {
    let budget = fd_limit.saturating_sub(NIT_BASELINE_FDS) / FDS_PER_AGENT;
    budget.clamp(1, MAX_SWARM_SIZE)
}

pub fn default_claude_pool_size() -> usize {
    compute_default_claude_pool_size(effective_max_swarm_size())
}

pub(crate) fn compute_default_claude_pool_size(effective_max: usize) -> usize {
    let quarter = effective_max / CLAUDE_POOL_FRACTION_DIVISOR;
    quarter.clamp(CLAUDE_POOL_DEFAULT_FLOOR, CLAUDE_POOL_DEFAULT_CEILING)
}

// On a host with abundant fds this is the static
// `LARGE_SWARM_WARN_THRESHOLD`; on a host with a tight `ulimit -n` it drops
// to ~75% of the effective ceiling so the warning fires before subprocess
// spawning starts hitting `EMFILE`.
pub(crate) fn large_swarm_warn_threshold() -> usize {
    compute_large_swarm_warn_threshold(current_fd_soft_limit())
}

pub(crate) fn compute_large_swarm_warn_threshold(fd_limit: usize) -> usize {
    let ceiling = compute_effective_max_swarm_size(fd_limit);
    if ceiling < LARGE_SWARM_WARN_THRESHOLD {
        // FD-bound host: warn at ~75% of the ceiling so the operator has
        // headroom to back off.
        (ceiling * 3 / 4).max(1)
    } else {
        LARGE_SWARM_WARN_THRESHOLD
    }
}
