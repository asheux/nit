#![allow(clippy::too_many_arguments)]

use nit_core::{
    AgentAlert, AgentAlertSeverity, AgentChannel, AgentDiagnosticEvent, AgentMessage, AppState,
};

use crate::claude_runner::ClaudeRunner;
use crate::codex_runner::CodexRunner;
use crate::vitals::VitalsState;

use super::*;

/// Maximum automatic genome-improvement retries per agent turn. Kept low (3)
/// because further retries start incentivising over-engineering — if the
/// agent can't restore quality in three focused attempts, the parsimony
/// detector is likely to fire on the next one.
pub(super) const GENOME_RETRY_LIMIT: u8 = 3;

// `GENOME_RETRY_MIN_LINES = 120` is intentionally removed (was a duplicated,
// more conservative arbitrary threshold on top of the encoder's auto-pass
// at <20 significant lines). Two defensive layers already handle the
// "don't push agents to over-engineer trivial code" concern:
//   1. The encoder auto-passes files under ~20 sig lines to Tier III, so
//      genuinely tiny files never appear in the retry path as below-tier.
//   2. The parsimony detector fires on over-engineered diffs at the next
//      pass, surfacing the over-engineering with bloat-specific guidance.
// Operator-observed failure mode: agents split a 341-line file into
// ~85-100-line submodules at Tier II / parsimony-flagged, and slipped
// past retry because each submodule was one line shy of the 120 floor.
// The encoder + parsimony detector are now the sole gates for "is this
// file substantive enough to retry on", and they're authoritative.

pub(super) fn push_genome_retry_message(
    state: &mut AppState,
    agent_id: &str,
    attempt: u8,
    degraded_files: &[std::path::PathBuf],
) {
    // Identify the actual writer(s) of the degraded files. The agent whose
    // batch is firing may be an evaluator/reviewer rather than the writer
    // — users want the retry attributed to whoever produced the code.
    // Fall back to the batch agent when no writer can be resolved.
    let writer_for_path = |path: &std::path::Path| -> String {
        // Primary: per-turn attribution. `genome_turn_modified` is reset on
        // each TurnStarted, so it only covers recent writes — sufficient
        // for the retry message which fires shortly after a turn finalizes.
        for (aid, paths) in state.genome_turn_modified.iter() {
            if paths.contains(path) {
                return aid.clone();
            }
        }
        // Fallback: substrate claim lattice. ExclusiveWrite claims auto-
        // asserted by FileWrite carry the writer across turn boundaries
        // within the claim's TTL, covering cases where the per-turn map
        // was cleared by a subsequent TurnStarted before the retry message
        // was built. Pick the most recently claimed entry on the path.
        let latest_writer = state
            .substrate
            .claims
            .values()
            .filter(|c| {
                matches!(c.kind, nit_core::substrate::ClaimKind::ExclusiveWrite)
                    && matches!(
                        &c.target,
                        nit_core::substrate::ClaimTarget::File { path: p } if p == path
                    )
            })
            .max_by_key(|c| c.claimed_at_gen)
            .map(|c| c.claimed_by.clone());
        if let Some(aid) = latest_writer {
            return aid;
        }
        // Last resort: attribute to the batch owner (the reporter).
        agent_id.to_string()
    };

    let writers: std::collections::BTreeSet<String> = degraded_files
        .iter()
        .map(|p| writer_for_path(p.as_path()))
        .collect();
    let header_writer = if writers.is_empty() {
        agent_id.to_string()
    } else if writers.len() == 1 {
        writers
            .into_iter()
            .next()
            .unwrap_or_else(|| agent_id.to_string())
    } else {
        writers
            .iter()
            .map(|a| compact_agent_id_for_log(a))
            .collect::<Vec<_>>()
            .join(", ")
    };

    let mut lines = Vec::new();
    let reporter = compact_agent_id_for_log(agent_id);
    let writer_display = if header_writer.contains(',') {
        header_writer.clone()
    } else {
        compact_agent_id_for_log(&header_writer)
    };
    // Keep the header on a single line short enough to fit narrow chat panes.
    // When reporter differs from writer, split onto a second indented line
    // instead of extending the header with a parenthetical.
    lines.push(format!(
        "\u{21b3} [{writer_display}] genome retry {attempt}/{GENOME_RETRY_LIMIT}"
    ));
    if writer_display != reporter {
        lines.push(format!("    reported by {reporter}"));
    }

    for path in degraded_files {
        let report = match state.genome_reports.get(path) {
            Some(r) => r,
            None => continue,
        };
        let file_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("?");
        let delta = if let Some(base) = state.genome_baselines.get(path) {
            if report.tier > base.tier {
                "\u{2191}" // ↑
            } else if report.tier < base.tier {
                "\u{2193}" // ↓
            } else {
                "\u{2014}" // —
            }
        } else {
            "+"
        };
        let bloat_tag = if report.parsimony.bloat_detected {
            " [bloat]"
        } else {
            ""
        };
        let writer = writer_for_path(path.as_path());
        let writer_tag = if writer != agent_id {
            format!(" by {}", compact_agent_id_for_log(&writer))
        } else {
            String::new()
        };
        lines.push(format!(
            "  {delta} {file_name} {} {} c={:.2}{bloat_tag}{writer_tag}",
            report.tier.numeral(),
            report.quality_level(),
            report.cross_encoder_consistency,
        ));
    }

    let msg = lines.join("\n");

    let at = timestamp_label(state);
    state.agents.messages.push(nit_core::AgentMessage {
        at: at.clone(),
        channel: nit_core::AgentChannel::Broadcast,
        agent_id: Some(agent_id.to_string()),
        mission_id: None,
        text: msg,
        prompt_msg_idx: None,
        kind: Some("genome-retry".into()),
    });
    state.agents.console_scroll = nit_core::CONSOLE_SCROLL_BOTTOM;

    let any_bloat = degraded_files.iter().any(|p| {
        state
            .genome_reports
            .get(p)
            .is_some_and(|r| r.parsimony.bloat_detected)
    });
    let reason = if any_bloat {
        "parsimony bloat / degraded quality"
    } else {
        "quality degraded"
    };
    state
        .agents
        .diag_events
        .push(nit_core::AgentDiagnosticEvent {
            severity: nit_core::AgentAlertSeverity::Warn,
            source: "genome".into(),
            message: format!("[{agent_id}] {reason}, retry {attempt}/{GENOME_RETRY_LIMIT}"),
            at,
        });
    state.agents.note_event();
}

/// Drain pending claim-violation retries queued by `agent_bus` on FileWrite
/// conflict. Each request becomes a corrective prompt for the violating
/// agent, dispatched through the same pipe as genome retries. Shares
/// `GENOME_RETRY_LIMIT` / `state.genome_retry_count` as the budget.
pub(super) fn drain_pending_claim_retries(
    state: &mut AppState,
    vitals: &mut VitalsState,
    codex: &CodexRunner,
    claude: &ClaudeRunner,
) {
    while let Some(req) = state.pending_claim_retries.pop() {
        // Per-agent budget: only the violating agent's counter is checked
        // and incremented. Another agent's exhausted budget must not block
        // this agent's retry (the bug that made parallel swarms appear to
        // retry sequentially).
        let count = state
            .genome_retry_counts
            .get(&req.agent_id)
            .copied()
            .unwrap_or(0);
        if count >= GENOME_RETRY_LIMIT {
            continue;
        }
        let new_count = count.saturating_add(1);
        state
            .genome_retry_counts
            .insert(req.agent_id.clone(), new_count);
        state
            .agents
            .diag_events
            .push(nit_core::AgentDiagnosticEvent {
                severity: nit_core::AgentAlertSeverity::Warn,
                source: "substrate".into(),
                message: format!(
                    "[{}] claim-retry {new_count}/{GENOME_RETRY_LIMIT} (wrote {}, blocked by {})",
                    compact_agent_id_for_log(&req.agent_id),
                    req.path.display(),
                    compact_agent_id_for_log(&req.conflicting_holder),
                ),
                at: timestamp_label(state),
            });
        let prompt = format!(
            "CLAIM VIOLATION: you wrote to {} but {} holds an {} claim. Rationale: {}. Back off and coordinate — choose a different file or wait for the claim to expire.",
            req.path.display(),
            req.conflicting_holder,
            req.conflicting_kind,
            req.conflicting_rationale,
        );
        dispatch_agent_prompt(
            state,
            vitals,
            Some(codex),
            Some(claude),
            req.agent_id,
            None,
            prompt,
        );
    }
}

/// Drain `pending_interventions` by actuating each queued arbiter
/// intervention. Modeled on `drain_pending_claim_retries`:
/// - Shares `GENOME_RETRY_LIMIT` / `state.genome_retry_count` as the budget.
/// - `EmitSignalOnly` interventions are consumed without dispatch — the
///   signal was emitted by `apply_interventions` already.
/// - `RedispatchWithEscalatedPrompt` dispatches to the chosen recipient:
///   AgentPair -> `chosen_recipient` payload field (fallback: larger id);
///   Agent     -> that agent_id;
///   Mission / Global -> consumed without dispatch (no single recipient).
pub(super) fn drain_pending_interventions(
    state: &mut AppState,
    vitals: &mut VitalsState,
    codex: &CodexRunner,
    claude: &ClaudeRunner,
) {
    while let Some(iv) = state.pending_interventions.pop() {
        let prompt = match iv.kind {
            nit_core::InterventionKind::EmitSignalOnly => continue,
            nit_core::InterventionKind::RedispatchWithEscalatedPrompt { prompt } => prompt,
        };
        let recipient = match &iv.target {
            nit_core::InterventionTarget::Agent { agent_id } => Some(agent_id.clone()),
            nit_core::InterventionTarget::AgentPair { a, b } => iv
                .payload
                .get("chosen_recipient")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .or_else(|| Some(if a > b { a.clone() } else { b.clone() })),
            nit_core::InterventionTarget::Mission { .. } | nit_core::InterventionTarget::Global => {
                None
            }
        };
        let Some(agent_id) = recipient else {
            continue;
        };
        // Per-agent budget (parallel-safe): skip only this recipient's
        // overflowing intervention, not every pending intervention.
        let count = state
            .genome_retry_counts
            .get(&agent_id)
            .copied()
            .unwrap_or(0);
        if count >= GENOME_RETRY_LIMIT {
            continue;
        }
        let new_count = count.saturating_add(1);
        state
            .genome_retry_counts
            .insert(agent_id.clone(), new_count);
        state
            .agents
            .diag_events
            .push(nit_core::AgentDiagnosticEvent {
                severity: nit_core::AgentAlertSeverity::Warn,
                source: "arbiter".into(),
                message: format!(
                    "[{}] intervention retry {new_count}/{GENOME_RETRY_LIMIT} — {}",
                    compact_agent_id_for_log(&agent_id),
                    iv.rationale,
                ),
                at: timestamp_label(state),
            });
        dispatch_agent_prompt(
            state,
            vitals,
            Some(codex),
            Some(claude),
            agent_id,
            None,
            prompt,
        );
    }
}

// Forget baselines for files this agent touched, leaving other agents' baselines
// intact. Replaces the old global `genome_baselines.clear()` — a settling
// agent in a parallel swarm must not wipe the reference snapshot another
// still-running agent needs.
pub(super) fn clear_baselines_for_agent(state: &mut AppState, agent_id: &str) {
    let paths: Vec<std::path::PathBuf> = state
        .genome_turn_modified
        .get(agent_id)
        .map(|s| s.iter().cloned().collect())
        .unwrap_or_default();
    for path in paths {
        state.genome_baselines.remove(&path);
    }
}

pub(super) fn build_genome_retry_prompt(
    state: &mut AppState,
    agent_id: &str,
) -> Option<(String, Vec<std::path::PathBuf>)> {
    if !state.settings.genome.genome_context_enabled {
        return None;
    }

    let agent_files: Vec<std::path::PathBuf> = state
        .genome_turn_modified
        .get(agent_id)
        .map(|s| s.iter().cloned().collect())
        .unwrap_or_default();

    // Detect parsimony bloat across the agent's modified files. Bloat
    // triggers a retry even when overall quality didn't degrade — only the
    // writer can fix it, so we route the fix to the agent that wrote it.
    let any_bloat = agent_files.iter().any(|p| {
        state
            .genome_reports
            .get(p)
            .is_some_and(|r| r.parsimony.bloat_detected)
    });

    // Retry on degradation relative to baselines OR parsimony bloat.
    // Read per-agent delta (falls back to 0 when no batch has reported yet).
    let agent_delta = state
        .genome_quality_deltas
        .get(agent_id)
        .copied()
        .unwrap_or(0);
    if agent_delta >= 0 && !any_bloat {
        state.genome_retry_counts.remove(agent_id);
        state.genome_quality_deltas.remove(agent_id);
        clear_baselines_for_agent(state, agent_id);
        return None;
    }
    let agent_retry_count = state
        .genome_retry_counts
        .get(agent_id)
        .copied()
        .unwrap_or(0);
    if agent_retry_count >= GENOME_RETRY_LIMIT {
        clear_baselines_for_agent(state, agent_id);
        return None;
    }
    let attempt = agent_retry_count + 1;
    state
        .genome_retry_counts
        .insert(agent_id.to_string(), attempt);

    // Collect files that degraded relative to their baseline, tripped
    // parsimony bloat, OR new files below the agent's quality threshold.
    let mut degraded_files: Vec<std::path::PathBuf> = Vec::new();
    let mut bloated_files: HashSet<std::path::PathBuf> = HashSet::new();
    for file_path in &agent_files {
        let report = match state.genome_reports.get(file_path) {
            Some(r) => r,
            None => continue,
        };

        // Skip non-code files — docs, config, and data files should never
        // trigger genome retries.
        let ext = file_path.extension().and_then(|e| e.to_str()).unwrap_or("");
        let is_code = matches!(
            ext,
            "rs" | "ts" | "tsx" | "js" | "jsx" | "py" | "go" | "c" | "cpp" | "h" | "hpp"
        );
        if !is_code {
            continue;
        }

        // No file-size gate. The encoder's <20-sig-line auto-pass to
        // Tier III + the parsimony detector together cover "don't push
        // agents to over-engineer trivial code" — a separate retry-side
        // threshold (was 120 lines) duplicated that work and let agents
        // slip below-tier mission-authored files past retry just by
        // making them small enough. See the constant block above for the
        // operator-reported failure mode this removal closes.

        let mut should_include = false;

        // Parsimony bloat: the writer needs to consolidate over-split
        // functions, inline trivial predicates, and remove comment padding.
        // Track separately so the retry prompt can give targeted guidance.
        if report.parsimony.bloat_detected {
            bloated_files.insert(file_path.clone());
            should_include = true;
        }

        if let Some(base) = state.genome_baselines.get(file_path) {
            // Tier takes priority — a tier drop is always degradation.
            if report.tier < base.tier {
                should_include = true;
            } else if report.tier == base.tier {
                let gen_base: i32 = base
                    .encoder_scores
                    .iter()
                    .map(|s| s.generations_survived as i32)
                    .sum();
                let gen_now: i32 = report
                    .encoder_scores
                    .iter()
                    .map(|s| s.generations_survived as i32)
                    .sum();
                if gen_now < gen_base {
                    should_include = true;
                }
            }
        } else {
            // New file — include if below the *specific* agent's adaptive
            // quality threshold (not the global max across all agents).
            let min_tier = state
                .genome_agent_min_tier
                .get(agent_id)
                .copied()
                .unwrap_or(nit_core::GenomeTier::Spaceship);
            if report.tier < min_tier {
                should_include = true;
            }
        }

        if should_include {
            degraded_files.push(file_path.clone());
        }
    }
    if degraded_files.is_empty() {
        state.genome_retry_counts.remove(agent_id);
        state.genome_quality_deltas.remove(agent_id);
        clear_baselines_for_agent(state, agent_id);
        return None;
    }

    let mut prompt = String::new();

    prompt.push_str(
        "IMPORTANT: The scores below are the ACTUAL measured results from nit's genome \
         system after your changes were applied to disk. These are authoritative \u{2014} \
         disregard any scores you computed or estimated yourself.\n\n",
    );

    // Categorise files for the header summary.
    let new_below_threshold = degraded_files
        .iter()
        .filter(|p| !state.genome_baselines.contains_key(*p))
        .count();
    let bloated_count = bloated_files.len();
    let existing_degraded = degraded_files.len() - new_below_threshold;
    prompt.push_str(&format!(
        "Files requiring attention ({} of {} modified",
        degraded_files.len(),
        agent_files.len(),
    ));
    if new_below_threshold > 0 {
        prompt.push_str(&format!(
            "; {existing_degraded} degraded, {new_below_threshold} new below threshold",
        ));
    }
    if bloated_count > 0 {
        prompt.push_str(&format!("; {bloated_count} parsimony bloat"));
    }
    prompt.push_str("):\n\n");
    for file_path in &degraded_files {
        let report = match state.genome_reports.get(file_path) {
            Some(r) => r,
            None => continue,
        };
        let is_bloated = bloated_files.contains(file_path);
        prompt.push_str(&format!("--- {} ---\n", file_path.display()));
        prompt.push_str("[ACTUAL]\n");
        prompt.push_str(&nit_core::format_genome_report(report));

        if is_bloated {
            prompt.push_str(&format!(
                "[PARSIMONY BLOAT] tier capped at IV. {} fns, avg {:.1} lines/fn, \
                 {:.0}% tiny (<=5 lines), {:.0}% comments.\n\
                 \u{2192} Consolidate over-split functions, inline trivial predicates, \
                 remove comment padding. Do NOT add more structure.\n",
                report.parsimony.fn_count,
                report.parsimony.avg_fn_body_lines,
                report.parsimony.tiny_fn_fraction * 100.0,
                report.parsimony.comment_ratio * 100.0,
            ));
        }

        if let Some(base) = state.genome_baselines.get(file_path) {
            let gen_base: i32 = base
                .encoder_scores
                .iter()
                .map(|s| s.generations_survived as i32)
                .sum();
            let gen_now: i32 = report
                .encoder_scores
                .iter()
                .map(|s| s.generations_survived as i32)
                .sum();
            let delta_label = if report.tier < base.tier || gen_now < gen_base {
                "DEGRADED"
            } else if is_bloated {
                "BLOAT (tier capped)"
            } else {
                "UNCHANGED"
            };
            prompt.push_str(&format!(
                "[BASELINE] {} (tier {}, consistency {:.2}, total gen {})\n\
                 [DELTA]    {delta_label} ({:+} generations)\n",
                base.quality_level(),
                base.tier.numeral(),
                base.cross_encoder_consistency,
                gen_base,
                gen_now - gen_base,
            ));

            // Pinpoint: identify which encoder(s) degraded most.
            for (now_score, base_score) in
                report.encoder_scores.iter().zip(base.encoder_scores.iter())
            {
                let drop =
                    base_score.generations_survived as i64 - now_score.generations_survived as i64;
                if drop > 0 {
                    prompt.push_str(&format!(
                        "  ↓ {} dropped {} generations (was {}, now {})\n",
                        now_score.encoder.label(),
                        drop,
                        base_score.generations_survived,
                        now_score.generations_survived,
                    ));
                }
            }
        } else {
            // New file — no baseline, show threshold requirement using the
            // agent's adaptive min tier (not hardcoded).
            let min_t = state
                .genome_agent_min_tier
                .get(agent_id)
                .copied()
                .unwrap_or(nit_core::GenomeTier::Spaceship);
            prompt.push_str(&format!(
                "[NEW FILE] Below minimum quality threshold ({} {}).\n\
                 [ACTUAL]   {} (tier {}, consistency {:.2})\n\
                 [TARGET]   {} ({}) or higher required for new files.\n",
                min_t.numeral(),
                min_t.name(),
                report.quality_level(),
                report.tier.numeral(),
                report.cross_encoder_consistency,
                min_t.numeral(),
                min_t.name(),
            ));
        }

        // Pinpoint: include specific function-level recommendations.
        let critical_recs: Vec<_> = report
            .recommendations
            .iter()
            .filter(|r| {
                matches!(
                    r.severity,
                    nit_core::RecommendationSeverity::Critical
                        | nit_core::RecommendationSeverity::Warning
                )
            })
            .collect();
        if !critical_recs.is_empty() {
            prompt.push_str("[FIX THESE SPECIFIC ISSUES]\n");
            for rec in &critical_recs {
                let loc = rec
                    .location
                    .as_deref()
                    .map(|l| format!(" at {l}"))
                    .unwrap_or_default();
                prompt.push_str(&format!("  • {}{loc}\n", rec.message));
            }
        }
        prompt.push('\n');
    }

    // Quality goals.
    prompt.push_str(nit_core::GENOME_AGENT_INSTRUCTIONS);
    prompt.push_str("\n\n");

    // Retry instructions — use the agent's adaptive minimum tier, not a hardcoded value.
    let agent_min_tier = state
        .genome_agent_min_tier
        .get(agent_id)
        .copied()
        .unwrap_or(nit_core::GenomeTier::Spaceship);
    let tier_target = format!(
        "tier {} ({}, {}+ generations)",
        agent_min_tier.numeral(),
        agent_min_tier.name(),
        match agent_min_tier {
            nit_core::GenomeTier::StillLife => 0,
            nit_core::GenomeTier::Oscillator => 51,
            nit_core::GenomeTier::Spaceship => 201,
            nit_core::GenomeTier::Methuselah => 501,
            nit_core::GenomeTier::Replicator => 2001,
        }
    );
    let header = if bloated_count > 0 && existing_degraded == 0 && new_below_threshold == 0 {
        format!(
            "[PARSIMONY BLOAT DETECTED \u{2014} automatic retry {attempt}/{GENOME_RETRY_LIMIT}]"
        )
    } else if bloated_count > 0 {
        format!(
            "[GENOME QUALITY DEGRADED + PARSIMONY BLOAT \u{2014} automatic retry {attempt}/{GENOME_RETRY_LIMIT}]"
        )
    } else {
        format!("[GENOME QUALITY DEGRADED \u{2014} automatic retry {attempt}/{GENOME_RETRY_LIMIT}]")
    };
    prompt.push_str(&format!(
        "{header}\n\n\
         Only fix the files listed above \u{2014} do not touch files that maintained or \
         improved quality. The ACTUAL results were measured by nit AFTER your changes \
         were written to disk. Do NOT call [evaluate_genome]; nit measures automatically.\n\n\
         SCOPE CONSTRAINT: You may modify code that YOU added or changed during this \
         session. Do NOT refactor, rename, or rewrite unrelated pre-existing code. \
         Exception: if one of your edits perturbed a neighbouring function or block — \
         and that perturbation is the cause of the degradation — you may touch the \
         directly-affected surrounding context to restore or improve quality. Leave \
         everything else exactly as it was.\n\n\
         Second exception: if the operator's original prompt explicitly asked you to \
         refactor the entire file, you may do so.\n\n\
         Your goal is to IMPROVE structural quality on the files you changed.\n\n\
         {no_revert_clause}\n\n",
        no_revert_clause = crate::swarm::NO_REVERT_CLAUSE,
    ));
    if bloated_count > 0 {
        prompt.push_str(
            "PARSIMONY FIX (for files marked [PARSIMONY BLOAT]):\n\
             - Consolidate over-split functions: inline trivial predicates, merge \
             one-line wrappers into their callers, collapse stub functions.\n\
             - Remove comment padding: delete doc comments that restate the type or \
             function name and section markers added purely for token diversity. \
             Keep comments that explain WHY.\n\
             - Replace duplicated near-identical function bodies with macros, \
             generics, or shared helpers.\n\
             - Do NOT add more structure to fix bloat. The fix is REMOVAL.\n\n",
        );
    }
    prompt.push_str(&format!(
        "For files marked [DEGRADED], focus on natural quality improvements:\n\
         - Reduce cyclomatic complexity in functions you wrote (aim for <= 8)\n\
         - Flatten deep nesting with early returns and guard clauses\n\
         - Use descriptive, unique identifiers in code you added\n\
         - Aim for {tier_target} or higher on all AST-driven encoders\n\
         Do NOT over-engineer: splitting every function into tiny pieces, adding \
         unnecessary types, or padding with comments will trigger parsimony bloat \
         detection and cap your tier. Write naturally structured code.\n",
    ));

    Some((prompt, degraded_files))
}

pub(super) fn maybe_compute_genome_report(
    state: &mut AppState,
    genome: &crate::genome_worker::GenomeWorker,
) {
    let file_path = match state.editor_buffer().path().cloned() {
        Some(p) => p,
        None => return,
    };
    // Genome metrics are only meaningful for code. Skip markdown, config,
    // and plaintext so the editor buffer eval doesn't repopulate the cache
    // with files the workspace scan deliberately excluded.
    if !crate::workspace_scan::is_code_file(&file_path) {
        return;
    }
    if state.genome_reports.contains_key(&file_path) {
        return;
    }
    if state.genome_computing {
        return; // Already dispatched, waiting for result.
    }
    state.genome_computing = true;
    let text = state.editor_buffer().content_as_string();
    genome.evaluate(file_path, text, true);
}

// Append the genome landscape to shadow-agent dispatches (single-agent mode)
// that play the propose / judge / review role. Scope falls back to the
// currently-focused editor buffer path, since shadow has no declared
// `scope_files` like swarm missions do.
pub(super) fn augment_shadow_prompt_with_landscape(
    state: &AppState,
    dispatch: &mut crate::shadow::ShadowDispatch,
) {
    let Some((_, _, role)) = crate::shadow::parse_shadow_lane_id(&dispatch.agent_id) else {
        return;
    };
    // Map shadow roles to the landscape-framing roles used for swarm.
    let landscape_role = match role {
        "propose-a" | "propose-b" => "propose",
        "judge" => "judge",
        "review" => "integrate",
        _ => return,
    };
    let Some(path) = state.editor_buffer().path() else {
        return;
    };
    let rel = path
        .strip_prefix(&state.workspace_root)
        .unwrap_or(path)
        .to_string_lossy()
        .into_owned();
    let scope: Vec<String> = vec![rel];
    let Some(section) =
        crate::app::dispatch::build_propose_genome_landscape(state, &scope, Some(landscape_role))
    else {
        return;
    };
    dispatch.prompt.push_str(&section);
}

pub(super) fn dispatch_shadow_outcome(
    state: &mut AppState,
    vitals: &mut VitalsState,
    codex: &CodexRunner,
    claude: &ClaudeRunner,
    mut dispatch: crate::shadow::ShadowDispatch,
) {
    // Shadow (single-agent) mode mirrors the swarm propose/judge/review
    // landscape injection. Scope is the currently-focused editor buffer —
    // shadow has no declared scope_files like swarm missions do. Silent
    // no-op when the editor buffer has no path or no genome report exists.
    augment_shadow_prompt_with_landscape(state, &mut dispatch);

    let is_claude = state
        .agents
        .agents
        .iter()
        .find(|lane| lane.id == dispatch.agent_id)
        .is_some_and(|lane| lane.is_claude());

    // Bypass dispatch_agent_prompt when the target is busy: its enqueue path
    // drops prompt_msg_idx, which would misattribute the next completing turn
    // to the shadow prompt. Route through enqueue_*_turn directly so the idx
    // rides on the queue entry and is applied when the turn actually starts.
    if crate::swarm::is_agent_busy(state, &dispatch.agent_id) {
        if is_claude {
            enqueue_claude_turn(
                state,
                vitals,
                Some(dispatch.agent_id),
                dispatch.mission_id,
                dispatch.prompt,
                dispatch.prompt_msg_idx,
            );
        } else {
            enqueue_codex_turn(
                state,
                vitals,
                Some(dispatch.agent_id),
                dispatch.mission_id,
                dispatch.prompt,
                dispatch.prompt_msg_idx,
            );
        }
        return;
    }

    if let Some(idx) = dispatch.prompt_msg_idx {
        if is_claude {
            state
                .agents
                .claude_turn_prompt_idx
                .insert(dispatch.agent_id.clone(), idx);
        } else {
            state
                .agents
                .codex_turn_prompt_idx
                .insert(dispatch.agent_id.clone(), idx);
        }
    }
    dispatch_agent_prompt(
        state,
        vitals,
        Some(codex),
        Some(claude),
        dispatch.agent_id,
        dispatch.mission_id,
        dispatch.prompt,
    );
}

// Exclusion set for the shadow-eval fallback: every path another agent
// already owns, taken from both per-turn attributions and the substrate's
// live `ExclusiveWrite` claims. The fallback uses this to keep a parallel
// swarm's writers from inheriting each other's files when runner-side
// `FileWrite` attribution drops a turn's writes.
fn paths_attributed_to_other_agents(
    state: &AppState,
    agent_id: &str,
) -> HashSet<std::path::PathBuf> {
    let mut excluded: HashSet<std::path::PathBuf> = state
        .genome_turn_modified
        .iter()
        .filter(|(other, _)| other.as_str() != agent_id)
        .flat_map(|(_, paths)| paths.iter().cloned())
        .collect();
    for claim in state.substrate.claims.values() {
        if !matches!(claim.kind, nit_core::substrate::ClaimKind::ExclusiveWrite) {
            continue;
        }
        if claim.claimed_by == agent_id {
            continue;
        }
        if let nit_core::substrate::ClaimTarget::File { path } = &claim.target {
            excluded.insert(path.clone());
        }
    }
    excluded
}

/// Dispatch authoritative genome evaluations to background threads after a turn completes.
/// Each modified file is sent to the genome worker; results stream back to `drain_genome_results`.
pub(super) fn dispatch_turn_genome_evals(
    state: &mut AppState,
    genome: &crate::genome_worker::GenomeWorker,
    agent_id: &str,
    mission_id: &Option<String>,
) {
    if !state.settings.genome.genome_context_enabled {
        return;
    }

    // Use runner-attributed files: the runners emit FileWrite events when
    // they detect tool_use(edit/write) targeting a file. These are already
    // collected in genome_turn_modified[agent_id] by the FileWrite event handler.
    // Primary: runner-attributed files from FileWrite events.
    let mut modified: Vec<std::path::PathBuf> = state
        .genome_turn_modified
        .get(agent_id)
        .map(|s| s.iter().cloned().collect())
        .unwrap_or_default();

    // Fallback: if runner attribution found nothing (tool-format mismatch),
    // recover from shadow-eval-detected files. The file watcher saw changes
    // during the turn — shadow evals prove files were modified.
    //
    // `genome_shadow_evals` is GLOBAL (keyed by path, not agent_id). In a
    // parallel swarm we MUST exclude paths another agent already owns —
    // either via its own `genome_turn_modified` entry or via a live
    // `ExclusiveWrite` claim — otherwise this agent inherits another
    // writer's files and the retry routes to the wrong agent.
    if modified.is_empty() && !state.genome_shadow_evals.is_empty() {
        let excluded = paths_attributed_to_other_agents(state, agent_id);
        modified = state
            .genome_shadow_evals
            .keys()
            .filter(|p| !excluded.contains(*p))
            .cloned()
            .collect();
        if !modified.is_empty() {
            state
                .genome_turn_modified
                .insert(agent_id.to_string(), modified.iter().cloned().collect());
        }
    }

    // Genome metrics only apply to code. Filter docs/config out so agent
    // turns that touch markdown alongside code don't pollute the cache —
    // the retry prompt builder already skips non-code, so evaluating them
    // is pure waste.
    modified.retain(|path| crate::workspace_scan::is_code_file(path));

    if modified.is_empty() {
        return;
    }

    // Per-agent batch. Parallel swarm turns interleave, so one agent's
    // batch must not clobber another's — keep the slot keyed by agent_id.
    let batch = state
        .genome_eval_batches
        .entry(agent_id.to_string())
        .or_default();
    batch.pending = modified.len();
    batch.worst_delta = 0;
    batch.mission_id = mission_id.clone();

    for file_path in modified {
        // File read + genome computation happen on the worker thread —
        // no blocking I/O on the main thread.
        if !genome.evaluate_from_disk(file_path, agent_id.to_string()) {
            if let Some(b) = state.genome_eval_batches.get_mut(agent_id) {
                b.pending = b.pending.saturating_sub(1);
            }
        }
    }
}

/// Drain file-watcher events: reload buffers whose files changed on disk
/// and feed every change into the workspace-scan runtime for genome cache
/// invalidation. The scan driver handles scope checks (workspace root,
/// gitignore, IGNORED_DIRS) and enqueueing re-evals via `GenomeWorker`.
pub(super) fn drain_file_watcher(
    state: &mut AppState,
    syntax: &mut SyntaxRuntime,
    watcher: &FileWatcher,
    workspace_scan: &mut crate::workspace_scan::WorkspaceScanRuntime,
) {
    while let Ok(changed_path) = watcher.events.try_recv() {
        // NOTE: we do NOT attribute file changes to agents here.
        // The file watcher can't distinguish agent writes from external editor
        // writes. Agent file attribution is done at TurnCompleted using mtime
        // watermarks (see dispatch_turn_genome_evals).

        // Reload any open editor buffer that matches.
        for buf_idx in 0..state.buffers.len() {
            let matches = state.buffers[buf_idx]
                .path()
                .map(|p| *p == changed_path)
                .unwrap_or(false);
            if matches && state.buffers[buf_idx].reload_from_disk() {
                syntax.note_buffer_change(buf_idx, &mut state.buffers[buf_idx]);
            }
        }

        // Route every change through the workspace-scan runtime — unconditional.
        // Agent-turn guards were removed so external editor writes also keep
        // the cached genome landscape fresh.
        workspace_scan.note_change(state, changed_path);
    }
}

/// Drain genome worker results: update shadow evals, reports, and stage labels.
/// Also finalizes turn evaluations when all pending results arrive.
pub(super) fn drain_genome_results(
    state: &mut AppState,
    genome: &crate::genome_worker::GenomeWorker,
    workspace_scan: &mut crate::workspace_scan::WorkspaceScanRuntime,
    vitals: &mut crate::vitals::VitalsState,
    codex_runner: &crate::codex_runner::CodexRunner,
    claude_runner: &crate::claude_runner::ClaudeRunner,
) {
    while let Ok(result) = genome.rx.try_recv() {
        let path = result.path;
        let result_agent_id = result.agent_id.clone();
        let is_workspace_scan = result.workspace_scan;
        let report = match result.report {
            Some(r) => r,
            None => {
                // File could not be read (e.g. deleted). Still decrement
                // the owning agent's batch so turn finalization is not stuck.
                if !result.shadow && !result.save_eval && !is_workspace_scan {
                    if let Some(aid) = result_agent_id.as_deref() {
                        if let Some(batch) = state.genome_eval_batches.get_mut(aid) {
                            batch.pending = batch.pending.saturating_sub(1);
                        }
                    }
                }
                if is_workspace_scan {
                    // Keep `done` counter moving even on read failure so the
                    // progress indicator eventually clears.
                    workspace_scan.note_completed(&path);
                }
                continue;
            }
        };
        if is_workspace_scan {
            // Persist to disk on a background thread so the main loop is
            // never blocked on I/O. Cache stays consistent with state.
            {
                let ws = state.workspace_root.clone();
                let r = report.clone();
                std::thread::Builder::new()
                    .name("genome-persist".into())
                    .spawn(move || nit_core::agent_bus::persist_genome_report(&ws, &r))
                    .ok();
            }
            state.genome_reports.insert(path.clone(), report);
            workspace_scan.note_completed(&path);
            continue;
        }

        // Clear genome_computing flag when result arrives for the current editor buffer.
        if state.genome_computing && state.editor_buffer().path() == Some(&path) {
            state.genome_computing = false;
            state.gate_monitor_scroll = 0;
        }

        if result.save_eval {
            // Save-triggered evaluation — update report and show quality delta in status.
            let msg = if let Some(prev) = state.genome_reports.get(&path) {
                let diff = nit_core::genome_report::compute_genome_diff(prev, &report);
                let gen_before: i32 = prev
                    .encoder_scores
                    .iter()
                    .map(|s| s.generations_survived as i32)
                    .sum();
                let gen_after: i32 = report
                    .encoder_scores
                    .iter()
                    .map(|s| s.generations_survived as i32)
                    .sum();
                let d = gen_after - gen_before;
                if diff.tier_after > diff.tier_before {
                    state.genome_quality_delta = 1;
                    format!(
                        "Saved \u{2014} quality upgraded: {} \u{2192} {}",
                        diff.tier_before, diff.tier_after,
                    )
                } else if diff.tier_after < diff.tier_before {
                    state.genome_quality_delta = -1;
                    format!(
                        "Saved \u{2014} quality degraded: {} \u{2192} {}",
                        diff.tier_before, diff.tier_after,
                    )
                } else if d > 0 {
                    state.genome_quality_delta = 1;
                    format!("Saved \u{2014} quality improved (+{d} gen)")
                } else if d < 0 {
                    state.genome_quality_delta = -1;
                    format!("Saved \u{2014} quality declined ({d} gen)")
                } else {
                    state.genome_quality_delta = 0;
                    format!("Saved \u{2014} quality unchanged ({})", report.tier)
                }
            } else {
                state.genome_quality_delta = 0;
                format!("Saved \u{2014} genome: {}", report.tier)
            };
            state.genome_reports.insert(path, report);
            state.status = Some(msg);
            continue;
        }

        if result.shadow {
            // Shadow evaluation — update UI only.
            let is_new_file = !state.genome_baselines.contains_key(&path);
            let delta_label: &'static str = if let Some(base) = state.genome_baselines.get(&path) {
                // Tier comparison takes priority — tier reflects the weakest
                // encoder (bottleneck). Gen sum is only a tiebreaker when
                // tiers are equal.
                if report.tier > base.tier {
                    "improved"
                } else if report.tier < base.tier {
                    "degraded"
                } else {
                    let gen_base: i32 = base
                        .encoder_scores
                        .iter()
                        .map(|s| s.generations_survived as i32)
                        .sum();
                    let gen_now: i32 = report
                        .encoder_scores
                        .iter()
                        .map(|s| s.generations_survived as i32)
                        .sum();
                    if gen_now > gen_base {
                        "improved"
                    } else if gen_now < gen_base {
                        "degraded"
                    } else {
                        "unchanged"
                    }
                }
            } else {
                "new"
            };

            let file_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("?");
            let quality = report.quality_level();
            let tier_str = report.tier.numeral();
            let consistency = report.cross_encoder_consistency;

            let genome_stage = format!(
                "{file_name} {quality} {delta_label} (tier {tier_str}, c={consistency:.2})",
            );
            // Only update stage labels for agents that own this file.
            for (agent_id, turn) in state.agents.active_turns.iter_mut() {
                let owns_file = state
                    .genome_turn_modified
                    .get(agent_id)
                    .map(|files| files.contains(&path))
                    .unwrap_or(false);
                if owns_file {
                    turn.stage = Some(genome_stage.clone());
                    turn.last_output_at = Instant::now();
                }
            }

            state.genome_shadow_evals.insert(
                path.clone(),
                nit_core::GenomeShadowEval {
                    tier: report.tier,
                    quality,
                    consistency,
                    delta_label,
                    is_new_file,
                    at: Instant::now(),
                },
            );

            // Update genome_quality_delta from shadow eval so the display
            // and retry logic stay correct even if the authoritative async
            // eval is still in flight.
            let shadow_delta = match delta_label {
                "improved" => 1,
                "degraded" => -1,
                _ => 0,
            };
            if shadow_delta < state.genome_quality_delta {
                state.genome_quality_delta = shadow_delta;
            }

            state.genome_reports.insert(path, report);
        } else {
            // Authoritative turn evaluation — update reports, compute delta, persist.
            // Drive off the result's own `agent_id` so parallel turns can't
            // cross-contaminate each other's batch state.
            let Some(agent_id) = result_agent_id else {
                state.genome_reports.insert(path, report);
                continue;
            };

            let delta: i32 = if let Some(base) = state.genome_baselines.get(&path) {
                // Tier comparison takes priority — tier reflects the weakest
                // encoder. Gen sum is only a tiebreaker when tiers are equal.
                if report.tier > base.tier {
                    1
                } else if report.tier < base.tier {
                    -1
                } else {
                    let gen_base: i32 = base
                        .encoder_scores
                        .iter()
                        .map(|s| s.generations_survived as i32)
                        .sum();
                    let gen_now: i32 = report
                        .encoder_scores
                        .iter()
                        .map(|s| s.generations_survived as i32)
                        .sum();
                    if gen_now > gen_base {
                        1
                    } else if gen_now < gen_base {
                        -1
                    } else {
                        0
                    }
                }
            } else {
                // New file — evaluate against adaptive quality threshold.
                let min_tier = state
                    .genome_agent_min_tier
                    .get(&agent_id)
                    .copied()
                    .unwrap_or(nit_core::GenomeTier::Spaceship);
                if report.tier < min_tier {
                    -1
                } else {
                    0
                }
            };

            let batch_reached_zero = {
                let batch = state
                    .genome_eval_batches
                    .entry(agent_id.clone())
                    .or_default();
                if delta < batch.worst_delta {
                    batch.worst_delta = delta;
                }
                batch.pending = batch.pending.saturating_sub(1);
                batch.pending == 0
            };

            // Persist to disk on a background thread to avoid blocking the UI.
            {
                let ws = state.workspace_root.clone();
                let r = report.clone();
                std::thread::Builder::new()
                    .name("genome-persist".into())
                    .spawn(move || nit_core::agent_bus::persist_genome_report(&ws, &r))
                    .ok();
            }
            state.genome_reports.insert(path, report);

            if !batch_reached_zero {
                continue;
            }

            // This agent's batch just finished — finalize for THIS agent only.
            let (worst_delta, mission_id) = state
                .genome_eval_batches
                .remove(&agent_id)
                .map(|b| (b.worst_delta, b.mission_id))
                .unwrap_or((0, None));
            // Per-agent delta (parallel-safe); the scalar is also updated so
            // display surfaces still reflect the latest batch outcome.
            state
                .genome_quality_deltas
                .insert(agent_id.clone(), worst_delta);
            state.genome_quality_delta = worst_delta;

            // Adaptive quality thresholds: streak tracking.
            let current_min = state
                .genome_agent_min_tier
                .get(&agent_id)
                .copied()
                .unwrap_or(nit_core::GenomeTier::Spaceship);
            let modified: Vec<_> = state
                .genome_turn_modified
                .get(&agent_id)
                .map(|s| s.iter().cloned().collect())
                .unwrap_or_default();
            let worst_tier = modified
                .iter()
                .filter_map(|p| state.genome_reports.get(p))
                .map(|r| r.tier)
                .min()
                .unwrap_or(nit_core::GenomeTier::StillLife);
            if worst_tier >= current_min {
                let streak = state
                    .genome_agent_streak
                    .entry(agent_id.clone())
                    .or_insert(0);
                *streak = streak.saturating_add(1);
                if *streak >= 5 {
                    let next_tier = match current_min {
                        nit_core::GenomeTier::StillLife | nit_core::GenomeTier::Oscillator => {
                            nit_core::GenomeTier::Spaceship
                        }
                        nit_core::GenomeTier::Spaceship => nit_core::GenomeTier::Methuselah,
                        nit_core::GenomeTier::Methuselah => nit_core::GenomeTier::Replicator,
                        _ => current_min,
                    };
                    if next_tier > current_min {
                        state
                            .genome_agent_min_tier
                            .insert(agent_id.clone(), next_tier);
                        *streak = 0;
                    }
                }
            } else {
                state.genome_agent_streak.insert(agent_id.clone(), 0);
            }

            // Build diff text for all modified files.
            let mut all_diffs = String::new();
            for file_path in &modified {
                if let (Some(rpt), Some(base)) = (
                    state.genome_reports.get(file_path),
                    state.genome_baselines.get(file_path),
                ) {
                    let diff = nit_core::compute_genome_diff(base, rpt);
                    all_diffs.push_str(&nit_core::format_genome_diff(&diff));
                    all_diffs.push('\n');
                } else if let Some(rpt) = state.genome_reports.get(file_path) {
                    all_diffs.push_str(&format!(
                        "[new file] {} — {} (tier {}, c={:.2})\n",
                        file_path.display(),
                        rpt.quality_level(),
                        rpt.tier.numeral(),
                        rpt.cross_encoder_consistency,
                    ));
                }
            }
            state.last_genome_diff = if all_diffs.is_empty() {
                None
            } else {
                Some(all_diffs)
            };

            // Auto-retry on genome quality degradation. Each agent's batch
            // finalizes independently, so parallel agents each get their own
            // targeted retry — not one aggregated retry sent to whichever
            // agent happened to finalize last.
            if let Some((prompt, degraded_files)) = build_genome_retry_prompt(state, &agent_id) {
                let attempt = state
                    .genome_retry_counts
                    .get(&agent_id)
                    .copied()
                    .unwrap_or(0);
                push_genome_retry_message(state, &agent_id, attempt, &degraded_files);
                dispatch_agent_prompt(
                    state,
                    vitals,
                    Some(codex_runner),
                    Some(claude_runner),
                    agent_id,
                    mission_id,
                    prompt,
                );
            }
        }
    }
}
