//! Warm pool of long-lived `claude -p --input-format stream-json` workers.
//!
//! Gated behind `NIT_CLAUDE_POOL=1`. The default (unset or `=0`) keeps the
//! cold-spawn path in `claude_runner.rs` byte-identical to today's per-turn
//! subprocess behaviour — that path is the rollback story for at least one
//! release after the pool flag flips to default-on.
//!
//! ## Slot model
//!
//! Each slot owns:
//! - a long-lived `claude -p` child with persistent stdin
//! - a stdout reader thread that demultiplexes stream-json lines into a
//!   per-checkout [`crossbeam_channel`] bounded at [`POOL_LINE_CAPACITY`]
//! - a stderr collector capped at [`POOL_STDERR_CAP_BYTES`] (100 MB tail
//!   window, mirroring `STDOUT_TAIL_CAP_BYTES` in `claude_runner.rs`)
//!
//! Slots are keyed by [`WorkerKey`] — a turn can only reuse a slot whose
//! spawn-time CLI args match its own (model, cwd, read-only allowlist).
//! Agent-affinity is a secondary preference inside a key match: same-agent
//! reuse is preferred, then any compatible slot, then a fresh spawn.
//!
//! ## Failure isolation
//!
//! Every unhealthy condition (BrokenPipe on write, stream-json `error`
//! envelope, non-zero child exit, auth banner, operator cancel, idle
//! timeout, age GC) replaces the slot rather than returning it. The
//! premise is that a slot in an unknown state poisons the next turn; the
//! cost of a replacement (one extra spawn) is dwarfed by the cost of a
//! silently corrupted turn.

use std::time::Duration;

mod pool;
mod worker;

pub use pool::ClaudePool;
pub use worker::{PoolLine, PoolWorker, RecycleReason, SpawnedChild, WorkerKey, WorkerSpawn};

pub const POOL_LINE_CAPACITY: usize = 64;
pub const POOL_STDERR_CAP_BYTES: usize = 100 * 1024 * 1024;
pub const POOL_IDLE_MAX: Duration = Duration::from_secs(60 * 60);
pub(crate) const POOL_GC_INTERVAL: Duration = Duration::from_secs(5 * 60);

/// Build the stream-json envelope sent over a pooled worker's stdin for a
/// single turn. Mirrors the protocol the Claude CLI accepts when launched
/// with `--input-format stream-json`.
pub fn build_stream_json_envelope(prompt: &str) -> String {
    let payload = serde_json::json!({
        "type": "user",
        "message": {
            "role": "user",
            "content": prompt,
        }
    });
    let mut envelope = payload.to_string();
    envelope.push('\n');
    envelope
}

/// True when an env-var value parses as a positive opt-in (`1` / `true` /
/// `yes`, case-insensitive). Empty / unset → false.
pub fn env_flag_enabled(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "yes" | "on"
    )
}

/// Read `NIT_CLAUDE_POOL` from the environment.
pub fn pool_enabled_from_env() -> bool {
    std::env::var("NIT_CLAUDE_POOL")
        .ok()
        .map(|v| env_flag_enabled(&v))
        .unwrap_or(false)
}

/// Read `NIT_CLAUDE_POOL_SIZE` from the environment, falling back to
/// [`crate::swarm::limits::default_claude_pool_size`] when unset or
/// unparseable.
pub fn pool_size_from_env() -> usize {
    if let Ok(raw) = std::env::var("NIT_CLAUDE_POOL_SIZE") {
        if let Ok(n) = raw.trim().parse::<usize>() {
            if n > 0 {
                return n;
            }
        }
    }
    crate::swarm::default_claude_pool_size()
}

#[cfg(test)]
#[path = "../tests/claude_pool.rs"]
mod tests;
