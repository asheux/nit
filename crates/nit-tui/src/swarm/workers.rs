use std::collections::HashMap;
use std::sync::mpsc;

use nit_core::AppState;

use super::{GenomeReviewPending, SwarmRun};

struct GenomeReviewInput {
    files_to_eval: Vec<std::path::PathBuf>,
    /// Baselines captured at turn start, keyed by file path. Used as the
    /// "before" side of the genome diff so the reviewer sees real change.
    baselines: HashMap<std::path::PathBuf, nit_core::GenomeReport>,
}

/// If the genome gate is enabled and a verifier agent exists, kick off the
/// background prompt build for the genome reviewer. Stores the receiver on
/// the run so `poll_genome_reviews` can pick up the result on a later tick.
pub(super) fn maybe_spawn_genome_review(run: &mut SwarmRun, state: &AppState) {
    if !state.settings.genome.genome_gate_enabled {
        return;
    }
    let Some(reviewer_id) = run.verifier_agent_id.clone() else {
        return;
    };
    run.genome_review_pending = Some(GenomeReviewPending {
        rx: spawn_genome_review_prompt(state, &run.mission_id, &run.initial_genome_baselines),
        reviewer_id,
    });
}

/// Spawn the genome review prompt build on a background thread and return a
/// receiver that delivers the prompt string. The main thread polls this with
/// `try_recv` so the UI never blocks while running multiple
/// `compute_genome_report` calls (each one is a 3000-generation GoL sim).
///
/// An empty string in the channel means the worker had nothing to evaluate
/// (no modified files) — the poller skips dispatching the reviewer in that
/// case.
fn spawn_genome_review_prompt(
    state: &AppState,
    mission_id: &str,
    mission_baselines: &HashMap<std::path::PathBuf, nit_core::GenomeReport>,
) -> mpsc::Receiver<String> {
    let (tx, rx) = mpsc::channel();

    // Prefer the mission-scoped accumulator so files from earlier turns of
    // the same mission aren't lost when an agent runs multiple sequential
    // tasks (each TurnStarted clears `genome_turn_modified[agent]`).
    // Fall back to unioning `genome_turn_modified` for defence-in-depth if
    // the mission key is somehow empty.
    let mut files_to_eval: Vec<std::path::PathBuf> =
        match state.genome_mission_modified.get(mission_id) {
            Some(set) if !set.is_empty() => set.iter().cloned().collect(),
            _ => state
                .genome_turn_modified
                .values()
                .flat_map(|s| s.iter().cloned())
                .collect::<std::collections::HashSet<_>>()
                .into_iter()
                .collect(),
        };
    if let Some(editor_path) = state.editor_buffer().path().cloned() {
        if !files_to_eval.contains(&editor_path) {
            files_to_eval.push(editor_path);
        }
    }
    files_to_eval.sort();

    // Use the mission-scoped snapshot (frozen at swarm start) as the "before".
    // `state.genome_baselines` is per-turn and gets cleared/re-captured
    // between agents, so by the time the review runs it equals current state
    // and `compute_genome_diff` returns +0.00 for every encoder.
    let baselines: HashMap<std::path::PathBuf, nit_core::GenomeReport> = files_to_eval
        .iter()
        .filter_map(|p| mission_baselines.get(p).map(|r| (p.clone(), r.clone())))
        .collect();

    let input = GenomeReviewInput {
        files_to_eval,
        baselines,
    };

    std::thread::Builder::new()
        .name("genome-review".into())
        .spawn(move || {
            let result = build_genome_review_prompt_bg(&input);
            let _ = tx.send(result);
        })
        .ok();

    rx
}

/// Build the genome review prompt for the genome-reviewer role on a worker
/// thread. Reads each modified file and computes a full genome report — this
/// is the expensive work (tree-sitter + 3000-gen GoL + parsimony per file)
/// that previously blocked the main loop for "Genome Quality Review".
fn build_genome_review_prompt_bg(input: &GenomeReviewInput) -> String {
    let mut prompt = String::from(
        "You are the genome reviewer in nit's coding lab. nit measures structural code \
         quality by encoding source files as Game of Life genomes. The lab's goal is to \
         produce elite Replicator-tier (Tier V, 2001+ generations) code. Evaluate the \
         structural quality of the code changes made by this swarm mission. For each \
         modified file, a genome report shows before/after metrics across four encoders.\n\n",
    );

    let mut has_content = false;
    for file_path in &input.files_to_eval {
        let text = match std::fs::read_to_string(file_path) {
            Ok(t) => t,
            Err(_) => continue,
        };
        let report = nit_core::compute_genome_report(&text, file_path);
        prompt.push_str(&format!("--- {} ---\n", file_path.display()));
        prompt.push_str(&nit_core::format_genome_report(&report));
        prompt.push('\n');

        if let Some(prev) = input.baselines.get(file_path) {
            let diff = nit_core::compute_genome_diff(prev, &report);
            prompt.push_str(&nit_core::format_genome_diff(&diff));
            prompt.push('\n');
        }
        has_content = true;
    }

    if !has_content {
        return String::new();
    }

    prompt.push_str(
        "\nProduce a structured review:\n\
         1. Which files improved in structural quality and which regressed\n\
         2. The most critical structural issues remaining\n\
         3. Specific refactoring recommendations for the worst-scoring files\n\
         4. Overall verdict: PASS (all files tier III+ Spaceship) or FAIL (any file below tier III)\n\
         5. Distance from Replicator (Tier V) — what would it take to reach elite status\n",
    );

    prompt
}

/// Snapshot of state needed by the genome gate evaluation thread.
struct GenomeGateInput {
    config: nit_core::config::GenomeGateConfig,
    files_to_eval: Vec<std::path::PathBuf>,
    /// Previous genome reports for regression checks (file → tier).
    prev_tiers: HashMap<std::path::PathBuf, nit_core::GenomeTier>,
}

/// Spawn the genome gate evaluation on a background thread and return a
/// receiver that will deliver the result string. The main thread should
/// poll this with `try_recv` so the UI never blocks.
pub(super) fn spawn_genome_gate_eval(
    state: &AppState,
    mission_id: &str,
    mission_baselines: &HashMap<std::path::PathBuf, nit_core::GenomeReport>,
) -> mpsc::Receiver<String> {
    let (tx, rx) = mpsc::channel();

    // Prefer the mission-scoped accumulator for the same reason as the
    // reviewer path: `genome_turn_modified` is cleared on each TurnStarted,
    // so an agent running multiple sequential tasks within a mission would
    // lose files from earlier turns without this.
    let mut files_to_eval: Vec<std::path::PathBuf> =
        match state.genome_mission_modified.get(mission_id) {
            Some(set) if !set.is_empty() => set.iter().cloned().collect(),
            _ => state
                .genome_turn_modified
                .values()
                .flat_map(|s| s.iter().cloned())
                .collect::<std::collections::HashSet<_>>()
                .into_iter()
                .collect(),
        };
    if let Some(editor_path) = state.editor_buffer().path().cloned() {
        if !files_to_eval.contains(&editor_path) {
            files_to_eval.push(editor_path);
        }
    }
    files_to_eval.sort();

    // Use the mission-scoped snapshot (frozen at swarm start) for regression
    // comparison. `state.genome_baselines` is per-turn and gets cleared
    // between agents, so by the time the gate runs it's empty and falling
    // back to `state.genome_reports` (post-change state) silently masks real
    // regressions. For files not in the mission snapshot (created during the
    // swarm), fall back to current state — "new file not regressed" is the
    // correct semantics there.
    let prev_tiers: HashMap<std::path::PathBuf, nit_core::GenomeTier> = files_to_eval
        .iter()
        .filter_map(|p| {
            mission_baselines
                .get(p)
                .or_else(|| state.genome_reports.get(p))
                .map(|r| (p.clone(), r.tier))
        })
        .collect();

    let input = GenomeGateInput {
        config: state.settings.genome.genome_gate.clone(),
        files_to_eval,
        prev_tiers,
    };

    std::thread::Builder::new()
        .name("genome-gate".into())
        .spawn(move || {
            let result = evaluate_genome_gate_bg(&input);
            let _ = tx.send(result);
        })
        .ok();

    rx
}

/// Evaluate genome quality on ALL modified files and produce a gate result
/// string.  Runs on a background thread — all data is passed via `input`.
fn evaluate_genome_gate_bg(input: &GenomeGateInput) -> String {
    let genome_config = &input.config;
    let min_tier = nit_core::GenomeTier::from_generations(match genome_config.min_tier {
        0 => 0,
        1 => 51,
        2 => 201,
        3 => 501,
        _ => 2001,
    });

    let mut out = String::new();
    let mut all_failures: Vec<String> = Vec::new();
    let mut file_count = 0u32;
    let mut pass_count = 0u32;

    for file_path in &input.files_to_eval {
        let text = match std::fs::read_to_string(file_path) {
            Ok(t) => t,
            Err(_) => continue,
        };
        let report = nit_core::compute_genome_report(&text, file_path);
        let mut failures = Vec::new();
        file_count += 1;

        if report.tier < min_tier {
            failures.push(format!(
                "Genome FAIL: {} tier {} ({}) below minimum {} ({})",
                file_path.display(),
                report.tier.numeral(),
                report.tier.name(),
                min_tier.numeral(),
                min_tier.name(),
            ));
        }

        for score in &report.encoder_scores {
            if matches!(
                score.encoder,
                nit_core::SeedEncoderId::TokenSpectrum
                    | nit_core::SeedEncoderId::AstStructure
                    | nit_core::SeedEncoderId::ComplexityField
            ) && score.density > genome_config.max_density
            {
                failures.push(format!(
                    "Genome FAIL: {} density {:.2} on {} exceeds {:.2}",
                    file_path.display(),
                    score.density,
                    score.encoder.label(),
                    genome_config.max_density,
                ));
            }
        }

        if let Some(s) = report
            .encoder_scores
            .iter()
            .find(|s| s.encoder == nit_core::SeedEncoderId::AstStructure)
        {
            if s.components < genome_config.min_components {
                failures.push(format!(
                    "Genome FAIL: {} has {} components (min: {})",
                    file_path.display(),
                    s.components,
                    genome_config.min_components,
                ));
            }
        }

        if report.cross_encoder_consistency < genome_config.min_consistency {
            failures.push(format!(
                "Genome FAIL: {} consistency {:.2} below {:.2}",
                file_path.display(),
                report.cross_encoder_consistency,
                genome_config.min_consistency,
            ));
        }

        // Parsimony bloat is intentionally not a swarm-gate failure: only the
        // writer (integrator) can fix it, and the per-agent genome retry path
        // (build_genome_retry_prompt) already routes bloat fixes back to the
        // writer. Surfacing it here would fail the verifier/synthesizer roles
        // for an issue they have no way to address.

        if genome_config.require_no_regression {
            if let Some(prev_tier) = input.prev_tiers.get(file_path) {
                if report.tier < *prev_tier {
                    failures.push(format!(
                        "Genome FAIL: {} regressed from {} ({}) to {} ({})",
                        file_path.display(),
                        prev_tier.numeral(),
                        prev_tier.name(),
                        report.tier.numeral(),
                        report.tier.name(),
                    ));
                }
            }
        }

        for rec in &report.recommendations {
            if matches!(rec.severity, nit_core::RecommendationSeverity::Critical) {
                failures.push(format!("  Recommendation: {}", rec.message));
            }
        }

        out.push_str(&format!("--- {} ---\n", file_path.display()));
        out.push_str(&nit_core::format_genome_report(&report));
        if failures.is_empty() {
            out.push_str(&format!("  Result: PASS ({})\n\n", report.quality_level()));
            pass_count += 1;
        } else {
            out.push_str(&format!("  Result: FAIL ({})\n", report.quality_level()));
            for f in &failures {
                out.push_str(&format!("  {f}\n"));
            }
            out.push('\n');
            all_failures.extend(failures);
        }
    }

    // Summary.
    if file_count == 0 {
        out.push_str("Genome gate: SKIP (no files to evaluate)\n");
    } else if all_failures.is_empty() {
        out.push_str(&format!(
            "Genome gate: PASS ({pass_count}/{file_count} files passed)\n"
        ));
    } else {
        out.push_str(&format!(
            "Genome gate: FAIL ({pass_count}/{file_count} files passed, {} failures)\n",
            all_failures.len(),
        ));
    }
    out
}
