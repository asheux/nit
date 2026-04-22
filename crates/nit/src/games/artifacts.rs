use std::fs;
use std::path::Path;

use nit_games::output::{StrategyDefinition, TournamentResults};

/// Persist run artifacts (config snapshot, strategy definitions, tournament results) to disk.
///
/// Individual write failures are logged as warnings rather than propagated, so that
/// partial artifact output is still available even when one file fails.
pub(super) fn write_run_artifacts(
    toml_output_path: &Path,
    raw_config_content: &str,
    definitions_output_path: &Path,
    compiled_strategy_list: &[StrategyDefinition],
    results_output_path: &Path,
    match_outcome_data: &TournamentResults,
) {
    persist_artifact(toml_output_path, "config snapshot", |target_path| {
        fs::write(target_path, raw_config_content)?;
        Ok(())
    });

    persist_artifact(
        definitions_output_path,
        "strategy definitions",
        |target_path| {
            nit_utils::fs::write_atomic(target_path, |json_writer| {
                serde_json::to_writer_pretty(json_writer, compiled_strategy_list)
                    .map_err(std::io::Error::other)
            })?;
            Ok(())
        },
    );

    persist_artifact(results_output_path, "tournament results", |target_path| {
        nit_utils::fs::write_atomic(target_path, |json_writer| {
            serde_json::to_writer_pretty(json_writer, match_outcome_data)
                .map_err(std::io::Error::other)
        })?;
        Ok(())
    });
}

fn persist_artifact(
    file_target: &Path,
    description_tag: &str,
    writer_operation: impl FnOnce(&Path) -> anyhow::Result<()>,
) {
    if let Err(io_failure) = writer_operation(file_target) {
        eprintln!("Warning: failed to write {description_tag}: {io_failure}");
    }
}
