use std::path::Path;

use nit_core::AppState;

use super::types::FollowupTaskSnapshot;
use super::{
    dashboard_gate_rows, enumerate_scope_files, is_cargo_workspace, normalize_role_label,
    run_gates_label, task_artifacts_summary_for_prompt, truncate_chars, FollowupContext,
    FollowupMessage, GateReport, SwarmMissionKind, SwarmRun, SwarmTask, SwarmTaskState,
    SwarmTemplate, COMPUTATIONAL_RESEARCH_ROLE, FINDINGS_RETRY_CLAUSE, NO_PADDING_CLAUSE,
    SWARM_VERIFY_MAX_CHARS, TEST_DISCIPLINE_CLAUSE,
};

/// Machine-checked sign-off sentinel. Swarm agents must emit this line exactly
/// once at the very end of a successfully completed task (after the structured
/// artifacts JSON block). Missing sentinel means the orchestrator treats the
/// output as incomplete and re-dispatches a continuation.
pub(super) const TASK_COMPLETE_SENTINEL: &str = "<SWARM_TASK_COMPLETE>";

/// Shared LENS definitions used by Lab and Parallel templates to make
/// multiple proposers diverge on a real optimisation axis. Each lens names
/// the axis it optimises for in one line so the judge / integrator can
/// weigh tradeoffs explicitly. Without distinct lenses, proposers produce
/// correlated output and the judge has nothing to choose between.
const PROPOSER_LENS_TABLE: &str = "\
LENS A: minimal-diff; LENS B: architectural coherence; LENS C: reversible stages; \
LENS D: measured performance and structural signals; LENS E: invariants and edge cases.\n";

/// Floor at which the parallel template upgrades from a single-recon shape
/// to multi-proposer + judge + integrators. Below this, integrators
/// directly consume the recon document; at or above, the extra lens
/// diversity is worth the proposer/judge budget. Operator-observed: lab
/// and bulk consistently produce higher-quality refactors than the
/// single-recon parallel shape, and the missing ingredient is lens
/// diversity feeding the writers.
const PARALLEL_MULTI_PROPOSER_FLOOR: usize = 3;

/// Returns `Some(reason)` when an agent's output looks like an early exit /
/// human-style "should I proceed?" stop rather than a real task completion.
/// Returns `None` when the output passes the sign-off check. Reasons are
/// short tags used in continuation prompts + system messages.
pub(super) fn detect_incomplete_signoff(message: &str) -> Option<&'static str> {
    let trimmed = message.trim();
    if trimmed.is_empty() {
        return Some("empty output");
    }
    if !trimmed.contains(TASK_COMPLETE_SENTINEL) {
        // Fallback heuristic: catch pre-sentinel deployments + agents that
        // strip it. Look for interrogatives in the tail of the output.
        let tail = tail_without_json_block(trimmed);
        if tail_contains_interactive_prose(tail) {
            return Some("asking for approval / offering options");
        }
        return Some("missing TASK_COMPLETE sentinel");
    }
    None
}

fn tail_without_json_block(text: &str) -> &str {
    if let Some(idx) = text.rfind("```") {
        if let Some(prev_fence) = text[..idx].rfind("```") {
            return text[..prev_fence].trim_end();
        }
    }
    text
}

fn tail_contains_interactive_prose(tail: &str) -> bool {
    // Only scan the last ~15 non-empty lines; early-exit prose lives at the
    // end, never at the start. Matching anywhere in a long output produces
    // false positives (proposers legitimately discuss "should we X" in prose).
    let tail_lines: Vec<&str> = tail
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect();
    let window_start = tail_lines.len().saturating_sub(15);
    let window = &tail_lines[window_start..];
    const NEEDLES: &[&str] = &[
        "shall i",
        "should i proceed",
        "should i continue",
        "want me to",
        "would you like me",
        "let me know",
        "pause here",
        "if you want me",
        "awaiting your",
        "awaiting approval",
        "ready for your review",
        "ready for review — shall",
    ];
    for line in window {
        let lower = line.to_ascii_lowercase();
        for needle in NEEDLES {
            if lower.contains(needle) {
                return true;
            }
        }
        // Trailing question mark on the very last non-empty line is a strong
        // signal (integrators don't ask rhetorical questions).
        if Some(*line) == window.last().copied() && line.ends_with('?') {
            return true;
        }
    }
    false
}

#[allow(clippy::too_many_arguments)]
pub(super) fn build_planner_prompt(
    root_prompt: &str,
    template: SwarmTemplate,
    mission_kind: SwarmMissionKind,
    planner_agent_id: &str,
    agent_ids: &[String],
    integrator_agent_id: Option<&str>,
    role_hints: &[(String, String)],
    priority_agent_ids: &[String],
    workspace_root: &Path,
    memory_hits: &[nit_core::MissionHit],
    prior_context: Option<&FollowupContext>,
    recent_messages: &[FollowupMessage],
) -> String {
    let intent = super::intent::detect_intent(root_prompt);
    build_planner_prompt_with_intent(
        root_prompt,
        template,
        mission_kind,
        planner_agent_id,
        agent_ids,
        integrator_agent_id,
        role_hints,
        priority_agent_ids,
        workspace_root,
        memory_hits,
        prior_context,
        recent_messages,
        &intent,
    )
}

#[allow(clippy::too_many_arguments)]
fn build_planner_prompt_with_intent(
    root_prompt: &str,
    template: SwarmTemplate,
    mission_kind: SwarmMissionKind,
    planner_agent_id: &str,
    agent_ids: &[String],
    integrator_agent_id: Option<&str>,
    role_hints: &[(String, String)],
    priority_agent_ids: &[String],
    workspace_root: &Path,
    memory_hits: &[nit_core::MissionHit],
    prior_context: Option<&FollowupContext>,
    recent_messages: &[FollowupMessage],
    intent: &super::intent::OperatorIntent,
) -> String {
    let available = agent_ids
        .iter()
        .filter(|id| id.as_str() != planner_agent_id)
        .cloned()
        .collect::<Vec<_>>();
    let scope_files = enumerate_scope_files(workspace_root, root_prompt);
    let large_scope = scope_files.len() > 15;
    let mut out = String::new();
    append_planner_header(&mut out, template, mission_kind, integrator_agent_id);
    append_planner_constraints(
        &mut out,
        template,
        mission_kind,
        integrator_agent_id,
        &available,
        role_hints,
        priority_agent_ids,
        &scope_files,
        large_scope,
    );
    append_template_specific_constraints(&mut out, template, mission_kind, intent, agent_ids.len());
    out.push_str(
        "- When the operator request involves refactoring or modifying a module/directory, the plan MUST cover ALL files in that scope. Assign a recon or propose task to survey the full directory tree first, and ensure the integrate task prompt lists every affected file.\n",
    );
    out.push_str(
        "- Each task prompt should be specific about which files or areas to focus on, not generic. The more concrete the prompt, the better the agent output.\n",
    );
    append_planner_validator_invariants(&mut out, template, mission_kind);
    append_planner_output_format(&mut out, template);
    append_planner_scope_section(&mut out, &scope_files);
    append_planner_memory_hits(&mut out, memory_hits, workspace_root);
    append_followup_context_prelude(&mut out, prior_context, recent_messages);

    out.push_str("\nOperator request:\n");
    out.push_str(root_prompt.trim());
    out.push('\n');
    out
}

fn append_planner_header(
    out: &mut String,
    template: SwarmTemplate,
    mission_kind: SwarmMissionKind,
    integrator_agent_id: Option<&str>,
) {
    out.push_str(
        "You are the SWARM PLANNER inside nit. Plan tasks under the supplied constraints. NitSkill swarm-plan covers dependency design and proposal lenses.\n\n",
    );
    out.push_str(&format!("Template: `{}`\n\n", template.label()));
    out.push_str(&format!("Mission focus: `{}`\n\n", mission_kind.label()));
    // Parallel template's runtime allows multi-writer dispatch (the
    // single-writer queue is removed for it). Saying "Single-writer
    // integrator: only this agent may do writes" would contradict the
    // runtime and force the planner to under-utilise the template — so
    // for parallel we mark the agent as "primary" and explicitly permit
    // additional integrate tasks. Lab and Bulk are convergence templates
    // and keep the single-writer invariant.
    match (template, integrator_agent_id) {
        (SwarmTemplate::Parallel, Some(integrator_agent_id))
            if matches!(mission_kind, SwarmMissionKind::General) =>
        {
            out.push_str(&format!(
                "Primary integrator: `{integrator_agent_id}` (additional `integrate` tasks may be assigned to other agents when the work splits naturally — e.g., topical subareas covered by different proposers. The runtime allows multi-writer dispatch under the parallel template.).\n\n"
            ));
        }
        (SwarmTemplate::Parallel, Some(integrator_agent_id)) => {
            // Research / computational-research: the researchers are the writers;
            // the single `integrate` task writes only the master index.
            out.push_str(&format!(
                "Index integrator: `{integrator_agent_id}` (research mission — the researchers write their own findings files; this single `integrate` task writes the master index over them).\n\n"
            ));
        }
        (SwarmTemplate::Lab | SwarmTemplate::Bulk, Some(integrator_agent_id)) => {
            out.push_str(&format!(
                "Single-writer integrator: `{integrator_agent_id}` (only this agent may do workspace writes, and only this agent may receive the `integrate` role).\n\n"
            ));
        }
        (SwarmTemplate::Lab | SwarmTemplate::Bulk, None) => {
            out.push_str("Single-writer integrator: (none)\n\n");
        }
        (SwarmTemplate::Parallel, None) => {}
    }
}

#[allow(clippy::too_many_arguments)]
fn append_planner_constraints(
    out: &mut String,
    template: SwarmTemplate,
    mission_kind: SwarmMissionKind,
    integrator_agent_id: Option<&str>,
    available: &[String],
    role_hints: &[(String, String)],
    priority_agent_ids: &[String],
    scope_files: &[String],
    large_scope: bool,
) {
    out.push_str("Constraints:\n");
    out.push_str("- Only assign tasks to these agent ids:\n");
    for id in available.iter() {
        out.push_str(&format!("  - {id}\n"));
    }
    if matches!(template, SwarmTemplate::Parallel | SwarmTemplate::Bulk) && !role_hints.is_empty() {
        out.push_str("- Agent role hints (from roster; 'all' means no constraint):\n");
        for (id, role) in role_hints.iter() {
            out.push_str(&format!("  - {id}: {role}\n"));
        }
        out.push_str(
            "- REQUIRED: when an agent has a specific role hint (anything other than `all`), you MUST assign that agent a task with the matching `role`. These hints reflect the swarm's deliberate role coverage — propose/recon, review/test, and integrate lanes are reserved this way to keep the swarm balanced. Do not reassign or ignore them. Agents with `all` are unconstrained and can take any role you find useful.\n",
        );
    }
    out.push_str(
        "- Role guide: use `research` for web/paper/resource exploration and idea discovery; use `computational-research` for tool-assisted or quantitative research, experiments, and evidence gathering.\n",
    );
    out.push_str(
        "- Reserve `research`/`computational-research` for topic investigation and strategy discovery, not routine codebase recon, unless the operator explicitly wants outside research.\n",
    );
    out.push_str(
        "- `computational-research` is the broad computation-heavy lane: simulations, modeling, numerical methods, optimization, data/model fitting, pattern or network analysis, reproducibility, and research-computing workflows across technical domains.\n",
    );
    out.push_str(
        "- If you assign `research` or `computational-research`, ensure the task output asks for sources, methods, assumptions, and ranked strategy recommendations.\n",
    );
    append_mission_kind_lines(out, mission_kind);
    // Bulk converges through one integrator; parallel allows multi-writer
    // dispatch (the runtime even removes the single-writer queue for
    // parallel). So `integrate` is singleton only for Bulk; for Parallel
    // the planner is free to fan out integrate across agents when work
    // splits topically.
    match template {
        SwarmTemplate::Bulk => out.push_str(
            "- Treat `judge` and `integrate` as singleton roles: assign at most one task for each role unless the operator explicitly asks for duplicates.\n",
        ),
        SwarmTemplate::Parallel if matches!(mission_kind, SwarmMissionKind::General) => out
            .push_str(
                "- Treat `judge` as a singleton role. The `integrate` role MAY be split across multiple tasks when the work splits naturally — e.g., one integrate task per topical subarea covered by a different proposer. The runtime allows multi-writer dispatch under parallel; use it when topical coherence beats alphabetical sharding. If you keep one integrate task, the runtime will auto-shard it for large scopes.\n",
            ),
        SwarmTemplate::Parallel => out.push_str(
            "- Research mission: the `integrate` role is the SINGLE master-index writer (research producers write their own files). Two `role=judge` tasks are expected here (map + reconcile); do not split `integrate`.\n",
        ),
        SwarmTemplate::Lab => {}
    }
    append_integrator_constraints(
        out,
        template,
        mission_kind,
        integrator_agent_id,
        scope_files,
        large_scope,
    );
    if matches!(template, SwarmTemplate::Parallel | SwarmTemplate::Bulk)
        && !priority_agent_ids.is_empty()
    {
        out.push_str("- Priority agents (from roster):\n");
        for id in priority_agent_ids.iter() {
            out.push_str(&format!("  - {id}\n"));
        }
        out.push_str(
            "- When multiple assignments are viable, prefer priority agents for the most critical/high-impact work.\n",
        );
    }
}

fn append_mission_kind_lines(out: &mut String, mission_kind: SwarmMissionKind) {
    match mission_kind {
        SwarmMissionKind::General => out.push_str(
            "- This mission is not research-oriented, so avoid `research` / `computational-research` roles unless the operator explicitly changes the mission focus.\n",
        ),
        SwarmMissionKind::Research => {
            out.push_str(
                "- This is a research mission: prefer a workflow like source survey -> evidence comparison -> synthesis / ranked strategy recommendation.\n",
            );
            out.push_str(
                "- `research` is the primary mission-specific role here; only use `computational-research` if the mission clearly needs simulations, modeling, or quantitative analysis.\n",
            );
            out.push_str(
                "- Producer roles WRITE the output: research missions ALWAYS persist findings to files. On the PARALLEL template the researchers write their own topic files (`writes=true`); on BULK the single `integrate` writes the consolidated output. Do not just answer in chat without producing the file(s).\n",
            );
            out.push_str(
                "- OUTPUT FORMAT: if the operator named a file or extension (e.g. `Links.nb`, `notes.md`), use exactly that. Otherwise match the project's obvious convention — a Wolfram/Mathematica project writes `.nb`, a docs/markdown repo writes `.md` — and when there's no clear convention, default to Markdown (`.md`).\n",
            );
        }
        SwarmMissionKind::ComputationalResearch => {
            out.push_str(
                "- This is a computational-research mission: prefer a workflow like source survey -> modeling / experiments / analysis -> synthesis / ranked strategy recommendation.\n",
            );
            out.push_str(
                "- `computational-research` is the DEFAULT producer role for this mission (both the survey lenses and the writers); reserve `research` for at most one pure literature/prior-art lens, and `research` lenses MUST NOT outnumber `computational-research` lenses.\n",
            );
            out.push_str(
                "- Producer roles WRITE the output: research missions ALWAYS persist findings to files. On the PARALLEL template the researchers write their own topic files (`writes=true`); on BULK the single `integrate` writes the consolidated output. Do not just answer in chat without producing the file(s).\n",
            );
            out.push_str(
                "- OUTPUT FORMAT: if the operator named a file or extension (e.g. `Links.nb`, `notes.md`), use exactly that. Otherwise match the project's obvious convention — a Wolfram/Mathematica project writes `.nb`, a docs/markdown repo writes `.md` — and when there's no clear convention, default to Markdown (`.md`).\n",
            );
        }
    }
}

fn append_integrator_constraints(
    out: &mut String,
    template: SwarmTemplate,
    mission_kind: SwarmMissionKind,
    integrator_agent_id: Option<&str>,
    _scope_files: &[String],
    _large_scope: bool,
) {
    let Some(integrator_agent_id) = integrator_agent_id else {
        return;
    };
    // Lab/Bulk converge through one writer — say so explicitly. Parallel
    // permits multi-writer; the planner picks how many integrate tasks to
    // emit based on topical fit.
    match template {
        SwarmTemplate::Lab | SwarmTemplate::Bulk => {
            out.push_str(&format!(
                "- If code changes are needed, assign `writes=true` and `role=integrate` only to `{integrator_agent_id}`.\n"
            ));
        }
        SwarmTemplate::Parallel if matches!(mission_kind, SwarmMissionKind::General) => {
            out.push_str(&format!(
                "- Code changes require `writes=true` and `role=integrate`. The PRIMARY integrator is `{integrator_agent_id}`; you MAY assign additional `integrate` tasks to other agents when work splits naturally (each with `writes=true`). Each integrate task's `task_prompt` MUST clearly state which files it owns.\n"
            ));
        }
        SwarmTemplate::Parallel => {
            // Research / computational-research: single index integrator (the
            // researchers write their own files).
            out.push_str(&format!(
                "- The `integrate` role is EXACTLY ONE task, assigned to `{integrator_agent_id}`, writing only the master index over the research producers' own files. Do not fan out `integrate`.\n"
            ));
        }
    }
    if !matches!(mission_kind, SwarmMissionKind::General) {
        return;
    }
    match template {
        SwarmTemplate::Lab | SwarmTemplate::Bulk => {
            out.push_str(&format!(
                "- REQUIRED: for code-change, refactoring, or implementation requests you MUST include exactly one task with `role=integrate` and `writes=true` assigned to `{integrator_agent_id}`. Without an integrate task, no workspace edits will be made and the swarm will produce no changes.\n"
            ));
        }
        SwarmTemplate::Parallel => {
            out.push_str(&format!(
                "- REQUIRED: for code-change, refactoring, or implementation requests you MUST include AT LEAST ONE task with `role=integrate` and `writes=true`. The primary integrator is `{integrator_agent_id}`; additional integrate tasks may go to other agents. Without any integrate task, no workspace edits will be made and the swarm will produce no changes.\n"
            ));
        }
    }
}

// The research / computational-research + parallel pipeline: survey lenses →
// judge-A (map) → researcher-writers → judge-B (reconcile) → one index
// integrator → review. The researchers ARE the writers, two judges are
// expected (the only shape that allows >1 judge), and research always persists
// findings to files. Mirrors the validator's research-aware invariants.
fn append_research_parallel_pipeline(out: &mut String, mission_kind: SwarmMissionKind) {
    let (default_role, role_note) = match mission_kind {
        SwarmMissionKind::ComputationalResearch => (
            "computational-research",
            "Use `computational-research` as the DEFAULT producer role for both the survey lenses and the writers; at most ONE lens may be `role=research` for a pure literature/prior-art survey, and `research` lenses must not outnumber `computational-research` lenses.",
        ),
        _ => (
            "research",
            "Use `role=research` for the lenses and writers; reserve `computational-research` only for lenses that are genuinely quantitative / tool-driven.",
        ),
    };
    out.push_str(
        "OPERATOR INTENT: research mission — the RESEARCHERS are the writers (each writes its own findings file), with TWO judges (map + reconcile) and ONE master index. Research missions ALWAYS persist findings to files, even when the operator only asked for a summary.\n",
    );
    out.push_str(&format!(
        "- RESEARCH PIPELINE (parallel) — MUST produce EXACTLY this DAG, in order:\n  \
         (1) SURVEY LENSES — 2-4 `role={default_role}` tasks, READ-ONLY (`writes=false`), empty `deps`, each surveying the SAME scope through a distinct LENS (use the lens table below or invent a sharper axis). {role_note}\n  \
         (2) JUDGE-A (MAP) — exactly ONE `role=judge` task whose `deps` are ALL the survey lenses. It dedups the survey and assigns each topic/source to exactly ONE output file → a per-file content map (this is what stops the writers from overlapping).\n  \
         (3) WRITERS — one `role={default_role}` task per output file, `writes=true`, `deps`=[judge-A]. Each writes ONLY its assigned file per the map. The producers ARE the writers (never a separate integrator fleet), and writers MUST NOT outnumber the survey lenses. Name each task's owned file in its `prompt` and `swarm_artifacts.files`.\n  \
         (4) JUDGE-B (RECONCILE) — exactly ONE MORE `role=judge` task whose `deps` are ALL the writer tasks. It reads the written files, fixes cross-file duplication / coverage gaps / conflicts, and specs the master index.\n  \
         (5) INDEX — exactly ONE `role=integrate` task, `writes=true`, `deps`=[judge-B], writing ONLY the master index file (the entry-point the operator named) that links every writer's file.\n  \
         (6) REVIEW — one `role=review` task, `deps`=[index], validating syntax / full coverage / no cross-file duplication / index correctness.\n",
    ));
    out.push_str(
        "- TWO JUDGES ARE EXPECTED here (judge-A map + judge-B reconcile) — this is the ONLY shape where more than one `role=judge` is allowed; do not collapse them, and do not emit a separate `propose` lane.\n",
    );
    out.push_str(
        "- DISTINCT FILES: every writer (each producer + the index) owns a DISTINCT file so they never race.\n",
    );
    out.push_str("- Lens framings for the survey lenses (step 1):\n");
    out.push_str(PROPOSER_LENS_TABLE);
}

fn append_template_specific_constraints(
    out: &mut String,
    template: SwarmTemplate,
    mission_kind: SwarmMissionKind,
    intent: &super::intent::OperatorIntent,
    agent_count: usize,
) {
    match template {
        SwarmTemplate::Parallel => {
            // Research / computational-research take the dedicated two-judge
            // writers-and-index pipeline, NOT the general multi-writer fanout.
            if !matches!(mission_kind, SwarmMissionKind::General) {
                append_research_parallel_pipeline(out, mission_kind);
                return;
            }
            // Compute the fanout floor up front so the planner sees the
            // exact integer it MUST hit, not just "split the work".
            // This number is the same one the validator's
            // INV-17 parallel_min_integrators enforces. When operator
            // intent exceeds capacity we explicitly switch the planner
            // into bundling mode rather than asking for unbounded
            // fanout.
            let writer_budget = agent_count.saturating_sub(1).max(1);
            let floor = super::validator::parallel_integrate_floor(intent, writer_budget);
            let over_capacity =
                super::validator::parallel_intent_exceeds_capacity(intent, writer_budget);
            let intent_block = match intent.ticket_count {
                Some(n) if intent.structured_list && over_capacity => format!(
                    "OPERATOR INTENT: {n} distinct tickets detected — exceeds capacity ({floor} writer slots). Plan in BUNDLE MODE: group related tickets into {floor} integrators by shared file scope / module / domain. Each integrator's `task_prompt` MUST quote ALL tickets in its bundle and list the union of their files.\n",
                ),
                Some(n) if intent.structured_list => format!(
                    "OPERATOR INTENT: {n} distinct tickets detected in the prompt (bullets / numbered items / `T<n>.` headers / `Files:` blocks). Capacity is sufficient ({writer_budget} writer slots) — one integrator per ticket.\n",
                ),
                _ => String::from(
                    "OPERATOR INTENT: no explicit ticket count detected; prompt may be a single deliverable or unstructured prose.\n",
                ),
            };
            out.push_str(&intent_block);
            out.push_str(&format!(
                "- PARALLEL FANOUT — MUST: produce AT LEAST {floor} `role=integrate` tasks (each `writes=true`). Under-fanned plans (`integrate_count < {floor}`) are auto-rejected by the post-parse validator (INV-17 parallel_min_integrators). Upper bound: {floor} (the writer budget caps fanout — beyond this the marginal integrator costs more than it saves).\n",
            ));
            if over_capacity {
                out.push_str(
                    "- BUNDLING — MUST: tickets exceed writer capacity. Group related tickets into bundles such that integrator_count = floor, and put the bundling rationale in each integrator's `task_prompt` (e.g. \"this integrator owns tickets T13 + T14 + T17 because they all touch editor_view.rs\"). Group by: shared file scope first, shared module second, shared semantic domain last. Avoid splitting a single ticket across integrators (the rebases will collide).\n",
                );
            } else {
                out.push_str(
                    "- ONE TICKET PER INTEGRATOR — MUST: do NOT consolidate multiple tickets into a single integrator. If the operator asked for N tickets, produce N integrators, each with `task_prompt` quoting its specific ticket text and listing the files that ticket owns. Consolidation is the most common parallel-template plan failure.\n",
                );
            }
            out.push_str(
                "- DISTINCT SCOPE: each integrate task's `task_prompt` MUST clearly state which files / which ticket(s) it owns. Two integrators with overlapping scope is a plan failure (the integrators race on the same files).\n",
            );
            // Switch the proposer shape on fanout. Small parallel runs (1-2
            // integrators) don't justify the lens-divergence budget — one
            // recon document is fine. At PARALLEL_MULTI_PROPOSER_FLOOR
            // (currently 3) and above, the upstream lens-diversity work
            // pays off in the integrators' output quality. Operator
            // observation: lab/bulk consistently beat single-recon parallel
            // on the same prompt because the integrators get a
            // judge-synthesised view of multiple lenses, not one recon's
            // single perspective.
            if floor >= PARALLEL_MULTI_PROPOSER_FLOOR {
                out.push_str(
                    "- LENS PROPOSERS — MUST: produce 2-3 `role=propose` tasks (max 3) running in parallel before the integrators. Each proposer surveys the SAME scope but commits to a distinct LENS framing so the judge synthesises across orthogonal axes instead of three samples of the same view. Proposers are read-only; their `deps` MUST be empty. Use these lenses (pick the 2-3 that fit the scope; invent a sharper axis if none of the canned ones apply):\n",
                );
                out.push_str(PROPOSER_LENS_TABLE);
                out.push_str(
                    "  Bake the lens name AND a one-line restatement of its axis into the first paragraph of each proposer's `task_prompt`. The judge and integrators need the axis explicit so they can weigh tradeoffs.\n",
                );
                out.push_str(
                    "- JUDGE LANE — MUST: produce exactly ONE `role=judge` task. Its `deps` MUST include EVERY propose task id. The judge synthesises the proposers into a unified per-area recommendation (which lens wins where, what's the consolidated file map, what conflicts surfaced). Without a judge, integrators consume raw multi-lens output and have to merge it themselves — exactly the failure mode this shape is designed to avoid.\n",
                );
                out.push_str(
                    "- INTEGRATOR DEPS (parallel multi-proposer): every `role=integrate` task's `deps` MUST be `[judge]` (and ONLY judge — do not also depend on the proposers; the judge already covers them). Integrators consume the judge's synthesis, not the raw proposals.\n",
                );
            } else {
                // Small-fanout shape: preserved verbatim for 1-2 integrator
                // runs where lens divergence is overkill.
                out.push_str(
                    "- RECON LANE: reserve ONE non-integrate lane for a `propose` (or `research` / `recon`) task that surveys the target scope first. The integrators depend on it (so they share a consistent view of the codebase) but the integrators themselves run in parallel after recon completes.\n",
                );
            }
            out.push_str(
                "- Prefer tasks that can run in parallel (deps should usually be empty), except where the recon / propose / judge lanes feed the integrate tasks.\n",
            );
            out.push_str(
                "- If you assign producer/consumer-style roles (e.g. research or computational-research → judge), use deps to express required ordering.\n",
            );
            out.push_str(
                "- VERIFIER ORDERING (parallel): every `test` and `review` task MUST set `deps` to ALL `integrate` task ids. Verifiers cannot run before writers have produced output — a `test` task with empty deps will fire alongside the proposers and report nothing-to-test. The runtime auto-repairs missing deps as a safety net, but plans should set them explicitly so the dependency intent is visible in the DAG.\n",
            );
            out.push_str(
                "- JUDGE ORDERING (parallel): every `judge` task MUST set `deps` to ALL `propose` / `research` task ids it's evaluating. A judge with empty deps would fire concurrently with the proposers and have no output to compare.\n",
            );
        }
        SwarmTemplate::Lab => {
            out.push_str(
                "- You MAY assign multiple tasks to the same agent id (they run sequentially).\n",
            );
            out.push_str("- Use deps to express ordering (DAG). Avoid cycles.\n");
            out.push_str("- Only the integrator agent may have `writes=true` tasks.\n");
            out.push_str(
                "- Use read-only proposal/review tasks for codebase work. Use a `research` role task (one or more, parallel) whenever the best approach for a ticket is non-obvious and benefits from comparing prior art, surveying related implementations in other editors/libraries, or surfacing competing patterns — research clones are read-only and feed their findings into the proposer/integrator dependencies. Reserve `computational-research` for missions that need simulations/numerical work.\n",
            );
            out.push_str(
                "- PROPOSER PARALLELISM (lab): if you assign multiple propose tasks, they MUST have empty `deps` and run concurrently. Do NOT chain them (propose-02 depending on propose-01 etc.) — proposers are independent investigators, not a pipeline. A judge task then fans in via `deps = [propose-01, propose-02, ...]` and waits for all of them in parallel, not serially. Sequential proposers just waste wall-clock time; the judge has to wait for the last one regardless.\n",
            );
            out.push_str(
                "- PROPOSER LENSES (lab): if you assign multiple propose tasks, each one's `task_prompt` MUST open with a distinct LENS framing so the proposers diverge on a real optimisation axis instead of producing three samples of the same prompt. Without distinct lenses, correlated output gives the judge nothing to choose between. Use one of these framings per proposer (pick the ones that fit the request, or invent a sharper axis for the specific scope):\n",
            );
            out.push_str(PROPOSER_LENS_TABLE);
            out.push_str(
                "Bake the lens name AND a one-line restatement of its axis into the first paragraph of the task's `task_prompt`. The judge and integrator need to see the axis the proposer was optimising for so they can weigh tradeoffs.\n",
            );
        }
        SwarmTemplate::Bulk => {
            if !matches!(mission_kind, SwarmMissionKind::General) {
                let default_role =
                    if matches!(mission_kind, SwarmMissionKind::ComputationalResearch) {
                        "computational-research"
                    } else {
                        "research"
                    };
                out.push_str(&format!(
                    "- RESEARCH (bulk) — the producer lenses are `role={default_role}` (read-only survey, one distinct lens each), NOT `propose`; use ids like `survey-01`, `survey-02`. They converge through ONE `role=judge` (`deps` = ALL lenses) and ONE `role=integrate` (`writes=true`, `deps`=[judge]) that writes the consolidated output the operator asked for; then a `role=review` (`deps`=[integrate]). SINGLE WRITER — bulk converges to one integrator, and research missions ALWAYS produce the output file. Follow the bulk rules below, reading each `propose` as your research lens.\n",
                ));
            }
            out.push_str(
                "- Bulk orchestration: explore multiple solution candidates in parallel, then converge.\n",
            );
            out.push_str(
                "- Prefer ONE proposer task per agent id (except the integrator), each with a distinct lens.\n",
            );
            out.push_str(
                "- Use ids `propose-01`, `propose-02`, ... plus `judge` and `integrate` so the workflow is easy to track.\n",
            );
            out.push_str(
                "- OPTIONAL `research` lane: when the request involves non-obvious design choices that benefit from prior-art comparison (other editors / libraries / RFCs / docs), prepend one or more `research` tasks (e.g. `research-01`, `research-02`) with empty `deps`. Make the proposers' `deps` include the research task ids so each proposer can build on the surfaced findings instead of duplicating the survey. Research clones are read-only and feed the proposer/integrator chain.\n",
            );
            out.push_str(
                "- Create a judge task that depends on ALL proposer tasks and selects the best approach.\n",
            );
            out.push_str(
                "- Create an integrator task assigned to the integrator agent with `writes=true`, depending on the judge.\n",
            );
            out.push_str("- Use deps to express ordering (DAG). Avoid cycles.\n");
            out.push_str("- Only the integrator agent may have `writes=true` tasks.\n");
        }
    }
}

// Pulls the validator's MustFix invariant list directly so the planner sees
// the same rules the post-parse check enforces. Single source of truth:
// editing `validator::planner_invariants_for_prompt` updates both the prompt
// and the check.
fn append_planner_validator_invariants(
    out: &mut String,
    template: SwarmTemplate,
    mission_kind: SwarmMissionKind,
) {
    let lines = super::validator::planner_invariants_for_prompt(template, mission_kind);
    if lines.is_empty() {
        return;
    }
    out.push_str(
        "\nDeterministic plan invariants (your output is auto-checked; failing plans are repaired and re-dispatched):\n",
    );
    for line in lines {
        out.push_str(&format!("- {line}\n"));
    }
}

fn append_planner_output_format(out: &mut String, template: SwarmTemplate) {
    out.push_str("\nOutput format:\n");
    out.push_str("1) 3-6 bullets summarizing the plan.\n");
    out.push_str("2) A JSON plan in a ```json code block with this schema (v2):\n");
    out.push_str("{\n");
    out.push_str("  \"version\": 2,\n");
    out.push_str(&format!("  \"template\": \"{}\",\n", template.label()));
    out.push_str("  \"integrator_agent_id\": \"(optional)\",\n");
    out.push_str("  \"tasks\": [\n");
    out.push_str("    {\n");
    out.push_str("      \"id\": \"task-id\",\n");
    out.push_str("      \"agent_id\": \"one-of-the-listed-agent-ids\",\n");
    out.push_str("      \"role\": \"(optional: propose|judge|research|computational-research|integrate|review|test)\",\n");
    out.push_str("      \"title\": \"short title\",\n");
    out.push_str("      \"prompt\": \"task instructions\",\n");
    out.push_str("      \"deps\": [\"task-id\"],\n");
    out.push_str("      \"writes\": false,\n");
    out.push_str(
        "      \"artifacts\": [\"(optional keys: files, diffs, commands, risks, notes)\"],\n",
    );
    out.push_str("      \"done_when\": \"(optional completion contract)\"\n");
    out.push_str("    }\n");
    out.push_str("  ],\n");
    out.push_str(
        "  \"synthesis_prompt\": \"(optional extra guidance for the final synthesis step)\"\n",
    );
    out.push_str("}\n");
}

fn append_planner_scope_section(out: &mut String, scope_files: &[String]) {
    if scope_files.is_empty() {
        return;
    }
    out.push_str("\nScope — files in the referenced module/directory (");
    out.push_str(&format!("{} files):\n", scope_files.len()));
    for path in scope_files.iter() {
        out.push_str(&format!("  - {path}\n"));
    }
    out.push_str("\nSCOPE RULES:\n");
    out.push_str("- \"Refactor module\" means refactor EVERY file listed above. No file may remain unchanged.\n");
    out.push_str("- Each integrate task prompt MUST embed the exact file paths it is responsible for as a numbered checklist, e.g.:\n");
    out.push_str("  \"Refactor the following files. Open each file, read it, and apply improvements. Check off each file as you go:\\n1. <path/to/first/file>\\n2. <path/to/second/file>\\n...\"\n");
    out.push_str("- Distribute ALL files across integrate tasks so every file is assigned to exactly one task.\n");
    out.push_str("- If there is one integrate task, it must list all files. If there are multiple, split them into disjoint subsets.\n");
}

fn append_planner_memory_hits(
    out: &mut String,
    memory_hits: &[nit_core::MissionHit],
    workspace_root: &Path,
) {
    if memory_hits.is_empty() {
        return;
    }
    out.push_str(
        "\nPrior similar missions (read-only context — do not re-plan these, use as precedent):\n",
    );
    for hit in memory_hits.iter() {
        let m = &hit.mission;
        out.push_str(&format!(
            "- {} [{}, {}]: {}\n",
            m.mission_id, m.template, m.status, m.title
        ));
        for s in m.task_summaries.iter().take(3) {
            out.push_str(&format!("    * {}\n", truncate_chars(s, 180)));
        }
        if m.files_touched.is_empty() {
            continue;
        }
        // Filter out paths that no longer exist in the current spawn
        // workspace. Without this, a polluted `.nit/swarm` index (e.g.
        // mission memory carried over from a prior, different workspace)
        // bleeds nit-internal paths into the planner — which then echoes
        // them into integrate task_prompts. The self-reinforcing leak
        // loop the operator hit on dotbox.
        let preview: Vec<&String> = m
            .files_touched
            .iter()
            .filter(|p| workspace_root.join(p).exists())
            .take(5)
            .collect();
        if !preview.is_empty() {
            out.push_str(&format!(
                "    files: {}\n",
                preview
                    .iter()
                    .map(|s| s.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
    }
}

pub(super) const FOLLOWUP_RECENT_MESSAGES_CAP: usize = 20;
const FOLLOWUP_SYNTHESIS_CHARS: usize = 800;
const FOLLOWUP_SCOPE_DISPLAY_CAP: usize = 40;
const FOLLOWUP_MESSAGE_BODY_CHARS: usize = 240;

fn append_followup_context_prelude(
    out: &mut String,
    prior_context: Option<&FollowupContext>,
    recent_messages: &[FollowupMessage],
) {
    let Some(ctx) = prior_context else {
        return;
    };
    out.push_str("\n## PREVIOUS RUN (follow-up context — read before planning)\n");

    let failed = ctx.report_status.as_deref().is_some_and(|s| {
        s.eq_ignore_ascii_case("FAILED")
            || s.eq_ignore_ascii_case("ABORTED")
            || s.to_uppercase().contains("FAIL")
    });
    if failed {
        out.push_str("\n⚠ Prior run did NOT succeed (status=");
        out.push_str(ctx.report_status.as_deref().unwrap_or("unknown"));
        out.push_str("). Treat artifacts below as PROPOSED work, not landed work.\n");
    }
    if let Some(prior) = ctx.prior_mission_kind {
        out.push_str(&format!(
            "Note: follow-up switched mission kind from `{}` to `{}`.\n",
            prior.label(),
            ctx.mission_kind.label(),
        ));
    }

    out.push_str(&format!(
        "- Mission kind: `{}`\n- Template: `{}`\n- Gate selection: `{}`\n",
        ctx.mission_kind.label(),
        ctx.template.label(),
        ctx.gate_selection,
    ));
    if let Some(status) = ctx.report_status.as_deref() {
        out.push_str(&format!("- Final status: `{status}`\n"));
    }

    if let Some(gate_report) = ctx.gate_report.as_ref() {
        append_followup_gate_results(out, gate_report);
    }
    if let Some(report) = ctx.report_output.as_deref() {
        out.push_str("\n### Prior synthesis (excerpt)\n");
        out.push_str(truncate_chars(report.trim(), FOLLOWUP_SYNTHESIS_CHARS).trim());
        out.push('\n');
    }
    if !ctx.tasks.is_empty() {
        append_followup_task_artifacts(out, &ctx.tasks);
    }
    if !ctx.scope_files.is_empty() {
        append_followup_scope_files(out, &ctx.scope_files);
    }
    if !recent_messages.is_empty() {
        append_followup_messages(out, recent_messages);
    }
}

fn append_followup_gate_results(out: &mut String, gate_report: &GateReport) {
    out.push_str("\n### Prior gate results (red lines first)\n");
    let mut gates: Vec<_> = gate_report.gates.iter().collect();
    gates.sort_by_key(|g| g.ui_status() == "PASS");
    for gate in gates {
        out.push_str(&format!("- {}: {}\n", gate.name, gate.ui_status()));
    }
}

fn append_followup_task_artifacts(out: &mut String, tasks: &[FollowupTaskSnapshot]) {
    out.push_str("\n### Prior task artifacts\n");
    for task in tasks {
        let role = task.role.as_deref().unwrap_or("(no role)");
        let headline = if task.headline.is_empty() {
            "(no notes)"
        } else {
            task.headline.as_str()
        };
        out.push_str(&format!(
            "- `{}` — {} ({} / {}): {}\n",
            task.id, task.title, role, task.agent_id, headline
        ));
    }
}

fn append_followup_scope_files(out: &mut String, scope_files: &[String]) {
    out.push_str(&format!(
        "\n### Prior scope (skip re-scan — already known: {} files)\n",
        scope_files.len(),
    ));
    for path in scope_files.iter().take(FOLLOWUP_SCOPE_DISPLAY_CAP) {
        out.push_str(&format!("  - {path}\n"));
    }
    if scope_files.len() > FOLLOWUP_SCOPE_DISPLAY_CAP {
        out.push_str(&format!(
            "  - … and {} more\n",
            scope_files.len() - FOLLOWUP_SCOPE_DISPLAY_CAP
        ));
    }
}

fn append_followup_messages(out: &mut String, messages: &[FollowupMessage]) {
    out.push_str(&format!(
        "\n### Recent conversation trace (most recent {} of ≤{} messages, oldest first)\n",
        messages.len(),
        FOLLOWUP_RECENT_MESSAGES_CAP,
    ));
    for msg in messages {
        let who = msg.agent_id.as_deref().unwrap_or("operator");
        let kind = msg
            .kind
            .as_deref()
            .map(|k| format!(" [{k}]"))
            .unwrap_or_default();
        let text = truncate_chars(msg.text.trim(), FOLLOWUP_MESSAGE_BODY_CHARS);
        out.push_str(&format!("- {who}{kind}: {text}\n"));
    }
}

pub(crate) fn role_contract_lines(role: &str) -> &'static [&'static str] {
    match role {
        "propose" => &[
            "Advance one concrete candidate from your assigned lens. Name files, commands and risks. Build on dependency outputs. Do not judge or implement.",
            "ROLE DISCIPLINE: Read-only. Do NOT run tests, builds, type-checkers, lints, formatters or other verification commands. Suggest commands as text.",
            "GENOME-AWARE PROPOSAL: When a GENOME LANDSCAPE is attached, cite file, current metric, target and direction for every recommendation. Prioritize breached files, bloat, zero entropy and cross-encoder spread >0.3.",
            "RECOMMENDATION COVERAGE: Scan the whole landscape for structural splits, repetition, complexity (≤8), AST fan-out (≥5), identifier uniqueness (≥65%), comment ratio and parsimony.",
            "MANDATORY STRUCTURAL SPLITS: For every file >2000 lines, tier I/II or density ≤0.10, name new module paths and assign functions/types. Specify consolidation for bloat and deduplication for zero entropy. Give every THRESHOLDS BREACHED file a structural action.",
            "FILES ARRAY = LOAD-BEARING HANDOFF: List every existing edit and new path in swarm_artifacts.files for the structural-compliance check. Files omitted can be skipped. Do NOT include files where you concluded \"audit only / no changes needed\"; use notes to avoid false-positive re-dispatch. See NitSkill swarm-propose for examples.",
        ],
        "research" => &[
            "Compare papers, docs, web resources and upstream findings. Hand off the best strategy candidates with evidence. See NitSkill research-evidence.",
            "Read-only survey. Do NOT run tests, builds, lints, formatters or CI. Cite upstream work and leave implementation and final selection to downstream roles.",
        ],
        COMPUTATIONAL_RESEARCH_ROLE => &[
            "Use simulations, modeling, numerical methods, optimization or other reproducible research workflows to compare strategies. Report commands, assumptions, sources and measurements. See NitSkill research-experiment.",
            "Read-only analysis. Do NOT run project tests, builds, lints or CI. Use tools only for the assigned computation and build on upstream evidence.",
        ],
        "judge" => &[
            "Compare the dependency outputs and choose the best path. State the decision, acceptance criteria and recommended verification steps.",
            "ROLE DISCIPLINE: Text analysis only. Do not edit. Do NOT run tests, builds, lints, formatters or any verification. Do not repeat upstream investigation.",
            "DECISION AXES: Rank every proposal on correctness, landscape fit, parsimony, blast radius, realistic failure handling and useful novelty. Penalize unnecessary defenses and artificial variety. Identify overlooked risks where proposals agree. Where they differ, name the axis and cite a reason. Avoid position bias.",
            "LANDSCAPE-AWARE JUDGING: With a landscape or threshold report, cite metrics and favor fixes to weak or high-leverage files. Downgrade cosmetic fixes that neglect them. Prefer diverse AST components (≥5) and token roles, and complexity ≤8. Flag monoliths and repetition.",
            "FILES ARRAY = SCOPE HINT: VERDICT PROSE binds the writer; swarm_artifacts.files is advisory. List expected edits, including adjacent wiring. The runtime no longer auto-re-dispatches for missed hints unless NIT_STRICT_CHECKLIST=1; stub and incomplete-split checks still apply. See NitSkill swarm-judge for worked examples.",
        ],
        "integrate" => &[
            "Implement the selected plan as sole writer. The judge’s verdict prose is authoritative. Without a judge, follow the proposer. Preserve approved paths, identifiers, constants, architecture and ordering. Do not restart ideation.",
            "FILE CHECKLIST: Open and read each listed file. Edit when required by the verdict. Report unmodified hints under `audit-only` with a reason. Missing-file retries require NIT_STRICT_CHECKLIST=1.",
            "DEFERRAL = TASK FAILURE: Deliver the whole accepted plan, not just its non-structural portion. The orchestrator detects the gap. Deviate only for a direct conflict with the operator request or genuine technical impossibility. Attempt risky changes in compiling steps. For a blocker, leave progress on disk, name completed and remaining files and record the exact failure per file in swarm_artifacts.risks.",
            "STUB FILES = TASK FAILURE: The runtime snapshots every declared file's line count and flags newly created declared files under 20 lines or performative splits. Move real code, shrink the large source by ≥30%, repair imports and verify each step.",
            "Complete end-to-end wiring outside the checklist when needed. List each under `out-of-scope edits` with a reason. Do not revive rejected changes, add unrelated features or refactor incidental code. Report exact files and validation results.",
            "GENOME QUALITY OBLIGATION: Cosmetic edits that don't move the underlying structural metrics cannot satisfy the plan. Target cyclomatic complexity, AST component fan-out, identifier uniqueness, comment ratio and cohesion. Aim for III+, aspire to V; nit evaluates automatically. Do not call evaluate_genome. See NitSkill swarm-integrate for examples.",
            TEST_DISCIPLINE_CLAUSE,
            NO_PADDING_CLAUSE,
        ],
        "review" => &[
            "Review correctness, UX, maintainability, regressions and missing tests. Report findings without editing. See NitSkill swarm-review.",
            TEST_DISCIPLINE_CLAUSE,
            "For genome findings name the affected encoder, metric and concrete refactor. Targets are in NitSkill genome-encoders.",
            FINDINGS_RETRY_CLAUSE,
        ],
        "test" => &[
            "Verify expected results and edge cases. Separate confirmed outcomes from unrun commands. Report failures without editing; suggest design changes only when a failure requires one. See NitSkill swarm-test.",
            TEST_DISCIPLINE_CLAUSE,
            FINDINGS_RETRY_CLAUSE,
        ],
        "genome-reviewer" => &[
            "Evaluate the structural quality of code changes using the genome reports provided.",
            "For each modified file, compare before/after genome metrics and identify regressions.",
            "Produce a structured review: which files improved, which regressed, critical issues, and specific refactoring recommendations.",
            "Overall verdict: PASS (all files tier III+ Spaceship) or FAIL (any file below tier III). Aspiration is tier V (Replicator).",
            "Do not edit the workspace; report findings as text only.",
            "ROLE DISCIPLINE: Genome metrics only — do NOT run tests, builds, lints, or any verification commands. The genome reports above are your sole input.",
        ],
        _ => &[
            "Stay within the assigned task scope.",
            "Do not silently switch into a different swarm role.",
            "UNKNOWN ROLE: the orchestrator didn't recognise your role name, so the strict role contract didn't apply. Default to read-only behaviour: do NOT edit the workspace, do NOT run tests/builds/lints/formatters/CI commands, and do NOT run workspace-wide commands under any circumstance. Produce text output only. If your task actually needs write access or verification, that's a plan bug — surface it in your reply so the operator can fix the role assignment.",
        ],
    }
}

fn role_response_format_lines(role: &str) -> Option<&'static [&'static str]> {
    match role {
        "research" | COMPUTATIONAL_RESEARCH_ROLE => Some(&[
            "Sources: list the key papers, docs, web resources, or datasets you relied on.",
            "Methods: explain how you searched, compared, computed, simulated, or evaluated the topic.",
            "Assumptions: call out the main assumptions, uncertainties, and missing information.",
            "Ranked strategies: provide the best options in ranked order with brief rationale and tradeoffs.",
        ]),
        _ => None,
    }
}

/// Detect a provider 429 rate-limit failure from the TurnFailed message.
/// Claude CLI surfaces these as "api_error_status:429" alongside prose like
/// "You've hit your limit · resets ...". Codex uses similar wording. When
/// the quota is exhausted, retrying in-window just burns the task retry
/// budget on calls that will fail immediately — so the swarm should stop.
pub(crate) fn is_provider_rate_limit_failure(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    lower.contains("api_error_status\":429")
        || lower.contains("api_error_status: 429")
        || lower.contains("status: 429")
        || lower.contains("status 429")
        || lower.contains("you've hit your limit")
        || lower.contains("rate limit")
        || lower.contains("rate-limited")
        || lower.contains("rate_limit")
        || lower.contains("429 too many requests")
}

/// Narrower variant for matching CLI-emitted quota-exhaustion banners that
/// appear in **successful** turn output (i.e. the subprocess exited 0 and
/// the result text contains the limit notice instead of real work). The
/// looser substrings used by `is_provider_rate_limit_failure` ("rate limit",
/// "rate_limit") would false-positive on assistant prose discussing rate
/// limiting as a topic — those are safe inside a known-failure message but
/// not against arbitrary completion text. Match only the distinctive
/// CLI-emitted phrases.
pub(crate) fn is_provider_quota_exhausted_in_result(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    (lower.contains("you've hit your limit") && lower.contains("resets"))
        || lower.contains("api_error_status\":429")
        || lower.contains("api_error_status: 429")
        || lower.contains("429 too many requests")
}

/// Extract cargo crate names from `crates/<name>/...` paths. Returns a sorted
/// de-duplicated list. Non-crates paths are ignored. Used to scope test/review
/// agents to only the packages the swarm actually touched.
fn crate_names_from_paths(paths: &[String]) -> Vec<String> {
    use std::collections::BTreeSet;
    let mut set: BTreeSet<String> = BTreeSet::new();
    for p in paths {
        let p = p.trim();
        let p = p.strip_prefix("./").unwrap_or(p);
        let Some(rest) = p.strip_prefix("crates/") else {
            continue;
        };
        let Some(name) = rest.split('/').next() else {
            continue;
        };
        if !name.is_empty() {
            set.insert(name.to_string());
        }
    }
    set.into_iter().collect()
}

#[allow(clippy::too_many_arguments)]
pub(super) fn wrap_task_prompt(
    root_prompt: &str,
    mission_kind: SwarmMissionKind,
    task: &SwarmTask,
    deps: Option<&[(String, String)]>,
    scope_files: &[String],
    spawn_cwd: &Path,
    shard_files: Option<&[String]>,
    proposers_skipped: bool,
) -> String {
    let mut out = String::new();
    append_task_continuation_preamble(&mut out, task);
    append_task_header(&mut out, task);
    append_task_execution_mode(&mut out, task);
    append_task_mission_contract(&mut out, mission_kind);
    append_task_role_contract(&mut out, task);
    append_task_done_and_artifacts(&mut out, task);

    out.push_str("\nOperator request:\n");
    out.push_str(root_prompt.trim());
    out.push('\n');

    // Inject the scope file list BEFORE the task prompt so the agent sees
    // the full file checklist first, then the task instructions. Prevents
    // the agent from forming a plan that ignores files. Sharded integrate
    // tasks skip the FILE CHECKLIST — the YOUR SHARD section that follows
    // already lists the partition; rendering both would just bloat the
    // prompt without adding signal.
    let role_kind = task.role.as_deref().and_then(normalize_role_label);
    let is_sharded_integrate =
        role_kind.as_deref() == Some("integrate") && task.shard_index.is_some();
    if !is_sharded_integrate {
        append_task_scope_section(&mut out, task, scope_files, spawn_cwd);
    }
    append_task_shard_section(&mut out, task, shard_files);

    out.push_str("\nYour task:\n");
    out.push_str(task.task_prompt.trim());
    out.push('\n');

    append_task_dependency_outputs(&mut out, task, deps, proposers_skipped);
    append_task_structured_artifacts(&mut out, task);

    out.push_str("\nRespond with:\n- Findings / recommendations\n- Concrete file paths / commands where relevant\n");

    // Embed genome quality instructions so every role sees the measurement
    // system, regardless of whether genome context is also injected at
    // dispatch time.
    out.push('\n');
    out.push_str(nit_core::GENOME_AGENT_INSTRUCTIONS);
    out.push('\n');

    // Machine-checked sign-off. Orchestrator scans for it on TurnCompleted
    // and auto-retries tasks that omit it. Kept verbatim — do not
    // paraphrase, quote, or wrap in backticks.
    out.push_str(&format!(
        "\n## SIGN-OFF (REQUIRED)\n\
         After the structured artifacts JSON block above, on its own final line, emit the literal sentinel:\n\
         {TASK_COMPLETE_SENTINEL}\n\
         Do not paraphrase it, do not wrap it in code fences, do not comment on it. Its presence is how the orchestrator knows you are done. Its absence triggers an automatic continuation turn with your partial output.\n"
    ));

    out
}

pub(super) fn append_task_continuation_preamble(out: &mut String, task: &SwarmTask) {
    if task.retries == 0 {
        return;
    }
    let structural_gap = !task.compliance_missing_files.is_empty();
    if structural_gap {
        out.push_str(&format!(
            "## CONTINUATION (attempt {}) — STRUCTURAL COMPLIANCE FAILURE\n\
             Your previous turn finished but did NOT modify these blueprint files (declared by the proposers/judge as in-scope):\n",
            task.retries + 1
        ));
        for file in task.compliance_missing_files.iter() {
            out.push_str(&format!("- {file}\n"));
        }
        out.push_str(
            "\n- Deferring any of these files as out-of-scope is a TASK FAILURE, not a graceful stopping point.\n\
             - Open each listed file and apply the blueprint's edits. If the blueprint marks a file `audit only` / `no changes`, you may skip it but say so explicitly per file in your final summary.\n\
             - If a specific file genuinely cannot be modified (compile error blocks the change, missing dep, etc.), emit a per-file blocker entry in your structured artifacts JSON `risks` field with the exact file path and the technical reason — do not silently skip.\n\
             - Do not re-plan, do not summarize what you already did, do not ask what to do next. Just finish the listed files.\n",
        );
        out.push_str(&format!(
            "- End your response with the {TASK_COMPLETE_SENTINEL} sentinel (see SIGN-OFF at the bottom).\n",
        ));
    } else {
        out.push_str(&format!(
            "## CONTINUATION (attempt {})\n\
             Your previous attempt on this task did NOT complete the sign-off check — either the {TASK_COMPLETE_SENTINEL} sentinel was missing, or your output ended by asking for approval / offering options. That is a task failure, not a valid stopping point.\n\
             - Treat this turn as a CONTINUATION of your prior work, not a fresh start.\n\
             - Pick up where you left off and finish the ENTIRE scope.\n\
             - Do not re-plan, do not summarize what you already did, do not ask what to do next. Just do the remaining work.\n\
             - End your response with the {TASK_COMPLETE_SENTINEL} sentinel (see SIGN-OFF at the bottom).\n",
            task.retries + 1
        ));
    }
    if let Some(prior) = task.output.as_deref() {
        let prior = prior.trim();
        if !prior.is_empty() {
            out.push_str("\nYour previous partial output (for context):\n");
            out.push_str("```\n");
            out.push_str(&truncate_chars(prior, 4000));
            out.push_str("\n```\n\n");
        }
    }
}

fn append_task_header(out: &mut String, task: &SwarmTask) {
    out.push_str(&format!(
        "SWARM TASK: {} ({})\n",
        task.title.trim(),
        task.id
    ));
    if let Some(role) = task.role.as_deref() {
        if !role.trim().is_empty() {
            out.push_str(&format!("ROLE: {}\n", role.trim()));
        }
    }
}

fn append_task_execution_mode(out: &mut String, task: &SwarmTask) {
    // Sentinel below is how the orchestrator detects completion; missing
    // it triggers an automatic continuation turn.
    out.push_str(
        "EXECUTION MODE: non-interactive (autonomous swarm — no human reviewer between turns).\n\
         - Complete the ENTIRE scope described below before returning; do not stop halfway.\n\
         - Never ask for approval, never offer options, never request permission to proceed.\n\
         - When the request is ambiguous, pick the safer option (narrower scope, smaller diff) and proceed; note the choice in your summary.\n\
         - \"Want me to proceed?\", \"Shall I continue?\", \"Should I do X or Y?\", \"Pause here for review?\" are all task failures.\n\
         - The orchestrator parses your output for a machine-checked completion sentinel (see SIGN-OFF section at the end). Missing the sentinel is treated as an incomplete task and you will be re-dispatched with your partial output as context.\n",
    );
    if task.writes {
        out.push_str("MODE: single-writer integrator (workspace writes allowed)\n");
    } else {
        out.push_str("MODE: read-only (do not edit the workspace)\n");
    }
}

fn append_task_mission_contract(out: &mut String, mission_kind: SwarmMissionKind) {
    if !mission_kind.allows_research_roles() {
        return;
    }
    out.push_str(&format!("MISSION FOCUS: {}\n", mission_kind.label()));
    out.push_str("MISSION CONTRACT:\n");
    match mission_kind {
        SwarmMissionKind::Research => out.push_str(
            "- This is a research mission: prioritize external sources, evidence, and ranked strategy discovery over routine code implementation.\n",
        ),
        SwarmMissionKind::ComputationalResearch => out.push_str(
            "- This is a computational-research mission: prioritize modeling, experiments, quantitative evidence, and reproducible analysis over routine code implementation.\n",
        ),
        SwarmMissionKind::General => {}
    }
}

// Role contract for a `research` / `computational-research` producer that WRITES
// its own findings file (parallel research, step 3 — see validator INV-06).
// Keeps the no-verification discipline of the read-only research contract but
// flips the deliverable: investigate AND persist the findings to the file this
// task owns, leaving the master index to the single `integrate` task.
fn research_writer_contract_lines() -> &'static [&'static str] {
    &[
        "You are a research WRITER: investigate your assigned topic AND write your findings to the output file this task owns. Produce real, well-sourced content in the file — not just notes for a downstream writer.",
        "Use your file-writing tools to create or update ONLY your assigned file (named in your task / the FILE CHECKLIST below). Do NOT write, edit, or delete any other file — the `integrate` task owns the master index, and your peers own their own files.",
        "Follow the judge's per-file content map: write exactly the sources/topics assigned to your file, structured so the index integrator can link to it. Surface sources, methods, assumptions, and the strongest findings with evidence.",
        "ROLE DISCIPLINE: Do NOT run tests, builds, lints, formatters, or any CI / verification commands — that belongs to the review agent. Do NOT duplicate investigation an upstream dependency already produced; cite it and build on it.",
    ]
}

fn append_task_role_contract(out: &mut String, task: &SwarmTask) {
    let Some(role) = task.role.as_deref().and_then(normalize_role_label) else {
        return;
    };
    out.push_str("ROLE CONTRACT:\n");
    out.push_str("- Act strictly as the assigned role for this task.\n");
    // A research / computational-research producer that writes its own findings
    // (parallel research) gets the writer contract instead of the read-only one.
    let research_writer =
        task.writes && matches!(role.as_str(), "research" | COMPUTATIONAL_RESEARCH_ROLE);
    let contract_lines = if research_writer {
        let guide = if role == COMPUTATIONAL_RESEARCH_ROLE {
            "research-experiment"
        } else {
            "research-evidence"
        };
        out.push_str(&format!(
            "- See NitSkill {guide} for the evidence workflow.\n"
        ));
        research_writer_contract_lines()
    } else {
        role_contract_lines(role.as_str())
    };
    for line in contract_lines {
        out.push_str(&format!("- {line}\n"));
    }
    if let Some(lines) = role_response_format_lines(role.as_str()) {
        out.push_str("RESPONSE FORMAT:\n");
        for line in lines {
            out.push_str(&format!("- {line}\n"));
        }
    }
}

fn append_task_done_and_artifacts(out: &mut String, task: &SwarmTask) {
    if let Some(done_when) = task.done_when.as_deref() {
        if !done_when.trim().is_empty() {
            out.push_str(&format!("DONE WHEN: {}\n", done_when.trim()));
        }
    }
    if !task.artifacts.is_empty() {
        out.push_str("ARTIFACTS:\n");
        for item in task.artifacts.iter() {
            let item = item.trim();
            if item.is_empty() {
                continue;
            }
            out.push_str(&format!("- {item}\n"));
        }
    }
}

fn append_task_scope_section(
    out: &mut String,
    task: &SwarmTask,
    scope_files: &[String],
    spawn_cwd: &Path,
) {
    if scope_files.is_empty() {
        return;
    }
    let role = task.role.as_deref().and_then(normalize_role_label);
    let role_kind = role.as_deref();
    if role_kind == Some("integrate") {
        append_task_integrate_checklist(out, scope_files);
    } else if role_kind == Some("judge") {
        append_task_judge_checklist(out, scope_files);
    } else if role_kind == Some("propose") {
        append_task_propose_scope(out, scope_files);
    } else if matches!(role_kind, Some("test") | Some("review")) && is_cargo_workspace(spawn_cwd) {
        append_task_cargo_scope(out, scope_files, role_kind == Some("test"));
    }
}

// When a task carries a shard_index, override the agent's view of "what to
// do" with the shard's own slice of the file list. Comes AFTER the full
// scope checklist so the agent still sees the broader context but is
// unambiguous that only the shard subset is its responsibility for this
// turn.
fn append_task_shard_section(out: &mut String, task: &SwarmTask, shard_files: Option<&[String]>) {
    let Some((idx, total)) = task.shard_index else {
        return;
    };
    let Some(files) = shard_files else {
        return;
    };
    out.push_str(&format!(
        "\n## YOUR SHARD ({idx}/{total}) — non-negotiable\n"
    ));
    out.push_str("The runtime split this large-scope refactor into sequential shards on the same writer agent. ");
    out.push_str("You are responsible for the file slice below. The earlier FILE CHECKLIST shows the full scope for context, but for THIS turn you must:\n");
    out.push_str("- Modify ONLY the files in the shard list below.\n");
    out.push_str("- Modify EVERY file in the shard list. \"Deferred\" / \"out of scope\" are task failures (orchestrator re-dispatches with the gap as a continuation).\n");
    if files.is_empty() {
        out.push_str("\n(Empty shard — propose/judge dependencies have not yet declared files. Pull from the FILE CHECKLIST above using your shard index, sorted alphabetically.)\n");
        return;
    }
    out.push_str(&format!(
        "\nShard files ({} total in this shard):\n",
        files.len()
    ));
    for (i, path) in files.iter().enumerate() {
        out.push_str(&format!("{}. {path}\n", i + 1));
    }
}

fn append_task_integrate_checklist(out: &mut String, scope_files: &[String]) {
    out.push_str("\n## FILE CHECKLIST (informational hint)\n");
    out.push_str("Scope comes from propose/judge `swarm_artifacts.files`. Follow the JUDGE'S VERDICT end-to-end. Inspect these candidate files. Record untouched paths under `audit-only` and necessary additional edits under `out-of-scope edits`, with reasons. Missing paths are advisory unless NIT_STRICT_CHECKLIST=1. Stub and incomplete-split checks still apply. Keep tests in dedicated test files and preserve comments that explain invariants. See NitSkill workspace-scope.\n");
    for (i, path) in scope_files.iter().enumerate() {
        out.push_str(&format!("{}. {path}\n", i + 1));
    }
    out.push_str("\nAfter finishing, list every file you actually changed and what you changed in each. Out-of-scope edits and audit-only decisions go in their own sub-sections so the reviewer can audit.\n");
}

fn append_task_propose_scope(out: &mut String, scope_files: &[String]) {
    out.push_str("\n## SCOPE — files in the target module\n");
    out.push_str("Your proposal must cover ALL of these files (no exceptions):\n");
    for (i, path) in scope_files.iter().enumerate() {
        out.push_str(&format!("{}. {path}\n", i + 1));
    }
}

/// Judge-facing view of what the proposers nominated. Renders the
/// proposer-only union so the judge can see — at decision time — every
/// path that was suggested upstream, then explicitly choose which set
/// becomes the integrator's FILE CHECKLIST via its own
/// `swarm_artifacts.files` array.
///
/// As of v0.2.10, the judge's `files` array is **authoritative** when
/// non-empty: `aggregate_judge_or_proposer_files` returns just the
/// judge's list, dropping proposer paths the judge omitted. The block
/// describes that contract explicitly so the judge sets its array
/// deliberately — the prior visibility-only behavior left the judge
/// believing it could only ADD via prose, when the runtime semantics
/// now let it ADD, REMOVE, or REPLACE the entire scope mechanically.
///
/// Operator-observed failure that drove this: when a proposer's
/// rejected split-plan paths stayed in the proposer's `files` array,
/// the union pulled them into the integrator's checklist and the
/// integrator was forced into a no-win loop — three re-dispatches,
/// ending with the integrator executing the rejected split just to
/// clear the gate. The judge's `files` array now BEING the checklist
/// closes that gap end-to-end (prompt + compliance check both honor
/// the same authority rule).
///
/// No block is rendered when no proposer artifacts have published
/// (`scope_files.is_empty()` after aggregation) — usually means the
/// judge is dispatching before its proposer deps have completed, in
/// which case there's nothing to show yet.
fn append_task_judge_checklist(out: &mut String, scope_files: &[String]) {
    if scope_files.is_empty() {
        return;
    }
    out.push_str("\n## PROPOSER-NOMINATED FILES (your authoritative selection set)\n");
    out.push_str("Every path below was nominated by an upstream proposer's `swarm_artifacts.files` array. Your `swarm_artifacts.files` is AUTHORITATIVE: when non-empty, the runtime drops the proposer arrays entirely and uses ONLY your list as the integrator's FILE CHECKLIST. The structural-compliance check, the prompt the integrator sees, and the re-dispatch logic all consult the same selection.\n");
    out.push('\n');
    for (i, path) in scope_files.iter().enumerate() {
        out.push_str(&format!("{}. {path}\n", i + 1));
    }
    out.push('\n');
    out.push_str("Your levers on this list:\n");
    out.push_str("- Files you ACCEPT: include them in your `swarm_artifacts.files`. The integrator will be required to modify each one.\n");
    out.push_str("- Files you REJECT (e.g. a proposer split-plan you didn't adopt, an audit-only file the proposer mistakenly included): leave them OUT of your `swarm_artifacts.files`. The runtime will not include them in the integrator's checklist, and the integrator will not be re-dispatched for failing to touch them. Call the rejection out in your `notes` so the integrator and the operator understand the reasoning.\n");
    out.push_str("- Files you want to ADD that no proposer mentioned (e.g. a callsite the proposers missed): include the path in your `swarm_artifacts.files`. It enters the checklist via your authority.\n");
    out.push('\n');
    out.push_str("If you publish an EMPTY `files` array — or omit the field — the runtime falls back to the proposer union as a backward-compat safety net. Always publish a deliberate selection; an empty array means \"defer to the proposers\", not \"no files are required\".\n");
}

// Inject the exact crate scope so test/review agents can't drift into
// `cargo test --all`. Only emit on actual cargo workspaces — a workspace
// can contain a `crates/` directory without being a Cargo workspace
// (vendored deps, monorepos with non-Rust subdirs, dotfiles repos that
// happen to share a token); injecting `cargo test -p <name>` into a
// non-Rust agent's prompt is the leak the operator reported on dotbox.
fn append_task_cargo_scope(out: &mut String, scope_files: &[String], is_test: bool) {
    let crates = crate_names_from_paths(scope_files);
    if crates.is_empty() {
        return;
    }
    out.push_str("\n## SCOPE — crates touched by this mission\n");
    out.push_str(
        "These are the ONLY crates you may exercise. Do NOT widen to workspace-wide (`--all` / `--workspace`) under any circumstance, even \"just to be safe\".\n",
    );
    for c in &crates {
        out.push_str(&format!("- {c}\n"));
    }
    out.push_str("\nREQUIRED COMMANDS (use exactly these; do not add `--all` or `--workspace`):\n");
    let pkg_flags: String = crates.iter().map(|c| format!(" -p {c}")).collect();
    if is_test {
        out.push_str(&format!(
            "- `cargo test{pkg_flags}` — run the scoped test suite.\n"
        ));
    } else {
        out.push_str(&format!(
            "- `cargo test{pkg_flags}` — if you need to confirm tests still pass.\n"
        ));
        out.push_str(&format!(
            "- `cargo clippy{pkg_flags} --all-targets -- -D warnings` — lint only the touched crates.\n"
        ));
    }
    out.push_str(
        "If a targeted command fails, report the failure and STOP — do not broaden the scope to diagnose.\n",
    );
}

fn append_task_dependency_outputs(
    out: &mut String,
    task: &SwarmTask,
    deps: Option<&[(String, String)]>,
    proposers_skipped: bool,
) {
    let Some(deps) = deps.filter(|d| !d.is_empty()) else {
        return;
    };
    let role_kind = task.role.as_deref().and_then(normalize_role_label);
    let role_kind = role_kind.as_deref();
    // Header phrasing signals how the integrator should treat the payload.
    // "Dependency outputs" reads as informational; for integrate we
    // escalate to "IMPLEMENTATION PLAN (BINDING)" so the writer knows
    // the proposer choices aren't suggestions.
    if role_kind == Some("judge") {
        out.push_str(&format!(
            "\nDependency outputs ({} proposals to evaluate — read ALL of them carefully before choosing):\n",
            deps.len()
        ));
    } else if role_kind == Some("integrate") {
        out.push_str(
            "\n## IMPLEMENTATION PLAN (BINDING — follow verbatim)\n\
             The proposer/judge output(s) below are the authoritative plan for this task. \
             Treat specific file paths, identifiers, constants, and ordering as fixed \
             requirements, not suggestions. See your ROLE \
             CONTRACT above for when a deviation is allowed and how to report it.\n",
        );
        if proposers_skipped {
            out.push_str(
                "Proposer outputs are intentionally omitted — the judge below has consolidated them into a single binding plan, and reading both would duplicate content and crowd out tool-use budget. The raw proposer reports are still on disk under `.nit/swarm/<mission>/tasks/propose-*/output.md` if you need to consult them; do NOT try to re-derive proposer detail from the judge's verdict on your own.\n",
            );
        }
    } else {
        out.push_str("\nDependency outputs:\n");
    }
    for (label, output) in deps.iter() {
        out.push_str(&format!("\n---\nDEP: {label}\n"));
        out.push_str(output.trim());
        out.push('\n');
    }
}

fn append_task_structured_artifacts(out: &mut String, task: &SwarmTask) {
    // Propose/judge tasks always emit the structured-artifacts block so
    // downstream integrators can parse the declared `files` array (the
    // substrate's structural-compliance check diffs this against on-disk
    // writes). Other read-only roles get the block only when the planner
    // explicitly requested it via task.artifacts.
    let role_kind = task.role.as_deref().and_then(normalize_role_label);
    let role_kind = role_kind.as_deref();
    let is_propose_or_judge = matches!(role_kind, Some("propose") | Some("judge"));
    if task.artifacts.is_empty() && !is_propose_or_judge {
        return;
    }
    out.push_str("\n## STRUCTURED ARTIFACTS (REQUIRED)\n");
    out.push_str("You MUST include a ```json code block at the END of your response with this exact structure:\n");
    out.push_str("```\n");
    out.push_str("{\n");
    out.push_str("  \"type\": \"swarm_artifacts\",\n");
    out.push_str("  \"version\": 1,\n");
    out.push_str(&format!("  \"task_id\": \"{}\",\n", task.id));
    out.push_str("  \"summary\": \"one-line summary of what you did or found\",\n");
    out.push_str("  \"artifacts\": {\n");
    out.push_str("    \"files\": [\"path/to/file\"],\n");
    out.push_str("    \"diffs\": [{\"path\": \"path/to/file\", \"summary\": \"what changed\"}],\n");
    out.push_str("    \"commands\": [\"<project test command>\"],\n");
    out.push_str("    \"risks\": [\"potential issue\"],\n");
    out.push_str("    \"notes\": [\"additional context\"],\n");
    out.push_str("    \"findings\": [{\"file\": \"path/to/file\", \"line\": 42, \"severity\": \"error\", \"issue\": \"one-line description\", \"category\": \"fmt|clippy|test|other\", \"suggestion\": \"concrete fix text\"}]\n");
    out.push_str("  }\n");
    out.push_str("}\n");
    out.push_str("```\n");
    out.push_str("Use the relevant keys. Test/review roles alone emit actionable `findings` for one integrator retry. Writers use `diffs` and `notes`. See NitSkill swarm-handoff for examples.\n");
    if is_propose_or_judge {
        out.push_str("Include EVERY file the integrator must MODIFY or CREATE in `files`. Put paths read for context and AUDIT-ONLY decisions in `notes`. The judge selects the final scope hints.\n");
    }
}

pub(super) fn build_synthesis_prompt(state: &AppState, run: &SwarmRun) -> String {
    let has_reviewer = run_has_reviewer_role(run);
    let mut out = String::new();
    append_synthesis_constraints(&mut out, has_reviewer);
    out.push_str("Operator request:\n");
    out.push_str(run.root_prompt.trim());
    out.push_str("\n\nAgent outputs:\n");
    for task in run.tasks.iter() {
        append_synthesis_agent_output(&mut out, run, task);
    }
    append_synthesis_gates(&mut out, state, run);
    if let Some(genome_results) = run.genome_gate_results.as_deref() {
        out.push_str("\n\nGenome quality review:\n");
        out.push_str(genome_results);
        out.push('\n');
    }
    if let Some(extra) = run.synthesis_prompt.as_deref() {
        out.push_str("\n\nSynthesis notes:\n");
        out.push_str(extra.trim());
        out.push('\n');
    }
    out.push_str(
        "\nResponse requirements (TEXT ONLY — no tool calls, no edits, no commands):\n\
        - Produce a cohesive synthesis of what the agents actually did and found.\n\
        - Be decisive about the outcome: what worked, what didn't, what's still open.\n\
        - If follow-up work or code changes are needed, DESCRIBE them in prose — do NOT perform them and do NOT produce diffs/patches. The operator will decide what to do next.\n\
        - If gates failed or tests are missing, REPORT that as a finding in the synthesis. Do NOT attempt to fix or rerun anything.\n\
        - Remember: you are a read-only text summarizer. Every tool call you make is a bug.\n",
    );
    out
}

fn run_has_reviewer_role(run: &SwarmRun) -> bool {
    run.tasks.iter().any(|t| {
        t.role
            .as_deref()
            .map(|r| {
                let r = r.trim();
                r.eq_ignore_ascii_case("review")
                    || r.eq_ignore_ascii_case("test")
                    || r.eq_ignore_ascii_case("genome-reviewer")
            })
            .unwrap_or(false)
    })
}

fn append_synthesis_constraints(out: &mut String, has_reviewer: bool) {
    out.push_str(
        "You are the SWARM SYNTHESIZER. Your ONLY job is to produce a text report that combines the agent outputs below into a single cohesive answer for the operator.\n\n",
    );
    out.push_str(
        "ABSOLUTE CONSTRAINTS (these override any other instruction you may have received):\n\
        1. DO NOT USE ANY TOOLS that edit files, write files, create files, delete files, move files, apply patches, or modify the workspace in any way. You are a pure read-only text summarizer.\n\
        2. DO NOT USE ANY TOOLS that run shell commands, bash, tests, builds, linters, formatters, type-checkers, CI pipelines, `cargo`/`npm`/`just`/`make`/`python -m pytest`/etc. — whatever this project's toolchain uses. Verification has ALREADY happened upstream.\n\
        3. DO NOT re-read source files, re-investigate the codebase, or call any code-search/grep/glob/find tools. The agent reports below are your ONLY source of truth.\n\
        4. DO NOT call any MCP tools or external integrations. Text output only.\n\
        5. DO NOT attempt to \"fix\" anything you notice in the reports — if you notice a problem, REPORT it in your synthesis as a known issue for the operator to decide on. You are not the integrator, not the reviewer, not the test runner.\n\n",
    );
    if has_reviewer {
        out.push_str(
            "A dedicated review/test agent already ran in this swarm — its output is in the agent reports below. Treat its verification findings as authoritative. If its findings are missing, ambiguous, or contradict another agent, note the gap in your synthesis text — DO NOT run verification yourself to resolve it.\n\n",
        );
    } else {
        out.push_str(
            "This swarm did not include a dedicated review/test agent. If verification is missing from the agent reports below, SURFACE THAT AS A GAP in your synthesis text (e.g. \"tests were not run by any agent — operator should verify manually\"). DO NOT run verification yourself. DO NOT edit files to fix issues. Your output is text only.\n\n",
        );
    }
}

fn append_synthesis_agent_output(out: &mut String, run: &SwarmRun, task: &SwarmTask) {
    out.push_str(&format!(
        "\n---\nAGENT: {}\nTASK: {} ({})\n",
        task.agent_id,
        task.title.trim(),
        task.id
    ));
    if let Some(role) = task.role.as_deref() {
        if !role.trim().is_empty() {
            out.push_str(&format!("ROLE: {}\n", role.trim()));
        }
    }
    if !task.deps.is_empty() {
        out.push_str(&format!("DEPS: {}\n", task.deps.join(", ")));
    }
    out.push_str(&format!(
        "STATUS: {}\n",
        task_state_synthesis_label(task.state)
    ));
    if let Some(summary) = task_artifacts_summary_for_prompt(task, &run.mission_id) {
        out.push_str("ARTIFACTS:\n");
        out.push_str(summary.trim());
        out.push('\n');
    } else if task.expected_artifacts_missing {
        out.push_str("ARTIFACTS: expected but missing parseable swarm_artifacts JSON block\n");
    }
    if let Some(output) = task.output.as_deref() {
        out.push_str(output.trim());
        out.push('\n');
    } else {
        out.push_str("(no output)\n");
    }
}

fn task_state_synthesis_label(state: SwarmTaskState) -> &'static str {
    match state {
        SwarmTaskState::Done => "DONE",
        SwarmTaskState::Failed => "FAILED",
        SwarmTaskState::Skipped => "SKIPPED",
        SwarmTaskState::Pending => "PENDING",
        SwarmTaskState::Ready => "READY",
        SwarmTaskState::Dispatched => "QUEUED",
        SwarmTaskState::Running => "RUNNING",
    }
}

fn append_synthesis_gates(out: &mut String, state: &AppState, run: &SwarmRun) {
    let Some(label) = run_gates_label(run) else {
        return;
    };
    out.push_str("\n\nVerification gates:\n");
    out.push_str(&format!("Bundle: {label}\n"));
    for gate in dashboard_gate_rows(state, run).iter() {
        out.push_str(&format!(
            "- {}: {} ({})\n",
            gate.name, gate.status, gate.command
        ));
    }
    if let Some(report) = run.gate_report.as_ref() {
        out.push_str("Structured report:\n```json\n");
        if let Ok(json) = serde_json::to_string_pretty(report) {
            out.push_str(&json);
        } else {
            out.push_str("{\"overall_ok\":false}");
        }
        out.push_str("\n```\n");
    } else {
        out.push_str("Structured report: (missing)\n");
    }
    if let Some(output) = run.gate_output.as_deref() {
        out.push_str("\nVerifier raw output (truncated):\n");
        out.push_str(&truncate_chars(output, SWARM_VERIFY_MAX_CHARS));
        out.push('\n');
    }
}
