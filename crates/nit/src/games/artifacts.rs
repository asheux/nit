use std::fs;
use std::path::Path;

use serde::Serialize;

use nit_games::output::{StrategyDefinition, TournamentResults};

// Per-artifact failures are logged and swallowed so a single bad write
// does not erase the other artifacts produced by a tournament run.
pub(super) fn write_run_artifacts(
    config_path: &Path,
    config_text: &str,
    definitions_path: &Path,
    definitions: &[StrategyDefinition],
    results_path: &Path,
    results: &TournamentResults,
) {
    persist_artifact(config_path, "config snapshot", |target| {
        fs::write(target, config_text)?;
        Ok(())
    });
    persist_artifact(definitions_path, "strategy definitions", |target| {
        write_json_pretty(target, definitions)
    });
    persist_artifact(results_path, "tournament results", |target| {
        write_json_pretty(target, results)
    });
}

fn write_json_pretty<T: Serialize + ?Sized>(path: &Path, value: &T) -> anyhow::Result<()> {
    nit_utils::fs::write_atomic(path, |writer| {
        serde_json::to_writer_pretty(writer, value).map_err(std::io::Error::other)
    })?;
    Ok(())
}

fn persist_artifact(path: &Path, label: &str, write: impl FnOnce(&Path) -> anyhow::Result<()>) {
    if let Err(err) = write(path) {
        eprintln!("Warning: failed to write {label}: {err}");
    }
}
