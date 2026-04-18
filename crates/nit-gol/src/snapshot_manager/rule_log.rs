//! Append-only log of scored rule discoveries, serialized as JSON-lines.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use crate::analyze::RuleEvaluation;
use crate::snapshot::now_iso8601;

/// A scored-rule discovery record appended to the JSON-lines log.
#[derive(Clone, Debug, serde::Serialize)]
pub struct RuleLogEntry {
    rule: String,
    score: f32,
    discovered_at: String,
    seed_hash: u64,
    notes: String,
    // Destination log file; routed by the worker, not serialized.
    #[serde(skip)]
    path: PathBuf,
}

impl RuleLogEntry {
    pub fn from_eval(eval: &RuleEvaluation, seed_hash: u64, path: &Path) -> Self {
        Self {
            rule: eval.rule.to_string(),
            score: eval.score,
            discovered_at: now_iso8601(),
            seed_hash,
            notes: format_notes(eval),
            path: path.to_path_buf(),
        }
    }
}

/// Human-readable diagnostic string. The `{:?}` on the period `Option`
/// emits `Some(..)` / `None` — intentional, the log is for humans.
fn format_notes(eval: &RuleEvaluation) -> String {
    format!(
        "period={:?} transient={} alive_end={}",
        eval.period, eval.transient, eval.alive_end
    )
}

/// Append the entry as one JSON object followed by `\n`.
///
/// Writing the newline separately (rather than via `writeln!`) keeps the
/// exact byte sequence platform-independent — `writeln!` on Windows could
/// otherwise surprise readers that split on `\n`.
pub(super) fn append(entry: RuleLogEntry) -> io::Result<()> {
    if let Some(parent) = entry.path.parent().filter(|p| !p.exists()) {
        fs::create_dir_all(parent)?;
    }
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&entry.path)?;
    serde_json::to_writer(&mut file, &entry).map_err(io::Error::other)?;
    file.write_all(b"\n")?;
    Ok(())
}
