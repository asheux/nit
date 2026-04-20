use std::collections::HashSet;

use super::{
    normalize_role_label, SwarmMissionKind, SwarmTask, SwarmTaskState, SwarmTemplate,
    COMPUTATIONAL_RESEARCH_ROLE,
};

#[derive(serde::Deserialize)]
pub(super) struct SwarmPlanV2 {
    #[serde(default)]
    pub(super) version: Option<u32>,
    #[serde(default)]
    pub(super) template: Option<String>,
    #[serde(default)]
    pub(super) integrator_agent_id: Option<String>,
    pub(super) tasks: Vec<SwarmPlanTaskV2>,
    #[serde(default)]
    pub(super) synthesis_prompt: Option<String>,
}

#[derive(serde::Deserialize)]
pub(super) struct SwarmPlanTaskV2 {
    #[serde(default)]
    pub(super) id: Option<String>,
    pub(super) agent_id: String,
    #[serde(default)]
    pub(super) role: Option<String>,
    pub(super) title: String,
    pub(super) prompt: String,
    #[serde(default)]
    pub(super) deps: Vec<String>,
    #[serde(default)]
    pub(super) writes: bool,
    #[serde(default)]
    pub(super) artifacts: Vec<String>,
    #[serde(default)]
    pub(super) done_when: Option<String>,
}

#[derive(serde::Deserialize)]
pub(super) struct SwarmPlanV1 {
    pub(super) tasks: Vec<SwarmPlanTaskV1>,
    #[serde(default)]
    pub(super) synthesis_prompt: Option<String>,
}

#[derive(serde::Deserialize)]
pub(super) struct SwarmPlanTaskV1 {
    pub(super) agent_id: String,
    pub(super) title: String,
    pub(super) prompt: String,
}

pub(super) struct ParsedSwarmPlan {
    pub(super) tasks: Vec<SwarmTask>,
    pub(super) synthesis_prompt: Option<String>,
    pub(super) integrator_agent_id: Option<String>,
    pub(super) warnings: Vec<String>,
}

fn task_role_is(task: &SwarmTask, role: &str) -> bool {
    task.role
        .as_deref()
        .is_some_and(|r| r.trim().eq_ignore_ascii_case(role))
}

fn bulk_is_proposer(task: &SwarmTask) -> bool {
    task_role_is(task, "propose") || task.id.to_ascii_lowercase().starts_with("propose-")
}

fn bulk_is_judge(task: &SwarmTask) -> bool {
    task_role_is(task, "judge") || task.id.eq_ignore_ascii_case("judge")
}

fn bulk_is_integrate(task: &SwarmTask) -> bool {
    task_role_is(task, "integrate") || task.id.eq_ignore_ascii_case("integrate")
}

pub(super) fn normalize_bulk_plan(
    tasks: &mut [SwarmTask],
    integrator_agent_id: Option<&str>,
) -> Vec<String> {
    let mut warnings = Vec::new();
    let proposer_ids = tasks
        .iter()
        .filter(|task| bulk_is_proposer(task))
        .map(|task| task.id.clone())
        .collect::<Vec<_>>();
    let judge_idx = tasks.iter().position(bulk_is_judge);
    let integrate_idx = tasks.iter().position(bulk_is_integrate);

    for task in tasks.iter_mut() {
        if bulk_is_proposer(task) && task.writes {
            task.writes = false;
            warnings.push(format!(
                "Bulk plan: proposer task '{}' had writes=true; forcing read-only.",
                task.id
            ));
        }
    }

    if let Some(judge_idx) = judge_idx {
        let judge_id = tasks[judge_idx].id.clone();
        let mut changed = false;
        for proposer in proposer_ids.iter() {
            if proposer == &judge_id {
                continue;
            }
            if tasks[judge_idx].deps.iter().any(|dep| dep == proposer) {
                continue;
            }
            tasks[judge_idx].deps.push(proposer.clone());
            changed = true;
        }
        if changed {
            warnings.push(
                "Bulk plan: added missing deps so the judge depends on all proposer tasks.".into(),
            );
        }
    }

    if let (Some(integrate_idx), Some(judge_idx)) = (integrate_idx, judge_idx) {
        let judge_id = tasks[judge_idx].id.clone();
        if !tasks[integrate_idx].deps.iter().any(|dep| dep == &judge_id) {
            tasks[integrate_idx].deps.push(judge_id);
            warnings.push("Bulk plan: added missing dep so integrate depends on judge.".into());
        }
    }

    if let Some(integrate_idx) = integrate_idx {
        let allowed = integrator_agent_id
            .is_none_or(|integrator| tasks[integrate_idx].agent_id == integrator);
        if allowed && !tasks[integrate_idx].writes {
            tasks[integrate_idx].writes = true;
            warnings
                .push("Bulk plan: forcing integrate task writes=true for the integrator.".into());
        }
    }

    warnings
}

/// Safety net: if the planner omitted an integrate task for a General mission,
/// inject one so the swarm can actually write to the workspace.
pub(super) fn ensure_integrate_task(
    tasks: &mut Vec<SwarmTask>,
    mission_kind: SwarmMissionKind,
    integrator_agent_id: Option<&str>,
) -> Vec<String> {
    let mut warnings = Vec::new();
    if !matches!(mission_kind, SwarmMissionKind::General) {
        return warnings;
    }
    let Some(integrator) = integrator_agent_id else {
        return warnings;
    };

    let has_integrate = tasks
        .iter()
        .any(|t| t.role.as_deref().and_then(normalize_role_label).as_deref() == Some("integrate"));
    if has_integrate {
        return warnings;
    }

    // Check if any task on the integrator agent can be promoted.
    let promote_idx = tasks
        .iter()
        .position(|t| t.agent_id == integrator && t.role.is_none());
    if let Some(idx) = promote_idx {
        tasks[idx].role = Some("integrate".into());
        tasks[idx].writes = true;
        warnings.push(format!(
            "Plan safety net: promoted task '{}' to role=integrate (writes=true) because no integrate task was found.",
            tasks[idx].id
        ));
        return warnings;
    }

    // No promotable task — inject a new integrate task that depends on all others.
    let all_deps: Vec<String> = tasks.iter().map(|t| t.id.clone()).collect();
    tasks.push(SwarmTask {
        id: "integrate".into(),
        agent_id: integrator.to_string(),
        role: Some("integrate".into()),
        title: "Integrate + implement".into(),
        task_prompt: "Implement the changes using the dependency outputs. You are the only agent allowed to make workspace edits. Process the FILE CHECKLIST above in order — open each file, refactor it, then move to the next. Prefer small, safe diffs. Follow the TEST DISCIPLINE in the role contract above for verification — do not run workspace-wide tests unless the operator explicitly asked.".into(),
        deps: all_deps,
        writes: true,
        artifacts: Vec::new(),
        done_when: Some("Changes are implemented cleanly with validations to run.".into()),
        state: SwarmTaskState::Pending,
        output: None,
        parsed_artifacts: None,
        expected_artifacts_missing: false,
        failed: false,
        retries: 0,
    });
    warnings
        .push("Plan safety net: injected integrate task because the planner omitted one.".into());
    warnings
}

/// Safety net for parallel template + general mission: if the planner
/// produced multiple integrate tasks but no read-only proposer/recon lane,
/// demote one of the integrate tasks (preferring one not on the designated
/// integrator agent) to a `propose` role and wire it as a dependency of the
/// remaining integrate tasks. This preserves parallel's write fan-out while
/// guaranteeing that at least one agent surveys the module before edits begin.
///
/// Mirrors `ensure_integrate_task` for the read-only side. Only acts when:
/// - template == Parallel (lab/bulk already enforce single-writer or
///   propose-then-judge-then-integrate via the planner prompt)
/// - mission_kind == General (research missions already lean read-only)
/// - no existing read-only role lane (propose / research / computational-research / review)
/// - at least 2 integrate tasks (so demoting one still leaves a writer)
///
/// Mutates tasks in place — never pushes or removes — so the slice is passed
/// as `&mut [SwarmTask]`, distinct from `ensure_integrate_task` which may
/// inject a new task.
/// Insert a judge-role task for parallel missions that have ≥2 proposers
/// but no judge. Without a judge, every integrator receives every proposer
/// output concatenated and must silently reconcile potentially contradictory
/// plans — exactly the decision a judge role was designed to handle.
///
/// The inserted judge runs after both proposers (role-deps wires this via
/// `default_role_deps`), consolidates into a single plan, and the integrator
/// depends on the judge. The agent assigned to the judge is picked from the
/// roster, preferring an agent that is NOT already running a propose or
/// integrate task.
pub(super) fn ensure_judge_task_for_multi_proposer(
    tasks: &mut Vec<SwarmTask>,
    template: SwarmTemplate,
    available_agents: &[String],
    integrator_agent_id: Option<&str>,
) -> Vec<String> {
    let mut warnings = Vec::new();
    if !matches!(template, SwarmTemplate::Parallel) {
        return warnings;
    }

    let is_role = |task: &SwarmTask, want: &str| -> bool {
        task.role
            .as_deref()
            .and_then(normalize_role_label)
            .as_deref()
            == Some(want)
    };

    // If a judge already exists, nothing to do.
    if tasks.iter().any(|t| is_role(t, "judge")) {
        return warnings;
    }

    let proposer_ids: Vec<String> = tasks
        .iter()
        .filter(|t| is_role(t, "propose"))
        .map(|t| t.id.clone())
        .collect();
    if proposer_ids.len() < 2 {
        return warnings;
    }

    // Pick an agent not already tied to a propose/integrate task. Fall back
    // to the integrator, then to any roster agent that isn't a proposer.
    let busy: std::collections::HashSet<&str> = tasks
        .iter()
        .filter(|t| is_role(t, "propose") || is_role(t, "integrate"))
        .map(|t| t.agent_id.as_str())
        .collect();
    let judge_agent = available_agents
        .iter()
        .find(|id| !busy.contains(id.as_str()))
        .cloned()
        .or_else(|| integrator_agent_id.map(|s| s.to_string()))
        .or_else(|| available_agents.first().cloned());
    let Some(judge_agent) = judge_agent else {
        return warnings;
    };

    let task_id = "judge-merge-proposals".to_string();
    let judge_task = SwarmTask {
        id: task_id.clone(),
        agent_id: judge_agent.clone(),
        role: Some("judge".into()),
        title: "Reconcile parallel proposer plans".into(),
        task_prompt: "Two or more proposers drafted independent plans for this mission. \
             Compare them, pick the stronger one (or synthesize a better hybrid), and \
             produce a SINGLE reconciled implementation plan for the integrator(s) to \
             follow verbatim. Be decisive: list exact file paths, identifiers, and ordering \
             the integrator must apply. Flag any proposer recommendation you rejected and \
             why. Do NOT edit the workspace; this is a text-only decision step."
            .into(),
        deps: proposer_ids.clone(),
        writes: false,
        artifacts: vec!["files".into(), "plan".into(), "rejections".into()],
        done_when: Some(
            "Single reconciled plan produced; proposer conflicts explicitly resolved.".into(),
        ),
        state: SwarmTaskState::Pending,
        output: None,
        parsed_artifacts: None,
        expected_artifacts_missing: false,
        failed: false,
        retries: 0,
    };
    tasks.push(judge_task);
    warnings.push(format!(
        "Parallel judge auto-inserted: {} proposers detected (no judge in plan) → judge '{}' on agent '{}' wired between proposers and integrator(s).",
        proposer_ids.len(),
        task_id,
        judge_agent,
    ));
    warnings
}

pub(super) fn ensure_proposer_task(
    tasks: &mut [SwarmTask],
    template: SwarmTemplate,
    mission_kind: SwarmMissionKind,
    integrator_agent_id: Option<&str>,
) -> Vec<String> {
    let mut warnings = Vec::new();
    if !matches!(template, SwarmTemplate::Parallel) {
        return warnings;
    }
    if !matches!(mission_kind, SwarmMissionKind::General) {
        return warnings;
    }

    // Bail out if any read-only proposal/research/review lane already exists.
    let has_read_only_lane = tasks.iter().any(|t| {
        if t.writes {
            return false;
        }
        let Some(role) = t.role.as_deref().and_then(normalize_role_label) else {
            return false;
        };
        matches!(
            role.as_str(),
            "propose" | "research" | COMPUTATIONAL_RESEARCH_ROLE | "review"
        )
    });
    if has_read_only_lane {
        return warnings;
    }

    let is_integrate = |task: &SwarmTask| -> bool {
        task.role
            .as_deref()
            .and_then(normalize_role_label)
            .as_deref()
            == Some("integrate")
    };

    // Need at least 2 integrate tasks: demoting one must still leave a writer.
    if tasks.iter().filter(|t| is_integrate(t)).count() < 2 {
        return warnings;
    }

    // Pick the demote target: prefer an integrate task whose agent is NOT
    // the designated integrator (so the integrator stays a writer). Fall back
    // to the first integrate task if every integrate is on the integrator.
    let demote_idx = tasks
        .iter()
        .position(|t| is_integrate(t) && Some(t.agent_id.as_str()) != integrator_agent_id)
        .or_else(|| tasks.iter().position(is_integrate));
    let Some(idx) = demote_idx else {
        return warnings;
    };

    let demoted_id = tasks[idx].id.clone();
    let demoted_agent = tasks[idx].agent_id.clone();
    tasks[idx].role = Some("propose".into());
    tasks[idx].writes = false;
    tasks[idx].title = "Module recon + design proposal".into();
    tasks[idx].task_prompt = "Survey the target module's structure (files, modules, key \
         functions) and produce a concrete file-by-file implementation plan for the integrate \
         agents to follow. List the files that need changes, the order they should be touched, \
         which integrate agent should take which subset, and any cross-file risks. Stay \
         read-only — the integrate agents will apply the changes after reading your output."
        .into();
    tasks[idx].artifacts = vec!["files".into(), "plan".into(), "risks".into()];
    tasks[idx].done_when = Some(
        "We have a concrete file-by-file implementation plan and the main risks identified.".into(),
    );
    tasks[idx].deps.clear();

    // Wire the propose task as a dep of every remaining integrate task so
    // they wait for the recon output before touching files.
    for task in tasks.iter_mut() {
        if task.id == demoted_id {
            continue;
        }
        if is_integrate(task) && !task.deps.contains(&demoted_id) {
            task.deps.push(demoted_id.clone());
        }
    }

    warnings.push(format!(
        "Plan safety net: demoted task '{demoted_id}' (agent '{demoted_agent}') to role=propose because the parallel template plan had no proposer/recon lane.",
    ));
    warnings
}

/// Safety net for the parallel template: synthesize a read-only task for
/// every agent the planner left without one. The planner prompt says "prefer
/// ONE task per agent id" but the LLM sometimes drops a role it deems
/// redundant, leaving a provisioned clone stuck at `swarm_pending`. This
/// gives that clone a role-appropriate review/research lane so the whole
/// swarm's work completes predictably.
///
/// Only runs for `Parallel`. `Lab` allows multiple tasks per agent
/// (sequentially) and may deliberately leave agents silent; `Bulk` uses an
/// explicit proposers -> judge -> integrate shape whose coverage is already
/// checked by `validate_bulk_plan`.
pub(super) fn ensure_agent_coverage(
    tasks: &mut Vec<SwarmTask>,
    template: SwarmTemplate,
    mission_kind: SwarmMissionKind,
    available_agents: &[String],
) -> Vec<String> {
    if !matches!(template, SwarmTemplate::Parallel) {
        return Vec::new();
    }

    let assigned: HashSet<&str> = tasks.iter().map(|t| t.agent_id.as_str()).collect();
    let uncovered: Vec<String> = available_agents
        .iter()
        .filter(|id| !assigned.contains(id.as_str()))
        .cloned()
        .collect();
    if uncovered.is_empty() {
        return Vec::new();
    }

    let mut used_ids: HashSet<String> = tasks.iter().map(|t| t.id.clone()).collect();
    let mut warnings = Vec::new();
    let mut counter = 1usize;

    for agent_id in uncovered.iter() {
        let (role, title, prompt, artifacts, done_when) = match mission_kind {
            SwarmMissionKind::Research => (
                "research",
                "Independent review & gap check",
                "Independently scan the operator request and sibling research outputs. Surface any missing sources, weak assumptions, or overlooked strategies. Do not repeat work already covered by a dependency task.",
                vec!["sources".to_string(), "risks".to_string()],
                Some("Evidence gaps and overlooked directions are identified.".to_string()),
            ),
            SwarmMissionKind::ComputationalResearch => (
                COMPUTATIONAL_RESEARCH_ROLE,
                "Independent methods & sanity check",
                "Independently review the operator request and sibling computational-research outputs. Sanity-check methods, assumptions, and proposed experiments; call out missing baselines or risks. Do not repeat work already covered by a dependency task.",
                vec!["methods".to_string(), "risks".to_string()],
                Some("Method gaps and missing baselines are identified.".to_string()),
            ),
            SwarmMissionKind::General => (
                "review",
                "Independent review",
                "Review the current approach for correctness, UX, and maintainability. Call out risks, regressions, and missing tests. Suggest follow-ups as text only; do not edit the workspace. Do not repeat work already covered by a dependency task.",
                vec!["risks".to_string(), "commands".to_string()],
                Some("We have an independent critique of the approach and the main risks.".to_string()),
            ),
        };

        let task_id = loop {
            let candidate = format!("cover-{counter:02}");
            counter = counter.saturating_add(1);
            if !used_ids.contains(&candidate) {
                used_ids.insert(candidate.clone());
                break candidate;
            }
        };

        warnings.push(format!(
            "Plan safety net: injected '{role}' task '{task_id}' for agent '{agent_id}' because the planner omitted it."
        ));

        tasks.push(SwarmTask {
            id: task_id,
            agent_id: agent_id.clone(),
            role: Some(role.to_string()),
            title: title.to_string(),
            task_prompt: prompt.to_string(),
            deps: Vec::new(),
            writes: false,
            artifacts,
            done_when,
            state: SwarmTaskState::Pending,
            output: None,
            parsed_artifacts: None,
            expected_artifacts_missing: false,
            failed: false,
            retries: 0,
        });
    }

    warnings
}

pub(super) fn validate_bulk_plan(
    tasks: &[SwarmTask],
    available_agents: &[String],
    integrator_agent_id: Option<&str>,
) -> Result<(), String> {
    let mut issues = Vec::new();
    let proposer_tasks = tasks
        .iter()
        .filter(|task| bulk_is_proposer(task))
        .collect::<Vec<_>>();
    let judge_task = tasks.iter().find(|task| bulk_is_judge(task));
    let integrate_task = tasks.iter().find(|task| bulk_is_integrate(task));

    if proposer_tasks.is_empty() {
        issues.push("missing proposer tasks (role=propose or id=propose-XX)".into());
    }
    if judge_task.is_none() {
        issues.push("missing judge task (role=judge or id=judge)".into());
    }
    if integrate_task.is_none() {
        issues.push("missing integrate task (role=integrate or id=integrate)".into());
    }

    if let Some(integrate_task) = integrate_task {
        if !integrate_task.writes {
            issues.push("integrate task must set writes=true (integrator step)".into());
        }
        if let Some(integrator) = integrator_agent_id {
            if integrate_task.agent_id != integrator {
                issues.push(format!(
                    "integrate task must be assigned to integrator agent '{integrator}' (got '{}')",
                    integrate_task.agent_id
                ));
            }
        }
    }

    if let Some(judge_task) = judge_task {
        for proposer in proposer_tasks.iter() {
            if proposer.id == judge_task.id {
                continue;
            }
            if !judge_task.deps.iter().any(|dep| dep == &proposer.id) {
                issues.push(format!(
                    "judge task must depend on proposer task '{}' (missing dep)",
                    proposer.id
                ));
            }
        }
    }

    if let (Some(judge_task), Some(integrate_task)) = (judge_task, integrate_task) {
        if !integrate_task.deps.iter().any(|dep| dep == &judge_task.id) {
            issues.push("integrate task must depend on judge task".into());
        }
    }

    let non_integrator_agents = match integrator_agent_id {
        Some(integrator) => available_agents
            .iter()
            .filter(|id| id.as_str() != integrator)
            .count(),
        None => available_agents.len(),
    };
    let min_proposers = if non_integrator_agents >= 2 { 2 } else { 1 };
    if proposer_tasks.len() < min_proposers {
        issues.push(format!(
            "expected at least {min_proposers} proposer tasks for bulk orchestration"
        ));
    }

    if issues.is_empty() {
        Ok(())
    } else {
        Err(issues.join("; "))
    }
}
