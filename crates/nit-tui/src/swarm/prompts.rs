use std::path::Path;

use nit_core::AppState;

use super::{
    dashboard_gate_rows, enumerate_scope_files, is_cargo_workspace, normalize_role_label,
    run_gates_label, task_artifacts_summary_for_prompt, truncate_chars, SwarmMissionKind, SwarmRun,
    SwarmTask, SwarmTaskState, SwarmTemplate, COMPUTATIONAL_RESEARCH_ROLE, NO_PADDING_CLAUSE,
    SWARM_VERIFY_MAX_CHARS, TEST_DISCIPLINE_CLAUSE,
};

/// Machine-checked sign-off sentinel. Swarm agents must emit this line exactly
/// once at the very end of a successfully completed task (after the structured
/// artifacts JSON block). Missing sentinel means the orchestrator treats the
/// output as incomplete and re-dispatches a continuation.
pub(super) const TASK_COMPLETE_SENTINEL: &str = "<SWARM_TASK_COMPLETE>";

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
    append_template_specific_constraints(&mut out, template);
    out.push_str(
        "- When the operator request involves refactoring or modifying a module/directory, the plan MUST cover ALL files in that scope. Assign a recon or propose task to survey the full directory tree first, and ensure the integrate task prompt lists every affected file.\n",
    );
    out.push_str(
        "- Each task prompt should be specific about which files or areas to focus on, not generic. The more concrete the prompt, the better the agent output.\n",
    );
    append_planner_validator_invariants(&mut out, template);
    append_planner_output_format(&mut out, template);
    append_planner_scope_section(&mut out, &scope_files);
    append_planner_memory_hits(&mut out, memory_hits, workspace_root);

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
        "You are the SWARM PLANNER inside nit. Create an execution plan for a multi-agent workflow.\n\n",
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
        (SwarmTemplate::Parallel, Some(integrator_agent_id)) => {
            out.push_str(&format!(
                "Primary integrator: `{integrator_agent_id}` (additional `integrate` tasks may be assigned to other agents when the work splits naturally — e.g., topical subareas covered by different proposers. The runtime allows multi-writer dispatch under the parallel template.).\n\n"
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
        SwarmTemplate::Parallel => out.push_str(
            "- Treat `judge` as a singleton role. The `integrate` role MAY be split across multiple tasks when the work splits naturally — e.g., one integrate task per topical subarea covered by a different proposer. The runtime allows multi-writer dispatch under parallel; use it when topical coherence beats alphabetical sharding. If you keep one integrate task, the runtime will auto-shard it for large scopes.\n",
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
                "- Prefer read-only investigation and synthesis tasks unless the operator explicitly asked for repo edits or docs changes.\n",
            );
        }
        SwarmMissionKind::ComputationalResearch => {
            out.push_str(
                "- This is a computational-research mission: prefer a workflow like source survey -> modeling / experiments / analysis -> synthesis / ranked strategy recommendation.\n",
            );
            out.push_str(
                "- `computational-research` is valid and preferred for quantitative or tool-driven lanes; `research` can support source survey and literature/context gathering.\n",
            );
            out.push_str(
                "- Prefer read-only investigation and synthesis tasks unless the operator explicitly asked for repo edits or docs changes.\n",
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
        SwarmTemplate::Parallel => {
            out.push_str(&format!(
                "- Code changes require `writes=true` and `role=integrate`. The PRIMARY integrator is `{integrator_agent_id}`; you MAY assign additional `integrate` tasks to other agents when work splits naturally (each with `writes=true`). Each integrate task's `task_prompt` MUST clearly state which files it owns.\n"
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

fn append_template_specific_constraints(out: &mut String, template: SwarmTemplate) {
    match template {
        SwarmTemplate::Parallel => {
            out.push_str(
                "- Prefer ONE task per agent id (max parallelism, deterministic tracking).\n",
            );
            out.push_str(
                "- REQUIRED: reserve at least ONE non-integrate lane for a `propose` (or `research` / `recon`) task that surveys the target module first and outputs a concrete file-by-file implementation plan. The remaining agents take `integrate` and split the file work, with the propose task as a dep. Even when the scope is large and multiple integrate tasks are allowed, do NOT make every agent an integrator — a single propose/recon lane should always run first as a dep for the integrate tasks.\n",
            );
            out.push_str(
                "- Prefer tasks that can run in parallel (deps should usually be empty), except where the propose lane feeds the integrate tasks.\n",
            );
            out.push_str(
                "- If you assign producer/consumer-style roles (e.g. research or computational-research → judge), use deps to express required ordering.\n",
            );
            out.push_str(
                "- Use `propose`, `research`, `review`, and `test` for the remaining lanes instead of repeating singleton roles.\n",
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
                "- Use read-only proposal/review tasks for codebase work; use research roles only when external/topic research is part of the mission.\n",
            );
            out.push_str(
                "- PROPOSER PARALLELISM (lab): if you assign multiple propose tasks, they MUST have empty `deps` and run concurrently. Do NOT chain them (propose-02 depending on propose-01 etc.) — proposers are independent investigators, not a pipeline. A judge task then fans in via `deps = [propose-01, propose-02, ...]` and waits for all of them in parallel, not serially. Sequential proposers just waste wall-clock time; the judge has to wait for the last one regardless.\n",
            );
            out.push_str(
                "- PROPOSER LENSES (lab): if you assign multiple propose tasks, each one's `task_prompt` MUST open with a distinct LENS framing so the proposers diverge on a real optimisation axis instead of producing three samples of the same prompt. Without distinct lenses, correlated output gives the judge nothing to choose between. Use one of these framings per proposer (pick the ones that fit the request, or invent a sharper axis for the specific scope):\n\
                 -   LENS A (minimal-diff, focused): favor the smallest change that solves the request. Avoid new abstractions, new modules, new types, new dependencies unless strictly required. Blast radius as small as possible.\n\
                 -   LENS B (architectural coherence): if the shape of the code is what's causing the problem, propose the consolidation, split, or abstraction the system is asking for. Larger diff fine when it lands on a better overall shape.\n\
                 -   LENS C (incremental/staged): prefer a multi-step plan that converts the current shape into the target shape through atomic, reversible steps. Optimise for safety and roll-back at each step.\n\
                 -   LENS D (performance/genome-first): target the files the GENOME LANDSCAPE flags as lowest-tier or highest-leverage. Accept more invasive changes when they unlock a tier jump or eliminate parsimony bloat.\n\
                 -   LENS E (safety/invariants): prioritise invariants, error handling, edge cases the tests don't cover. Be defensive about assumptions; flag anything the current tests don't exercise.\n\
                 Bake the lens name AND a one-line restatement of its axis into the first paragraph of the task's `task_prompt`. The judge and integrator need to see the axis the proposer was optimising for so they can weigh tradeoffs.\n",
            );
        }
        SwarmTemplate::Bulk => {
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
fn append_planner_validator_invariants(out: &mut String, template: SwarmTemplate) {
    let lines = super::validator::planner_invariants_for_prompt(template);
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

pub(crate) fn role_contract_lines(role: &str) -> &'static [&'static str] {
    match role {
        "propose" => &[
            "Advance one concrete solution candidate from your assigned lens.",
            "Do not judge between candidates or claim final implementation ownership.",
            "Be specific about files, commands, and risks.",
            "ROLE DISCIPLINE: You are read-only and propose-only. Do NOT run tests, builds, type-checkers, lints, formatters, CI pipelines, or any other verification commands in this project — whatever toolchain it uses. Suggest commands as text only; running them is the integrate/test/review agent's job. Do NOT redo investigation that an upstream task already covered; build on dependency outputs instead of repeating them.",
            "GENOME-AWARE PROPOSAL — STRICT: a GENOME LANDSCAPE section may be attached below with current tier/consistency/generations/parsimony for every scope file. When it is present, you MUST ground your proposal in those numbers. For every recommendation, name the file, the current metric, the target metric, and the direction — e.g. \"split <mega-file>: current <N> lines / structural density <x> → aim <M> submodules each ≤1500 lines, density ≥0.25\", \"inline <bloated-file> trivial predicates: parsimony-bloat cap at tier IV → consolidate single-line fns into compound checks to unlock higher tier\", \"kill <file> lines A-B (entropy 0.0) → replace with a single templated helper\". Do NOT emit surface-level advice (\"rename x to y\", \"extract helper\") without tying it to a concrete encoder metric it is meant to move. If the landscape shows mega-files (>2000 lines), low structural density (≤0.10), zero-entropy blocks, parsimony bloat, or cross-encoder consistency spread >0.3, those are the highest-leverage fixes — name them explicitly.",
            "RECOMMENDATION COVERAGE: Do not stop at one suggestion per file. Scan the whole landscape and recommend every class of fix the integrator could apply: structural splits, entropy elimination, cyclomatic-complexity reduction (target ≤8 per fn), AST component fan-out (target ≥5), identifier uniqueness (≥65%), comment-to-code ratio, consolidation of parsimony-capped files. The integrator only writes what you surface — missing a whole category means it never gets fixed.",
            "MANDATORY STRUCTURAL SPLITS — STRICT: for EVERY scope file over 2000 lines, tier I/II, or with a density ≤0.10, your proposal MUST contain a concrete split plan: name the new submodule files you want created, assign specific functions/types to each, and list the files by path in your `swarm_artifacts.files` array so the integrator has an explicit target list. Same rule for any file with parsimony bloat (consolidation plan) or zero-entropy blocks (deduplication plan). Silence on a file that breaches these thresholds is a proposal failure, even if the rest of your proposal is excellent. If the landscape below includes a THRESHOLDS BREACHED section, every listed file must appear in your recommendations with a specific structural action.",
            "FILES ARRAY = LOAD-BEARING HANDOFF: your `swarm_artifacts.files` is not just metadata — the runtime treats it as the canonical scope for the integrator's structural-compliance check. Include EVERY file the integrator must MODIFY or CREATE: existing files to edit + new files to create (every submodule path the split plan introduces). Do NOT include files where you concluded \"audit only / no changes needed\" — mention those in your prose summary or in `notes`, not in `files`. The compliance check requires every entry to be modified; an audit-only file in `files` would trigger a false-positive re-dispatch. Files omitted from `files` are silently skippable; files included that don't get touched (or get touched only with stub doc-comment shells) trigger an automatic re-dispatch. If you describe a split in prose but only put the source file in `files`, the integrator can create stub submodules to game the existence check — list each new submodule path explicitly.",
        ],
        "research" => &[
            "Explore the topic through papers, docs, web resources, and related references when available.",
            "Surface competing ideas, promising directions, and the best strategy candidates with evidence.",
            "Do not turn this into a final implementation or winner-picking step; hand off concrete findings.",
            "ROLE DISCIPLINE: You are read-only research. Do NOT run tests, builds, lints, formatters, or any CI commands — verification belongs to the integrate/test/review agents. Do NOT repeat work already covered by an upstream dependency task; cite it and move on.",
        ],
        COMPUTATIONAL_RESEARCH_ROLE => &[
            "Handle the broad computation-heavy lane: simulations, modeling, numerical methods, optimization, data/model fitting, pattern or network analysis, and reproducible research workflows.",
            "Perform tool-assisted research with explicit methods, commands, sources, assumptions, and computations.",
            "Use the findings to recommend strong strategies or narrow the search space for downstream roles across technical domains.",
            "ROLE DISCIPLINE: Tool use must be in service of computation/analysis for this task — do NOT run the project's test suite, build, lints, or CI as a side activity. Do not duplicate investigation an upstream task already produced.",
        ],
        "judge" => &[
            "Compare the dependency outputs and choose the best path forward.",
            "Produce a decisive recommendation, acceptance criteria, and verification steps.",
            "Do not edit the workspace or perform the final implementation.",
            "DECISION AXES — rank every proposal against all six; name the axis when you rule for or against one: (1) Correctness: solves the user's request. (2) Landscape fit: targets the lowest-tier / highest-leverage files in the GENOME LANDSCAPE. (3) Parsimony: no metric-gaming over-engineering. (4) Blast radius: diff scoped to the ask. (5) Robustness: handles the failure modes the request implies (real boundary inputs, partial failures, error returns) WITHOUT inventing defensive code for impossible scenarios — penalise both under-handled real boundaries AND over-handled impossible ones. (6) Novelty: applies a fitting non-obvious abstraction rather than recycling boilerplate; reward genuine structural novelty, do NOT reward novelty added purely for token variety (that's already covered by parsimony — don't double-count). Position bias is the most common judge failure: do NOT silently pick the first proposal — when proposals agree, identify risks neither addressed; when they disagree, name the disagreement, name the axis, and rule with a cited reason.",
            "ROLE DISCIPLINE: Pure decision step. Do NOT run tests, builds, lints, formatters, or any verification commands — text analysis only, based on the proposals and (if present) the GENOME LANDSCAPE below. List any commands you'd recommend as suggestions for the integrator/reviewer, not actions you take yourself. Do NOT re-explore the problem space that the proposers already covered; just compare and decide.",
            "LANDSCAPE-AWARE JUDGING: if a GENOME LANDSCAPE or THRESHOLDS BREACHED section is attached below, your decision MUST be grounded in it. Prefer proposals whose recommendations target the lowest-tier / highest-leverage files (mega-files, parsimony-capped, low-density). Reject or downgrade proposals that recommend changes uncorrelated with the landscape (e.g. cosmetic tweaks on tier-IV files while tier-I/II files go untouched). Name the specific landscape metrics in your verdict.",
            "GENOME: nit measures code across four encoders: token_spectrum, ast_structure, complexity_field, structural. See the full ENCODER GUIDE and TARGETS in the genome instructions attached to this prompt. Prefer proposals that enable varied AST node types, low per-function complexity (<= 8), diverse token-role sequences, and >= 5 structural components. Flag proposals that would force monolithic functions or repetitive patterns.",
            "FILES ARRAY = LOAD-BEARING HANDOFF: your `swarm_artifacts.files` is the canonical scope for the integrator's structural-compliance check. The unified plan you produce MUST list every file path the integrator is expected to MODIFY or CREATE — existing files to edit, plus every new submodule path the chosen split plan introduces. Do NOT include files you decided are audit-only / no-changes-needed; mention those in your prose verdict or `notes`, not in `files`. The compliance check requires every entry to be modified. Files omitted from `files` are files the integrator can silently skip without retry; files that get touched only as stub doc-comment shells get caught by the runtime's stub detector. If a proposer's split plan you accepted introduces 8 new submodule paths, all 8 MUST be in `files` — not just the original source path. Be exhaustive here even if it duplicates content from the proposers.",
        ],
        "integrate" => &[
            "Implement the chosen plan and convert it into concrete edits.",
            "Do not restart broad ideation; focus on carrying the selected approach through.",
            "If a FILE CHECKLIST is provided above, you MUST modify every listed file — process them in order, one by one. A file left unchanged means your task is incomplete.",
            "DEFERRAL = TASK FAILURE: \"applied the non-structural portion\", \"deferred the directory splits\", \"out of time so I'll stop at 70%\", \"would risk breaking the workspace\" are all task failures, not graceful stopping points. The orchestrator detects the gap from `git diff` against the proposer's declared file list and re-dispatches you with the specific files you missed — you cannot finish by self-classifying part of the plan as out-of-scope. If a single file genuinely cannot be modified (compile error blocks the change after best-effort attempts, missing toolchain feature, etc.), emit a per-file `risks` entry in the structured artifacts JSON naming the file and the technical blocker. Do not aggregate skipped work into a prose paragraph at the end.",
            "STUB FILES = TASK FAILURE: creating a new file whose body is just header comments saying \"Stub: still lives in <other file>\", \"Deferred to a dedicated turn\", \"Tracked in the risks JSON\" — or anything similar — is a task failure, not a placeholder. The runtime snapshots every declared file's line count BEFORE you start and compares post-edit. A newly-created declared file with fewer than 20 lines is detected as a stub. When the plan calls for splitting a large source file into a directory of smaller modules, those new files MUST contain the moved code; the original source MUST shrink commensurately (≥30% reduction expected on a declared large source). The runtime detects performative splits (new sibling files exist but the source kept ≥70% of its lines) and re-dispatches you with the gap descriptor showing pre/post line counts. If the move requires multiple rounds of compile/test verification, do it in atomic steps within THIS turn — move one unit at a time, fix imports, build — not by leaving stub markers and asking for a future turn.",
            "Report exact files changed and validation results.",
            "PROPOSER-PLAN BINDING — STRICT: any upstream propose/judge task output in the Dependency outputs section below is BINDING, not informational. You MUST implement the proposer's specific choices — file paths, identifiers, constants, architectural decisions, ordering — exactly as specified. Do NOT substitute your own design, invent new files the proposer didn't mention, or skip files the proposer listed. You MAY deviate only when (a) the proposer's recommendation directly contradicts the operator's original request above, or (b) the recommendation is genuinely technically impossible — meaning it names a non-existent type, breaks a hard compile invariant, or requires a feature the toolchain doesn't support. \"It might break tests\", \"it's risky\", \"it's too ambitious for one turn\", \"I'll do a safer subset\", \"full splits are too aggressive\", \"aggressive splits would almost certainly break imports\" are NOT valid deviations — they're excuses for doing less work. The correct response to a risky split is to execute it in atomic compilation-preserving steps (move one submodule at a time, re-run the build after each, fix imports as you go), NOT to substitute cosmetic cleanups (trim comments, flatten nesting, extract a helper) for the declared structural plan. If you genuinely cannot execute the plan after attempting it, STOP, leave partial progress on disk, and in your final message list exactly which files you moved, which remain, and the specific compile/test failure blocking further progress. Substituting your own smaller-scoped refactor for the declared structural plan is a task failure — the proposer, not you, decided what this task is. \"Do not break functionality\" is a constraint on HOW you execute the plan (atomic steps), not a license to SKIP the plan.",
            TEST_DISCIPLINE_CLAUSE,
            NO_PADDING_CLAUSE,
            "GENOME QUALITY OBLIGATION: You are the sole writer. Your code is measured by nit's genome system across four encoders (token_spectrum, ast_structure, complexity_field, structural). The runtime captures pre-edit genome scores per file, runs the encoders again post-edit, and feeds regressions back to you on retry. Cosmetic edits that don't move the underlying structural metrics — extracting a one-line helper, renaming a variable, trimming a comment — won't pass: the encoders measure cyclomatic complexity, AST component fan-out, identifier uniqueness, comment-to-code ratio, and structural cohesion. The proposer's plan was crafted to move those metrics; substituting a smaller refactor that doesn't move them is a task failure even if the file count looks right. Aim for Tier III+ (Spaceship) minimum, aspire to Tier V (Replicator). Do NOT call [evaluate_genome] — the runtime evaluates automatically after your writes hit disk.",
        ],
        "review" => &[
            "Critique the current output or diff for correctness, UX, and maintainability.",
            "Call out risks, regressions, and missing tests.",
            "Do not edit the workspace; suggest follow-ups as text only.",
            TEST_DISCIPLINE_CLAUSE,
            "GENOME: nit measures code across four encoders. See the full ENCODER GUIDE and TARGETS in the genome instructions attached to this prompt. Name the affected encoder when flagging issues — e.g., 'complexity 12 in parse_config (complexity_field: target <= 8)', 'only 2 node types (ast_structure: need >= 5 components)', 'repeated role sequence across match arms (structural: role n-gram uniqueness)', 'comment-to-code ratio too low (token_spectrum)'. Suggest concrete refactoring the integrator can apply.",
        ],
        "test" => &[
            "Focus on validation commands, expected results, and edge cases.",
            "Differentiate confirmed results from unrun suggestions.",
            "Do not redesign the solution unless a test failure makes it necessary.",
            TEST_DISCIPLINE_CLAUSE,
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

fn append_task_role_contract(out: &mut String, task: &SwarmTask) {
    let Some(role) = task.role.as_deref().and_then(normalize_role_label) else {
        return;
    };
    out.push_str("ROLE CONTRACT:\n");
    out.push_str("- Act strictly as the assigned role for this task.\n");
    for line in role_contract_lines(role.as_str()) {
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
    out.push_str("\n## FILE CHECKLIST (non-negotiable)\n");
    out.push_str("This list is sourced from the propose/judge `swarm_artifacts.files` arrays (your dependency outputs below) — it's the canonical scope the runtime's structural-compliance check enforces. Modifying every file here is required; touching only some, or creating others as stub doc-comment shells, triggers automatic re-dispatch.\n");
    out.push_str("\"Refactor module\" = refactor EVERY file below. No exceptions, no skipping.\n");
    out.push_str("Process this checklist in order. Open each file, read it, refactor it, then move to the next.\n");
    out.push_str("Even if a file looks clean, improve naming, docs, structure, or consistency.\n");
    out.push_str("Do NOT add inline test modules (`#[cfg(test)] mod tests { ... }`) inside source files. Tests must live in a dedicated tests directory or test file.\n");
    out.push_str("COMMENTS: Trim doc comments that restate the type/function name, echo visible type signatures, or describe obvious behavior (e.g. \"/// Returns the value\" on fn value()). Keep comments that explain WHY something is done, document non-obvious constraints, safety invariants, or algorithmic choices. A comment worth keeping tells the reader something the code alone cannot.\n");
    out.push_str("Your task is NOT complete until every file has been modified.\n\n");
    for (i, path) in scope_files.iter().enumerate() {
        out.push_str(&format!("{}. {path}\n", i + 1));
    }
    out.push_str("\nAfter finishing, list every file and what you changed in each.\n");
}

fn append_task_propose_scope(out: &mut String, scope_files: &[String]) {
    out.push_str("\n## SCOPE — files in the target module\n");
    out.push_str("Your proposal must cover ALL of these files (no exceptions):\n");
    for (i, path) in scope_files.iter().enumerate() {
        out.push_str(&format!("{}. {path}\n", i + 1));
    }
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
             requirements, not suggestions. See PROPOSER-PLAN BINDING in your ROLE \
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
    out.push_str("    \"notes\": [\"additional context\"]\n");
    out.push_str("  }\n");
    out.push_str("}\n");
    out.push_str("```\n");
    out.push_str("Only include artifact keys relevant to your task. This JSON block is machine-parsed by the swarm orchestrator — omitting it means your output cannot be tracked.\n");
    if is_propose_or_judge {
        out.push_str(
            "\n### `files` array — load-bearing for compliance\n\
             Your `files` array is the canonical handoff to the integrator AND the source of truth for the runtime's structural-compliance check. Include EVERY file the integrator must MODIFY or CREATE:\n\
             - Existing files to MODIFY.\n\
             - NEW files to CREATE (e.g., when the plan calls for splitting a large source into a directory of submodules, list each new submodule path).\n\
             Do NOT include:\n\
             - Files you read for CONTEXT only (\"I read X to understand Y\").\n\
             - Files where you concluded AUDIT-ONLY / no-changes-needed. Mention these in your prose summary or in the `notes` field, not in `files`. The compliance check requires every entry in `files` to be modified — an audit-only file here would trigger a false-positive re-dispatch.\n\
             Files omitted from `files` are files the integrator can silently skip; files included here that don't get touched (or get touched only with stub doc-comment shells) trigger an automatic re-dispatch with the gap descriptor.\n",
        );
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
