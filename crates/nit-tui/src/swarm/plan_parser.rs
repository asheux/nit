use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;

use nit_core::AppState;

use super::{
    apply_lab_lenses, chat_clone_base_id, explicit_swarm_mission_kind_from_prompt,
    extract_json_code_block, fallback_tasks, is_swarm_clone_agent_id, normalize_bulk_plan,
    normalize_lab_plan, parse_swarm_template, swarm_clone_base_id, validate_bulk_plan,
    ParsedSwarmPlan, SwarmMissionKind, SwarmPlanTaskV2, SwarmPlanV1, SwarmPlanV2, SwarmTask,
    SwarmTaskState, SwarmTemplate, COMPUTATIONAL_RESEARCH_ROLE, COMPUTATIONAL_RESEARCH_ROLE_LEGACY,
};

#[derive(Copy, Clone, Debug, Default)]
struct RoleDepStats {
    added: usize,
    skipped_cycle: usize,
}

pub(crate) fn normalize_role_label(raw: &str) -> Option<String> {
    let role = raw.trim().to_ascii_lowercase();
    if role.is_empty() {
        return None;
    }
    if role.eq_ignore_ascii_case("all") {
        return None;
    }
    if role.eq_ignore_ascii_case(COMPUTATIONAL_RESEARCH_ROLE_LEGACY) {
        return Some(COMPUTATIONAL_RESEARCH_ROLE.into());
    }
    Some(role)
}

fn role_is_singleton(role: &str) -> bool {
    matches!(
        normalize_role_label(role).as_deref(),
        Some("judge" | "integrate")
    )
}

fn role_requires_research_intent(role: &str) -> bool {
    matches!(
        normalize_role_label(role).as_deref(),
        Some("research" | COMPUTATIONAL_RESEARCH_ROLE)
    )
}

fn prompt_contains_any(prompt: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| prompt.contains(needle))
}

fn prompt_explicitly_requests_research_role(prompt: &str) -> bool {
    prompt_contains_any(
        prompt,
        &[
            "mission=research",
            "mission: research",
            "use research",
            "assign research",
            "need research",
            "want research",
            "with research role",
            "research agent",
            "research lane",
        ],
    )
}

fn prompt_explicitly_requests_computational_research_role(prompt: &str) -> bool {
    prompt_contains_any(
        prompt,
        &[
            "mission=computational",
            "mission=computational-research",
            "mission: computational",
            "mission: computational-research",
            "mission: computational research",
            "use computational research",
            "use computational-research",
            "assign computational research",
            "assign computational-research",
            "need computational research",
            "need computational-research",
            "want computational research",
            "want computational-research",
            "with computational research role",
            "with computational-research role",
            "computational research agent",
            "computational-research agent",
            "computational research lane",
            "computational-research lane",
        ],
    )
}

fn prompt_has_research_intent(prompt: &str) -> bool {
    if prompt_contains_any(
        prompt,
        &[
            "do research",
            "conduct research",
            "research the",
            "research this topic",
            "survey the literature",
            "literature review",
            "read papers",
            "read the papers",
            "search the web",
            "browse the web",
            "search online",
            "find sources",
            "find references",
            "gather citations",
            "prior art",
            "related work",
            "explore ideas",
            "explore topics",
            "new ideas",
        ],
    ) {
        return true;
    }

    prompt_contains_any(
        prompt,
        &[
            "research",
            "investigate",
            "survey",
            "study",
            "search",
            "browse",
            "read",
            "compare",
            "evaluate",
            "explore",
        ],
    ) && prompt_contains_any(
        prompt,
        &[
            "papers",
            "literature",
            "web",
            "online",
            "sources",
            "references",
            "citations",
            "resources",
            "prior art",
            "related work",
            "topic",
            "topics",
            "ideas",
            "hypothesis",
            "hypotheses",
        ],
    )
}

fn prompt_has_computational_research_intent(prompt: &str) -> bool {
    if prompt_contains_any(
        prompt,
        &[
            "computational research",
            "run simulations",
            "build a model",
            "model this",
            "numerical study",
            "optimization study",
            "design an experiment",
            "reproducible analysis",
        ],
    ) {
        return true;
    }

    prompt_contains_any(
        prompt,
        &[
            "simulation",
            "simulate",
            "modeling",
            "modelling",
            "numerical",
            "optimization",
            "optimisation",
            "data fitting",
            "model fitting",
            "network analysis",
            "pattern analysis",
            "reproducible",
            "benchmark",
            "experiment",
            "measurement",
        ],
    ) && prompt_contains_any(
        prompt,
        &[
            "research",
            "study",
            "evaluate",
            "compare",
            "topic",
            "topics",
            "hypothesis",
            "hypotheses",
            "papers",
            "literature",
            "sources",
            "evidence",
            "dataset",
            "datasets",
            "methods",
        ],
    )
}

pub(crate) fn detect_swarm_mission_kind_from_prompt(root_prompt: &str) -> Option<SwarmMissionKind> {
    let prompt = root_prompt.trim().to_ascii_lowercase();
    if prompt.is_empty() {
        return None;
    }

    if let Some(kind) = explicit_swarm_mission_kind_from_prompt(root_prompt) {
        return Some(kind);
    }

    if prompt_explicitly_requests_computational_research_role(prompt.as_str())
        || prompt_has_computational_research_intent(prompt.as_str())
    {
        return Some(SwarmMissionKind::ComputationalResearch);
    }

    if prompt_explicitly_requests_research_role(prompt.as_str())
        || prompt_has_research_intent(prompt.as_str())
    {
        return Some(SwarmMissionKind::Research);
    }

    None
}

pub(super) fn classify_swarm_mission_kind(
    root_prompt: &str,
    explicit: Option<SwarmMissionKind>,
) -> SwarmMissionKind {
    explicit
        .or_else(|| detect_swarm_mission_kind_from_prompt(root_prompt))
        .unwrap_or(SwarmMissionKind::General)
}

fn role_allowed_for_mission(mission_kind: SwarmMissionKind, role: &str) -> bool {
    if !role_requires_research_intent(role) {
        return true;
    }
    mission_kind.allows_role(role)
}

pub(super) fn direct_role_hint_for_agent(
    role_hints_by_agent_id: &HashMap<String, String>,
    agent_id: &str,
) -> Option<String> {
    role_hints_by_agent_id
        .get(agent_id)
        .and_then(|hint| normalize_role_label(hint.as_str()))
}

fn inherited_clone_role_hint_for_agent(
    role_hints_by_agent_id: &HashMap<String, String>,
    agent_id: &str,
) -> Option<String> {
    let base_id = swarm_clone_base_id(agent_id)?;
    let hint = direct_role_hint_for_agent(role_hints_by_agent_id, base_id)?;
    (!role_is_singleton(hint.as_str())).then_some(hint)
}

fn inferred_role_hint_for_agent(
    role_hints_by_agent_id: &HashMap<String, String>,
    agent_id: &str,
    integrator_agent_id: Option<&str>,
    mission_kind: SwarmMissionKind,
) -> Option<String> {
    let hint = direct_role_hint_for_agent(role_hints_by_agent_id, agent_id)
        .or_else(|| inherited_clone_role_hint_for_agent(role_hints_by_agent_id, agent_id))?;
    if hint == "integrate" && integrator_agent_id.is_some_and(|integrator| integrator != agent_id) {
        return None;
    }
    if !role_allowed_for_mission(mission_kind, hint.as_str()) {
        return None;
    }
    Some(hint)
}

pub(super) fn planner_role_hint_for_agent(
    role_hints_by_agent_id: &HashMap<String, String>,
    agent_id: &str,
    integrator_agent_id: Option<&str>,
    mission_kind: SwarmMissionKind,
) -> String {
    inferred_role_hint_for_agent(
        role_hints_by_agent_id,
        agent_id,
        integrator_agent_id,
        mission_kind,
    )
    .unwrap_or_else(|| "all".into())
}

/// Deduplicate inherited role hints so that clones of the same base agent don't
/// all receive the same hint. Only the first clone keeps the inherited hint; the
/// rest get "all" so the planner is free to diversify roles.
pub(super) fn deduplicate_inherited_role_hints(
    role_hints: &mut [(String, String)],
    role_hints_by_agent_id: &HashMap<String, String>,
) {
    let mut seen_inherited: HashMap<&str, usize> = HashMap::new();
    for (idx, (agent_id, hint)) in role_hints.iter().enumerate() {
        if hint == "all" {
            continue;
        }
        // Check if this hint was inherited (agent has no direct hint but its base does).
        let has_direct = direct_role_hint_for_agent(role_hints_by_agent_id, agent_id).is_some();
        if has_direct {
            continue;
        }
        let Some(base_id) = swarm_clone_base_id(agent_id).or_else(|| chat_clone_base_id(agent_id))
        else {
            continue;
        };
        seen_inherited.entry(base_id).or_insert(idx);
    }
    // Second pass: reset duplicates to "all".
    let mut count_by_base: HashMap<&str, usize> = HashMap::new();
    for (agent_id, hint) in role_hints.iter_mut() {
        if hint == "all" {
            continue;
        }
        let has_direct = direct_role_hint_for_agent(role_hints_by_agent_id, agent_id).is_some();
        if has_direct {
            continue;
        }
        let Some(base_id) = swarm_clone_base_id(agent_id).or_else(|| chat_clone_base_id(agent_id))
        else {
            continue;
        };
        let count = count_by_base.entry(base_id).or_insert(0);
        if *count > 0 {
            *hint = "all".into();
        }
        *count += 1;
    }
}

/// Always assigns role hints to fresh clones in the parallel template so the
/// swarm covers a `propose` lane and a `review`/`test` lane — mirroring the
/// lab template's read-only worker structure (synthesizer, propose, review,
/// integrator). Priority agents (or other agents with pre-assigned hints)
/// that already declare those roles satisfy the requirement; clones are only
/// filled in where coverage is missing.
///
/// Runs regardless of the planner's own role hint — there is no escape hatch.
/// The planner is always the synthesizer, and the swarm should always have
/// reasonable role coverage so the LLM produces a balanced plan instead of
/// the all-integrate failure mode.
///
/// Coverage rules:
/// - The `propose` slot is satisfied by any non-planner agent with role hint
///   `propose`, `research`, or `computational-research`.
/// - The `review`/`test` slot is satisfied by any non-planner agent with role
///   hint `review` or `test`.
/// - The designated integrator (already chosen by the caller) is excluded
///   from clone role assignment so it stays a writer.
pub(super) fn assign_clone_roles_for_parallel_coverage(
    state: &mut AppState,
    template: SwarmTemplate,
    planner_agent_id: &str,
    integrator_agent_id: Option<&str>,
    agents: &[String],
) -> Vec<(String, &'static str)> {
    if !matches!(template, SwarmTemplate::Parallel) {
        return Vec::new();
    }

    let mut has_propose = false;
    let mut has_review_or_test = false;
    for id in agents {
        if id.as_str() == planner_agent_id {
            continue;
        }
        let Some(role) =
            direct_role_hint_for_agent(&state.agents.swarm_role_by_agent_id, id.as_str())
        else {
            continue;
        };
        match role.as_str() {
            "propose" | "research" => has_propose = true,
            r if r == COMPUTATIONAL_RESEARCH_ROLE => has_propose = true,
            "review" | "test" => has_review_or_test = true,
            _ => {}
        }
    }

    if has_propose && has_review_or_test {
        return Vec::new();
    }

    // Find clones without an explicit role hint that we can assign to.
    // Exclude the designated integrator so it stays a writer.
    let assignable_clones: Vec<String> = agents
        .iter()
        .filter(|id| id.as_str() != planner_agent_id)
        .filter(|id| Some(id.as_str()) != integrator_agent_id)
        .filter(|id| is_swarm_clone_agent_id(id.as_str()))
        .filter(|id| {
            direct_role_hint_for_agent(&state.agents.swarm_role_by_agent_id, id.as_str()).is_none()
        })
        .cloned()
        .collect();

    let mut to_assign: Vec<&'static str> = Vec::new();
    if !has_propose {
        to_assign.push("propose");
    }
    if !has_review_or_test {
        to_assign.push("review");
    }

    let mut assignments = Vec::new();
    for (clone_id, role) in assignable_clones.into_iter().zip(to_assign.into_iter()) {
        state
            .agents
            .swarm_role_by_agent_id
            .insert(clone_id.clone(), role.to_string());
        assignments.push((clone_id, role));
    }
    assignments
}

fn infer_role_from_task_id(task_id: &str) -> Option<&'static str> {
    let id = task_id.trim();
    if id.is_empty() {
        return None;
    }
    if id.to_ascii_lowercase().starts_with("propose-") {
        return Some("propose");
    }
    if id.eq_ignore_ascii_case("judge") {
        return Some("judge");
    }
    if id.eq_ignore_ascii_case("integrate") || id.eq_ignore_ascii_case("implement") {
        return Some("integrate");
    }
    if id.eq_ignore_ascii_case("review") {
        return Some("review");
    }
    if id.eq_ignore_ascii_case("test") {
        return Some("test");
    }
    None
}

fn infer_integrator_agent_id_from_v2_tasks(
    tasks: &[SwarmPlanTaskV2],
    available_agents: &[String],
) -> Option<(String, &'static str)> {
    let normalize_agent_id = |raw: &str| {
        let raw = raw.trim();
        available_agents
            .iter()
            .find(|candidate| candidate.as_str() == raw)
            .cloned()
    };

    let mut integrate_agents = Vec::new();
    let mut writer_agents = Vec::new();
    for task in tasks.iter() {
        let Some(agent_id) = normalize_agent_id(task.agent_id.as_str()) else {
            continue;
        };

        let has_integrate_role = task
            .role
            .as_deref()
            .and_then(normalize_role_label)
            .as_deref()
            == Some("integrate")
            || task
                .id
                .as_deref()
                .and_then(infer_role_from_task_id)
                .is_some_and(|role| role == "integrate");
        if has_integrate_role
            && !integrate_agents
                .iter()
                .any(|existing| existing == &agent_id)
        {
            integrate_agents.push(agent_id.clone());
        }
        if task.writes && !writer_agents.iter().any(|existing| existing == &agent_id) {
            writer_agents.push(agent_id);
        }
    }

    if integrate_agents.len() == 1
        && (writer_agents.is_empty() || writer_agents.iter().all(|id| id == &integrate_agents[0]))
    {
        let reason = if writer_agents.is_empty() {
            "integrate task"
        } else {
            "integrate task + writes=true task"
        };
        return Some((integrate_agents.remove(0), reason));
    }

    if writer_agents.len() == 1 && integrate_agents.is_empty() {
        return Some((writer_agents.remove(0), "writes=true task"));
    }

    None
}

fn default_role_deps() -> HashMap<String, Vec<String>> {
    let mut map = HashMap::new();
    map.insert("consumer".into(), vec!["producer".into()]);
    map.insert(
        "judge".into(),
        vec![
            "research".into(),
            COMPUTATIONAL_RESEARCH_ROLE.into(),
            "propose".into(),
        ],
    );
    map.insert(
        "integrate".into(),
        vec![
            "judge".into(),
            "research".into(),
            COMPUTATIONAL_RESEARCH_ROLE.into(),
            "propose".into(),
        ],
    );
    map.insert("review".into(), vec!["integrate".into()]);
    map.insert("test".into(), vec!["integrate".into()]);
    map
}

fn read_workspace_role_deps(
    workspace_root: &Path,
) -> Result<Option<HashMap<String, Vec<String>>>, String> {
    let path = workspace_root.join(".nit").join("config.toml");
    if !path.exists() {
        return Ok(None);
    }
    let contents = fs::read_to_string(&path)
        .map_err(|err| format!("failed reading {}: {err}", path.display()))?;
    let value = toml::from_str::<toml::Value>(&contents)
        .map_err(|err| format!("failed parsing {}: {err}", path.display()))?;
    let table = value
        .get("swarm")
        .and_then(|value| value.get("role_deps"))
        .and_then(|value| value.as_table());
    let Some(table) = table else {
        return Ok(None);
    };

    let mut out: HashMap<String, Vec<String>> = HashMap::new();
    for (consumer, producers) in table.iter() {
        let Some(consumer) = normalize_role_label(consumer) else {
            continue;
        };
        let mut normalized = Vec::new();
        if let Some(producers) = producers.as_array() {
            for producer in producers.iter() {
                let Some(producer) = producer.as_str().and_then(normalize_role_label) else {
                    continue;
                };
                if normalized
                    .iter()
                    .any(|existing: &String| existing == &producer)
                {
                    continue;
                }
                normalized.push(producer);
            }
        } else if let Some(producer) = producers.as_str().and_then(normalize_role_label) {
            normalized.push(producer);
        } else {
            continue;
        }
        if !normalized.is_empty() {
            out.insert(consumer, normalized);
        }
    }

    if out.is_empty() {
        Ok(None)
    } else {
        Ok(Some(out))
    }
}

fn would_create_cycle(
    tasks: &[SwarmTask],
    idx_by_id: &HashMap<String, usize>,
    task_id: &str,
    dep_id: &str,
) -> bool {
    if task_id == dep_id {
        return true;
    }
    let Some(&start) = idx_by_id.get(dep_id) else {
        return false;
    };
    let Some(&target) = idx_by_id.get(task_id) else {
        return false;
    };

    let mut seen: HashSet<usize> = HashSet::new();
    let mut stack = vec![start];
    while let Some(idx) = stack.pop() {
        if idx == target {
            return true;
        }
        if !seen.insert(idx) {
            continue;
        }
        for dep in tasks[idx].deps.iter() {
            if let Some(&next) = idx_by_id.get(dep) {
                stack.push(next);
            }
        }
    }
    false
}

fn apply_role_deps(
    tasks: &mut [SwarmTask],
    role_deps: &HashMap<String, Vec<String>>,
) -> RoleDepStats {
    let mut stats = RoleDepStats::default();
    if tasks.is_empty() || role_deps.is_empty() {
        return stats;
    }

    let idx_by_id = tasks
        .iter()
        .enumerate()
        .map(|(idx, task)| (task.id.clone(), idx))
        .collect::<HashMap<_, _>>();

    let mut tasks_by_role: HashMap<String, Vec<String>> = HashMap::new();
    for task in tasks.iter() {
        if let Some(role) = task.role.as_deref().and_then(normalize_role_label) {
            tasks_by_role.entry(role).or_default().push(task.id.clone());
        }
        if task.writes {
            // Treat writer tasks as integrate-like for role-based ordering.
            let entry = tasks_by_role.entry("integrate".into()).or_default();
            if !entry.iter().any(|id| id == &task.id) {
                entry.push(task.id.clone());
            }
        }
    }

    let mut consumer_roles = role_deps.keys().cloned().collect::<Vec<_>>();
    consumer_roles.sort();
    for consumer_role in consumer_roles.iter() {
        let Some(producer_roles) = role_deps.get(consumer_role) else {
            continue;
        };
        let Some(consumer_task_ids) = tasks_by_role.get(consumer_role) else {
            continue;
        };
        if consumer_task_ids.is_empty() {
            continue;
        }
        for consumer_task_id in consumer_task_ids.iter() {
            let Some(&consumer_idx) = idx_by_id.get(consumer_task_id) else {
                continue;
            };
            for producer_role in producer_roles.iter() {
                let Some(producer_task_ids) = tasks_by_role.get(producer_role) else {
                    continue;
                };
                for producer_task_id in producer_task_ids.iter() {
                    if producer_task_id == consumer_task_id {
                        continue;
                    }
                    if tasks[consumer_idx]
                        .deps
                        .iter()
                        .any(|existing| existing == producer_task_id)
                    {
                        continue;
                    }
                    if would_create_cycle(
                        tasks,
                        &idx_by_id,
                        consumer_task_id.as_str(),
                        producer_task_id.as_str(),
                    ) {
                        stats.skipped_cycle = stats.skipped_cycle.saturating_add(1);
                        continue;
                    }
                    tasks[consumer_idx].deps.push(producer_task_id.clone());
                    stats.added = stats.added.saturating_add(1);
                }
            }
        }
    }

    stats
}

// Picks a writer count so the per-writer scope stays under the empirical
// "single-writer defers" threshold. ~12 files per writer is the largest
// scope the integrator role tolerates without self-classifying chunks as
// out-of-scope, so the formula trends toward that bucket size.
pub(super) fn recommended_writer_count(scope_file_count: usize) -> usize {
    const TARGET_FILES_PER_WRITER: usize = 12;
    const MIN_WRITERS: usize = 2;
    const MAX_WRITERS: usize = 8;
    if scope_file_count <= 15 {
        return 1;
    }
    let raw = scope_file_count.div_ceil(TARGET_FILES_PER_WRITER);
    raw.clamp(MIN_WRITERS, MAX_WRITERS)
}

// Stable partition: sort the file list, then chunk contiguously, distributing
// remainder across the first shards. Used at dispatch time (to inject the
// shard's file slice) and at compliance-check time (to scope coverage to
// the shard) so both views agree on which files a shard owns.
pub(super) fn partition_files_for_shard(
    files: &[String],
    shard_index: u8,
    shard_total: u8,
) -> Vec<String> {
    if shard_total == 0 || shard_index == 0 || shard_index > shard_total {
        return Vec::new();
    }
    let mut sorted: Vec<String> = files
        .iter()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    sorted.sort();
    sorted.dedup();
    if sorted.is_empty() {
        return Vec::new();
    }
    let n = shard_total as usize;
    let i = (shard_index - 1) as usize;
    let total = sorted.len();
    let base = total / n;
    let rem = total % n;
    let start = i * base + i.min(rem);
    let len = base + if i < rem { 1 } else { 0 };
    let end = (start + len).min(total);
    sorted[start..end].to_vec()
}

// Runtime invariant: when a Parallel-template plan covers a large scope but
// the planner produced a single integrate task, fan it into N sequential
// shards on the same writer agent. Each shard owns a deterministic slice of
// the file list (computed from `partition_files_for_shard` at dispatch and
// compliance-check time). Reviewers/testers that depended on the original
// integrate task are rewired to wait for the LAST shard so they see the
// final state. Idempotent: a plan that already has multiple integrate tasks
// (planner sharded itself) is left alone.
pub(super) fn shard_integrate_for_large_scope(
    tasks: &mut Vec<SwarmTask>,
    template: SwarmTemplate,
    scope_file_count: usize,
) -> Vec<String> {
    let mut warnings = Vec::new();
    if !matches!(template, SwarmTemplate::Parallel) {
        return warnings;
    }
    let writer_count = recommended_writer_count(scope_file_count);
    if writer_count <= 1 {
        return warnings;
    }

    let integrate_indices: Vec<usize> = tasks
        .iter()
        .enumerate()
        .filter(|(_, t)| {
            t.role.as_deref().and_then(normalize_role_label).as_deref() == Some("integrate")
        })
        .map(|(i, _)| i)
        .collect();
    if integrate_indices.len() != 1 {
        return warnings;
    }

    let original_idx = integrate_indices[0];
    if tasks[original_idx].shard_index.is_some() {
        return warnings;
    }

    let original_id = tasks[original_idx].id.clone();
    let original_title = tasks[original_idx].title.clone();
    let original_clone = tasks[original_idx].clone();

    let shard_ids: Vec<String> = (0..writer_count)
        .map(|i| format!("{original_id}-shard-{}", i + 1))
        .collect();

    for shard_i in 0..writer_count {
        let mut shard = original_clone.clone();
        shard.id = shard_ids[shard_i].clone();
        shard.title = format!(
            "{} (shard {}/{})",
            original_title.trim(),
            shard_i + 1,
            writer_count
        );
        shard.shard_index = Some(((shard_i + 1) as u8, writer_count as u8));
        if shard_i > 0 {
            shard.deps.push(shard_ids[shard_i - 1].clone());
        }
        if shard_i == 0 {
            tasks[original_idx] = shard;
        } else {
            tasks.push(shard);
        }
    }

    // `writer_count > 1` (checked above) guarantees shard_ids has ≥ 2 entries.
    let last_shard_id = shard_ids
        .last()
        .cloned()
        .expect("writer_count > 1 guarantees a final shard");
    for task in tasks.iter_mut() {
        if shard_ids.iter().any(|sid| sid == &task.id) {
            continue;
        }
        // Replace ALL occurrences (a defensive task could in theory dep on
        // the original twice via role-dep auto-wiring; rare but cheap to be
        // correct about).
        let mut changed = false;
        for dep in task.deps.iter_mut() {
            if dep == &original_id {
                *dep = last_shard_id.clone();
                changed = true;
            }
        }
        if changed {
            task.deps.sort();
            task.deps.dedup();
        }
    }

    warnings.push(format!(
        "Plan safety net: large-scope integrate task '{original_id}' fanned into {writer_count} sequential shards (scope={scope_file_count} files; ~{} files per shard). Reviewers/testers wait for the last shard.",
        scope_file_count.div_ceil(writer_count).max(1),
    ));
    warnings
}

pub(super) fn apply_role_dependency_ordering(
    workspace_root: &Path,
    role_hints_by_agent_id: &HashMap<String, String>,
    mission_kind: SwarmMissionKind,
    integrator_agent_id: Option<&str>,
    tasks: &mut [SwarmTask],
    multi_integrator: bool,
) -> Vec<String> {
    if tasks.is_empty() {
        return Vec::new();
    }

    let mut warnings = Vec::new();
    let integrator_agent_id = integrator_agent_id
        .map(str::trim)
        .filter(|id| !id.is_empty());

    validate_explicit_roles(
        tasks,
        mission_kind,
        integrator_agent_id,
        multi_integrator,
        &mut warnings,
    );
    let inferred_roles = infer_missing_roles(
        tasks,
        role_hints_by_agent_id,
        mission_kind,
        integrator_agent_id,
        multi_integrator,
        &mut warnings,
    );

    let (role_deps, source) = match read_workspace_role_deps(workspace_root) {
        Ok(Some(map)) => (map, "config"),
        Ok(None) => (default_role_deps(), "built-in"),
        Err(err) => {
            warnings.push(format!("Role ordering: {err}; using built-in role deps."));
            (default_role_deps(), "built-in")
        }
    };

    let stats = apply_role_deps(tasks, &role_deps);
    if stats.added > 0 || stats.skipped_cycle > 0 {
        warnings.push(format_role_ordering_summary(source, inferred_roles, stats));
    }

    warnings
}

fn validate_explicit_roles(
    tasks: &mut [SwarmTask],
    mission_kind: SwarmMissionKind,
    integrator_agent_id: Option<&str>,
    multi_integrator: bool,
    warnings: &mut Vec<String>,
) {
    for task in tasks.iter_mut() {
        let Some(role) = task.role.as_deref().and_then(normalize_role_label) else {
            task.role = None;
            continue;
        };
        if !role_allowed_for_mission(mission_kind, role.as_str()) {
            warnings.push(format!(
                "Role ordering: cleared role '{}' on task '{}' because mission focus '{}' does not permit that research role.",
                role,
                task.id,
                mission_kind.label()
            ));
            task.role = None;
            continue;
        }
        if role == "integrate"
            && !multi_integrator
            && integrator_agent_id.is_some_and(|integrator| task.agent_id != integrator)
        {
            warnings.push(format!(
                "Role ordering: cleared invalid integrate role on task '{}' because agent '{}' is not the integrator.",
                task.id, task.agent_id
            ));
            task.role = None;
            continue;
        }
        task.role = Some(role);
    }
}

fn infer_missing_roles(
    tasks: &mut [SwarmTask],
    role_hints_by_agent_id: &HashMap<String, String>,
    mission_kind: SwarmMissionKind,
    integrator_agent_id: Option<&str>,
    multi_integrator: bool,
    warnings: &mut Vec<String>,
) -> usize {
    let mut inferred = 0usize;
    for task in tasks.iter_mut() {
        if task.role.is_some() {
            continue;
        }
        if task.writes {
            task.role = Some("integrate".into());
            inferred = inferred.saturating_add(1);
            continue;
        }
        if let Some(role) = infer_role_from_task_id(task.id.as_str()) {
            if role == "integrate"
                && !multi_integrator
                && integrator_agent_id.is_some_and(|integrator| task.agent_id != integrator)
            {
                warnings.push(format!(
                    "Role ordering: left task '{}' without role because its id implies integrate but agent '{}' is not the integrator.",
                    task.id, task.agent_id
                ));
                continue;
            }
            task.role = Some(role.to_string());
            inferred = inferred.saturating_add(1);
            continue;
        }
        let Some(hint) = inferred_role_hint_for_agent(
            role_hints_by_agent_id,
            task.agent_id.as_str(),
            integrator_agent_id,
            mission_kind,
        ) else {
            continue;
        };
        task.role = Some(hint);
        inferred = inferred.saturating_add(1);
    }
    inferred
}

fn format_role_ordering_summary(
    source: &str,
    inferred_roles: usize,
    stats: RoleDepStats,
) -> String {
    let mut parts = Vec::new();
    if inferred_roles > 0 {
        parts.push(format!("inferred {inferred_roles} role(s)"));
    }
    if stats.added > 0 {
        parts.push(format!("added {} dep(s)", stats.added));
    }
    if stats.skipped_cycle > 0 {
        parts.push(format!("skipped {} dep(s) (cycle)", stats.skipped_cycle));
    }
    if parts.is_empty() {
        parts.push("no changes".into());
    }
    format!("Role ordering ({source}): {}.", parts.join(", "))
}

#[allow(clippy::too_many_arguments)]
pub(super) fn parse_plan_from_planner(
    planner_message: &str,
    template: SwarmTemplate,
    mission_kind: SwarmMissionKind,
    root_prompt: &str,
    available_agents: &[String],
    integrator_hint: Option<&str>,
    integrator_locked: bool,
    multi_integrator: bool,
) -> ParsedSwarmPlan {
    let Some(json) = extract_json_code_block(planner_message) else {
        return fallback_tasks(
            template,
            mission_kind,
            root_prompt,
            available_agents,
            None,
            integrator_hint,
        );
    };

    if let Ok(plan) = serde_json::from_str::<SwarmPlanV2>(&json) {
        if let Some(mut parsed) = parse_v2_plan(
            plan,
            template,
            available_agents,
            integrator_hint,
            integrator_locked,
            multi_integrator,
        ) {
            if matches!(template, SwarmTemplate::Bulk) {
                let integrator = parsed
                    .integrator_agent_id
                    .as_deref()
                    .or(integrator_hint)
                    .or_else(|| available_agents.first().map(String::as_str));
                let mut warnings = normalize_bulk_plan(&mut parsed.tasks, integrator);
                parsed.warnings.append(&mut warnings);
                if let Err(issue) = validate_bulk_plan(&parsed.tasks, available_agents, integrator)
                {
                    let mut fallback = fallback_tasks(
                        template,
                        mission_kind,
                        root_prompt,
                        available_agents,
                        Some(&issue),
                        integrator_hint,
                    );
                    fallback.warnings.push(format!(
                        "Planner did not produce a usable bulk plan; using built-in bulk workflow. Reason: {issue}"
                    ));
                    return fallback;
                }
            }
            // Lab-specific repairs: first strip proposer-to-proposer deps
            // (sequential proposers waste wall-clock time; the judge has
            // to wait for the last one regardless), then inject distinct
            // lens framings when the planner assigned multiple proposers
            // without them (N identical proposer prompts produce
            // correlated output; lens diversification forces divergent
            // outputs the judge can actually weigh). The planner guide
            // steers compliant planners; these repairs catch drift.
            if matches!(template, SwarmTemplate::Lab) {
                let mut dep_warnings = normalize_lab_plan(&mut parsed.tasks);
                parsed.warnings.append(&mut dep_warnings);
                let mut lens_warnings = apply_lab_lenses(&mut parsed.tasks);
                parsed.warnings.append(&mut lens_warnings);
            }
            return parsed;
        }
    }

    if matches!(template, SwarmTemplate::Bulk) {
        let mut fallback = fallback_tasks(
            template,
            mission_kind,
            root_prompt,
            available_agents,
            Some("Planner did not return a valid v2 bulk plan."),
            integrator_hint,
        );
        fallback.warnings.push(
            "Bulk template requires the v2 JSON schema (with deps); using built-in bulk workflow."
                .into(),
        );
        return fallback;
    }

    let Ok(plan) = serde_json::from_str::<SwarmPlanV1>(&json) else {
        return fallback_tasks(
            template,
            mission_kind,
            root_prompt,
            available_agents,
            None,
            integrator_hint,
        );
    };
    let tasks = parse_v1_tasks(plan.tasks, available_agents);

    if tasks.is_empty() {
        return fallback_tasks(
            template,
            mission_kind,
            root_prompt,
            available_agents,
            None,
            integrator_hint,
        );
    }
    let mut tasks = tasks;
    tasks.truncate(available_agents.len());

    ParsedSwarmPlan {
        tasks,
        synthesis_prompt: plan.synthesis_prompt,
        integrator_agent_id: None,
        warnings: Vec::new(),
    }
}

fn parse_v1_tasks(
    plan_tasks: Vec<super::bulk_plan::SwarmPlanTaskV1>,
    available_agents: &[String],
) -> Vec<SwarmTask> {
    let mut tasks = Vec::new();
    let mut idx = 0usize;
    let mut seen_agents = HashSet::new();
    for task in plan_tasks.into_iter() {
        let agent_id = task.agent_id.trim().to_string();
        if agent_id.is_empty() {
            continue;
        }
        if available_agents.iter().all(|id| id != &agent_id) {
            continue;
        }
        // Keep v1 deterministic: at most one task per agent id.
        if !seen_agents.insert(agent_id.clone()) {
            continue;
        }
        let title = task.title.trim().to_string();
        let prompt = task.prompt.trim().to_string();
        if title.is_empty() || prompt.is_empty() {
            continue;
        }
        idx = idx.saturating_add(1);
        tasks.push(SwarmTask {
            id: format!("task-{idx:02}"),
            agent_id,
            role: None,
            title,
            task_prompt: prompt,
            deps: Vec::new(),
            writes: false,
            artifacts: Vec::new(),
            done_when: None,
            state: SwarmTaskState::Pending,
            output: None,
            parsed_artifacts: None,
            expected_artifacts_missing: false,
            failed: false,
            retries: 0,
            compliance_missing_files: Vec::new(),
            shard_index: None,
            pre_dispatch_file_state: std::collections::HashMap::new(),
        });
    }
    tasks
}

fn parse_v2_plan(
    plan: SwarmPlanV2,
    template: SwarmTemplate,
    available_agents: &[String],
    integrator_hint: Option<&str>,
    integrator_locked: bool,
    multi_integrator: bool,
) -> Option<ParsedSwarmPlan> {
    if plan.tasks.is_empty() {
        return None;
    }
    if let Some(version) = plan.version {
        if version != 2 {
            return None;
        }
    }

    let mut warnings = Vec::new();
    let integrator = resolve_v2_integrator(
        plan.integrator_agent_id.as_deref(),
        plan.tasks.as_slice(),
        available_agents,
        integrator_hint,
        integrator_locked,
        &mut warnings,
    );
    note_template_mismatch(plan.template.as_deref(), template, &mut warnings);

    let mut tasks = Vec::new();
    let mut seen_ids = HashSet::new();
    for (idx, task) in plan.tasks.into_iter().enumerate() {
        if let Some(parsed) = parse_v2_task(
            task,
            idx,
            available_agents,
            integrator.as_deref(),
            multi_integrator,
            &mut seen_ids,
            &mut warnings,
        ) {
            tasks.push(parsed);
        }
    }

    if tasks.is_empty() {
        return None;
    }

    Some(ParsedSwarmPlan {
        tasks,
        synthesis_prompt: plan.synthesis_prompt,
        integrator_agent_id: integrator,
        warnings,
    })
}

fn resolve_v2_integrator(
    plan_integrator: Option<&str>,
    plan_tasks: &[SwarmPlanTaskV2],
    available_agents: &[String],
    integrator_hint: Option<&str>,
    integrator_locked: bool,
    warnings: &mut Vec<String>,
) -> Option<String> {
    let integrator_plan = plan_integrator.map(str::trim).filter(|id| !id.is_empty());
    let integrator_hint = integrator_hint.map(str::trim).filter(|id| !id.is_empty());
    let integrator_locked = integrator_locked && integrator_hint.is_some();

    if integrator_locked {
        if let (Some(plan_id), Some(hint_id)) = (integrator_plan, integrator_hint) {
            if !plan_id.eq_ignore_ascii_case(hint_id) {
                warnings.push(format!(
                    "Planner returned integrator_agent_id '{plan_id}' but integrator is locked to '{hint_id}'; ignoring planner override."
                ));
            }
        }
    }
    let inferred = if integrator_locked || integrator_plan.is_some() {
        None
    } else {
        infer_integrator_agent_id_from_v2_tasks(plan_tasks, available_agents)
    };
    if let Some((agent_id, reason)) = inferred.as_ref() {
        warnings.push(format!(
            "Planner omitted integrator_agent_id; inferred integrator '{agent_id}' from {reason}."
        ));
    }

    let candidate = if integrator_locked {
        integrator_hint
    } else {
        integrator_plan
            .or(inferred.as_ref().map(|(agent_id, _)| agent_id.as_str()))
            .or(integrator_hint)
    };
    candidate.and_then(|id| {
        available_agents
            .iter()
            .find(|candidate| candidate.as_str() == id)
            .map(|id| id.to_string())
    })
}

fn note_template_mismatch(
    plan_template: Option<&str>,
    swarm_template: SwarmTemplate,
    warnings: &mut Vec<String>,
) {
    let Some(label) = plan_template.map(str::trim).filter(|s| !s.is_empty()) else {
        return;
    };
    let parsed = parse_swarm_template(Some(label));
    if parsed != swarm_template {
        warnings.push(format!(
            "Planner returned template '{}' but swarm is running template '{}'; continuing with the swarm template.",
            parsed.label(),
            swarm_template.label()
        ));
    }
}

fn parse_v2_task(
    task: SwarmPlanTaskV2,
    idx: usize,
    available_agents: &[String],
    integrator: Option<&str>,
    multi_integrator: bool,
    seen_ids: &mut HashSet<String>,
    warnings: &mut Vec<String>,
) -> Option<SwarmTask> {
    let agent_id = task.agent_id.trim().to_string();
    if agent_id.is_empty() || available_agents.iter().all(|id| id != &agent_id) {
        return None;
    }

    let title = task.title.trim().to_string();
    let prompt = task.prompt.trim().to_string();
    if title.is_empty() || prompt.is_empty() {
        return None;
    }

    let id = task
        .id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToString::to_string)
        .unwrap_or_else(|| format!("task-{:02}", idx + 1));
    if !seen_ids.insert(id.clone()) {
        warnings.push(format!(
            "Duplicate task id '{id}' in planner output; skipping."
        ));
        return None;
    }

    let mut writes = task.writes;
    if writes && !multi_integrator {
        let allowed = integrator.is_some_and(|integrator| integrator == agent_id.as_str());
        if !allowed {
            writes = false;
            warnings.push(format!(
                "Planner marked task '{id}' as writes=true but agent '{agent_id}' is not the integrator; forcing read-only."
            ));
        }
    }

    let deps = task
        .deps
        .into_iter()
        .map(|dep| dep.trim().to_string())
        .filter(|dep| !dep.is_empty() && dep != &id)
        .collect::<Vec<_>>();
    // Write-role (integrate) tasks produce file modifications as output —
    // don't declare artifacts for them. Declaring artifacts injects a
    // STRUCTURED ARTIFACTS section that forces the agent to produce a JSON
    // block instead of focusing on code edits; when it doesn't, downstream
    // tasks see a misleading "artifacts missing" error.
    let artifacts = if writes {
        Vec::new()
    } else {
        task.artifacts
            .into_iter()
            .map(|a| a.trim().to_string())
            .filter(|a| !a.is_empty())
            .collect::<Vec<_>>()
    };

    Some(SwarmTask {
        id,
        agent_id,
        role: task
            .role
            .map(|r| r.trim().to_string())
            .filter(|r| !r.is_empty()),
        title,
        task_prompt: prompt,
        deps,
        writes,
        artifacts,
        done_when: task
            .done_when
            .map(|d| d.trim().to_string())
            .filter(|d| !d.is_empty()),
        state: SwarmTaskState::Pending,
        output: None,
        parsed_artifacts: None,
        expected_artifacts_missing: false,
        failed: false,
        retries: 0,
        compliance_missing_files: Vec::new(),
        shard_index: None,
        pre_dispatch_file_state: std::collections::HashMap::new(),
    })
}
