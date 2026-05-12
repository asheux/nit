use std::collections::HashMap;
use std::sync::mpsc;

use nit_core::{AppState, GenomeReportMap};

use super::{GenomeReviewPending, SwarmRun};

struct GenomeReviewInput {
    files_to_eval: Vec<std::path::PathBuf>,
    /// Baselines captured at turn start, keyed by file path. Used as the
    /// "before" side of the genome diff so the reviewer sees real change.
    baselines: GenomeReportMap,
}

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

// Empty channel value = worker had nothing to evaluate (no modified files).
// The poller skips dispatching the reviewer in that case.
fn spawn_genome_review_prompt(
    state: &AppState,
    mission_id: &str,
    mission_baselines: &GenomeReportMap,
) -> mpsc::Receiver<String> {
    let (tx, rx) = mpsc::channel();

    let files_to_eval = collect_files_to_eval(state, mission_id);

    // Use the mission-scoped snapshot (frozen at swarm start) as "before".
    // `state.genome_baselines` is per-turn and gets cleared/re-captured
    // between agents, so by the time the review runs it equals current
    // state and `compute_genome_diff` returns +0.00 for every encoder.
    let baselines: GenomeReportMap = files_to_eval
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

// Prefer the mission-scoped accumulator over per-turn `genome_turn_modified`
// so files from earlier turns of the same mission aren't lost when an agent
// runs multiple sequential tasks (each TurnStarted clears
// `genome_turn_modified[agent]`). Fall back to unioning `genome_turn_modified`
// for defence-in-depth if the mission key is somehow empty.
fn collect_files_to_eval(state: &AppState, mission_id: &str) -> Vec<std::path::PathBuf> {
    let mut files: Vec<std::path::PathBuf> = match state.genome_mission_modified.get(mission_id) {
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
        if !files.contains(&editor_path) {
            files.push(editor_path);
        }
    }
    files.sort();
    files
}

// Reads each modified file and computes a full genome report — expensive
// (tree-sitter + 3000-gen GoL + parsimony per file) — that previously
// blocked the main loop for "Genome Quality Review".
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
        let Ok(text) = std::fs::read_to_string(file_path) else {
            continue;
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

    // Inject the measurement-attribution primer before the review task so
    // the reviewer interprets tier deltas correctly — without this, a
    // reviewer comparing two reports can mistake AST-stable rendering
    // drift (shifted recommendation line numbers, comment-only edits) for
    // a real structural regression and recommend useless reverts.
    prompt.push('\n');
    prompt.push_str(nit_core::GENOME_AGENT_INSTRUCTIONS);
    prompt.push_str("\n\n");

    prompt.push_str(
        "Produce a structured review:\n\
         1. Which files improved in structural quality and which regressed\n\
         2. The most critical structural issues remaining\n\
         3. Specific refactoring recommendations for the worst-scoring files\n\
         4. Overall verdict: PASS (all files tier III+ Spaceship) or FAIL (any file below tier III)\n\
         5. Distance from Replicator (Tier V) — what would it take to reach elite status\n",
    );

    prompt
}

struct GenomeGateInput {
    config: nit_core::config::GenomeGateConfig,
    files_to_eval: Vec<std::path::PathBuf>,
    /// Previous genome reports for regression checks (file → tier).
    prev_tiers: HashMap<std::path::PathBuf, nit_core::GenomeTier>,
}

pub(super) fn spawn_genome_gate_eval(
    state: &AppState,
    mission_id: &str,
    mission_baselines: &GenomeReportMap,
) -> mpsc::Receiver<String> {
    let (tx, rx) = mpsc::channel();

    let files_to_eval = collect_files_to_eval(state, mission_id);

    // Fall back to current state when the mission snapshot is missing the
    // file (created during the swarm) — "new file not regressed" is the
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

fn min_tier_from_config(config: &nit_core::config::GenomeGateConfig) -> nit_core::GenomeTier {
    let generations = match config.min_tier {
        0 => 0,
        1 => 51,
        2 => 201,
        3 => 501,
        _ => 2001,
    };
    nit_core::GenomeTier::from_generations(generations)
}

fn collect_file_failures(
    file_path: &std::path::Path,
    report: &nit_core::GenomeReport,
    config: &nit_core::config::GenomeGateConfig,
    min_tier: nit_core::GenomeTier,
    prev_tier: Option<&nit_core::GenomeTier>,
) -> Vec<String> {
    let mut failures = Vec::new();

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
        let is_ast_encoder = matches!(
            score.encoder,
            nit_core::SeedEncoderId::TokenSpectrum
                | nit_core::SeedEncoderId::AstStructure
                | nit_core::SeedEncoderId::ComplexityField
        );
        if is_ast_encoder && score.density > config.max_density {
            failures.push(format!(
                "Genome FAIL: {} density {:.2} on {} exceeds {:.2}",
                file_path.display(),
                score.density,
                score.encoder.label(),
                config.max_density,
            ));
        }
    }

    if let Some(s) = report
        .encoder_scores
        .iter()
        .find(|s| s.encoder == nit_core::SeedEncoderId::AstStructure)
    {
        if s.components < config.min_components {
            failures.push(format!(
                "Genome FAIL: {} has {} components (min: {})",
                file_path.display(),
                s.components,
                config.min_components,
            ));
        }
    }

    if report.cross_encoder_consistency < config.min_consistency {
        failures.push(format!(
            "Genome FAIL: {} consistency {:.2} below {:.2}",
            file_path.display(),
            report.cross_encoder_consistency,
            config.min_consistency,
        ));
    }

    // Parsimony bloat is intentionally not a swarm-gate failure: only the
    // writer (integrator) can fix it, and the per-agent genome retry path
    // already routes bloat fixes back to the writer. Surfacing it here
    // would fail the verifier/synthesizer roles for an issue they have no
    // way to address.

    if config.require_no_regression {
        if let Some(prev) = prev_tier {
            if report.tier < *prev {
                failures.push(format!(
                    "Genome FAIL: {} regressed from {} ({}) to {} ({})",
                    file_path.display(),
                    prev.numeral(),
                    prev.name(),
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

    failures
}

fn append_summary(out: &mut String, file_count: u32, pass_count: u32, failure_count: usize) {
    if file_count == 0 {
        out.push_str("Genome gate: SKIP (no files to evaluate)\n");
    } else if failure_count == 0 {
        out.push_str(&format!(
            "Genome gate: PASS ({pass_count}/{file_count} files passed)\n"
        ));
    } else {
        out.push_str(&format!(
            "Genome gate: FAIL ({pass_count}/{file_count} files passed, {failure_count} failures)\n"
        ));
    }
}

fn evaluate_genome_gate_bg(input: &GenomeGateInput) -> String {
    let min_tier = min_tier_from_config(&input.config);
    let mut out = String::new();
    let mut total_failures = 0usize;
    let mut file_count = 0u32;
    let mut pass_count = 0u32;

    for file_path in &input.files_to_eval {
        let Ok(text) = std::fs::read_to_string(file_path) else {
            continue;
        };
        let report = nit_core::compute_genome_report(&text, file_path);
        file_count += 1;

        let prev_tier = input.prev_tiers.get(file_path);
        let failures =
            collect_file_failures(file_path, &report, &input.config, min_tier, prev_tier);

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
            total_failures += failures.len();
        }
    }

    append_summary(&mut out, file_count, pass_count, total_failures);
    out
}
