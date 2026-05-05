use std::collections::{HashMap, HashSet};

use super::{normalize_role_label, SwarmTask, SwarmTemplate, COMPUTATIONAL_RESEARCH_ROLE};

#[derive(Clone, Debug, Default)]
pub(super) struct SwarmDagIssues {
    pub(super) unknown_deps: Vec<(String, String)>,
    pub(super) cycle: Option<Vec<String>>,
}

impl SwarmDagIssues {
    pub(super) fn is_empty(&self) -> bool {
        self.unknown_deps.is_empty() && self.cycle.is_none()
    }

    pub(super) fn summary(&self) -> String {
        let mut parts = Vec::new();
        if !self.unknown_deps.is_empty() {
            parts.push(self.format_unknown_deps());
        }
        if let Some(cycle) = self.cycle.as_ref() {
            parts.push(format_cycle(cycle));
        }
        if parts.is_empty() {
            "ok".into()
        } else {
            parts.join("; ")
        }
    }

    fn format_unknown_deps(&self) -> String {
        let mut examples = self
            .unknown_deps
            .iter()
            .take(6)
            .map(|(task, dep)| format!("{task}->{dep}"))
            .collect::<Vec<_>>();
        if self.unknown_deps.len() > examples.len() {
            examples.push("…".into());
        }
        format!(
            "unknown deps: {} ({} total)",
            examples.join(", "),
            self.unknown_deps.len()
        )
    }
}

fn format_cycle(cycle: &[String]) -> String {
    let mut items = cycle.to_vec();
    if items.len() > 12 {
        items.truncate(12);
        items.push("…".into());
    }
    format!("cycle: {}", items.join(" -> "))
}

pub(super) fn analyze_swarm_dag(tasks: &[SwarmTask]) -> SwarmDagIssues {
    let mut issues = SwarmDagIssues::default();
    if tasks.is_empty() {
        return issues;
    }

    let ids = tasks
        .iter()
        .map(|task| task.id.as_str())
        .collect::<HashSet<_>>();
    for task in tasks.iter() {
        for dep in task.deps.iter() {
            if !ids.contains(dep.as_str()) {
                issues.unknown_deps.push((task.id.clone(), dep.clone()));
            }
        }
    }

    issues.cycle = find_swarm_cycle_path(tasks);
    issues
}

pub(super) fn find_swarm_cycle_path(tasks: &[SwarmTask]) -> Option<Vec<String>> {
    if tasks.is_empty() {
        return None;
    }
    let idx_by_id = build_idx_by_id(tasks);
    let mut state = vec![0u8; tasks.len()];
    let mut stack: Vec<usize> = Vec::new();
    let mut on_stack = vec![false; tasks.len()];

    fn dfs(
        v: usize,
        tasks: &[SwarmTask],
        idx_by_id: &HashMap<&str, usize>,
        state: &mut [u8],
        stack: &mut Vec<usize>,
        on_stack: &mut [bool],
    ) -> Option<Vec<String>> {
        state[v] = 1;
        stack.push(v);
        on_stack[v] = true;

        for dep in tasks[v].deps.iter() {
            let Some(&u) = idx_by_id.get(dep.as_str()) else {
                continue;
            };
            if state[u] == 0 {
                if let Some(cycle) = dfs(u, tasks, idx_by_id, state, stack, on_stack) {
                    return Some(cycle);
                }
            } else if on_stack[u] {
                let Some(pos) = stack.iter().position(|&idx| idx == u) else {
                    continue;
                };
                let mut cycle = stack[pos..]
                    .iter()
                    .map(|&idx| tasks[idx].id.clone())
                    .collect::<Vec<_>>();
                cycle.push(tasks[u].id.clone());
                return Some(cycle);
            }
        }

        stack.pop();
        on_stack[v] = false;
        state[v] = 2;
        None
    }

    for v in 0..tasks.len() {
        if state[v] != 0 {
            continue;
        }
        if let Some(cycle) = dfs(v, tasks, &idx_by_id, &mut state, &mut stack, &mut on_stack) {
            return Some(cycle);
        }
    }
    None
}

fn find_swarm_cycle_back_edge(tasks: &[SwarmTask]) -> Option<(usize, String)> {
    if tasks.is_empty() {
        return None;
    }
    let idx_by_id = build_idx_by_id(tasks);
    let mut state = vec![0u8; tasks.len()];
    let mut on_stack = vec![false; tasks.len()];

    fn dfs(
        v: usize,
        tasks: &[SwarmTask],
        idx_by_id: &HashMap<&str, usize>,
        state: &mut [u8],
        on_stack: &mut [bool],
    ) -> Option<(usize, String)> {
        state[v] = 1;
        on_stack[v] = true;

        for dep in tasks[v].deps.iter() {
            let Some(&u) = idx_by_id.get(dep.as_str()) else {
                continue;
            };
            if state[u] == 0 {
                if let Some(edge) = dfs(u, tasks, idx_by_id, state, on_stack) {
                    return Some(edge);
                }
            } else if on_stack[u] {
                return Some((v, dep.clone()));
            }
        }

        on_stack[v] = false;
        state[v] = 2;
        None
    }

    for v in 0..tasks.len() {
        if state[v] != 0 {
            continue;
        }
        if let Some(edge) = dfs(v, tasks, &idx_by_id, &mut state, &mut on_stack) {
            return Some(edge);
        }
    }
    None
}

fn build_idx_by_id(tasks: &[SwarmTask]) -> HashMap<&str, usize> {
    tasks
        .iter()
        .enumerate()
        .map(|(idx, task)| (task.id.as_str(), idx))
        .collect()
}

#[derive(Default)]
struct DepRepairTally {
    unknown_total: usize,
    unknown_examples: Vec<(String, String)>,
    dupe_total: usize,
}

fn dedupe_and_drop_unknown_deps(
    tasks: &mut [SwarmTask],
    known_ids: &HashSet<String>,
) -> DepRepairTally {
    let mut tally = DepRepairTally::default();
    for task in tasks.iter_mut() {
        let mut seen: HashSet<String> = HashSet::new();
        task.deps.retain(|dep| {
            if dep == &task.id {
                return false;
            }
            if !known_ids.contains(dep) {
                tally.unknown_total = tally.unknown_total.saturating_add(1);
                if tally.unknown_examples.len() < 6 {
                    tally.unknown_examples.push((task.id.clone(), dep.clone()));
                }
                return false;
            }
            if !seen.insert(dep.clone()) {
                tally.dupe_total = tally.dupe_total.saturating_add(1);
                return false;
            }
            true
        });
    }
    tally
}

fn break_dependency_cycles(tasks: &mut [SwarmTask]) -> (usize, Vec<(String, String)>) {
    let mut total = 0usize;
    let mut examples: Vec<(String, String)> = Vec::new();
    while let Some((task_idx, dep_id)) = find_swarm_cycle_back_edge(tasks) {
        let Some(pos) = tasks[task_idx].deps.iter().position(|dep| dep == &dep_id) else {
            break;
        };
        tasks[task_idx].deps.remove(pos);
        total = total.saturating_add(1);
        if examples.len() < 6 {
            examples.push((tasks[task_idx].id.clone(), dep_id));
        }
    }
    (total, examples)
}

fn format_repair_examples(examples: Vec<(String, String)>) -> String {
    examples
        .into_iter()
        .map(|(task, dep)| format!("{task}->{dep}"))
        .collect::<Vec<_>>()
        .join(", ")
}

fn warning_with_examples(prefix: &str, examples: Vec<(String, String)>) -> String {
    let body = format_repair_examples(examples);
    if body.is_empty() {
        format!("{prefix}.")
    } else {
        format!("{prefix} (examples: {body}).")
    }
}

pub(super) fn repair_swarm_dag(tasks: &mut [SwarmTask]) -> Vec<String> {
    if tasks.is_empty() {
        return Vec::new();
    }

    let ids = tasks
        .iter()
        .map(|task| task.id.clone())
        .collect::<HashSet<_>>();
    let tally = dedupe_and_drop_unknown_deps(tasks, &ids);
    let (cycle_total, cycle_examples) = break_dependency_cycles(tasks);

    let mut warnings = Vec::new();
    if tally.unknown_total > 0 {
        warnings.push(warning_with_examples(
            &format!("DAG repair: removed {} unknown dep(s)", tally.unknown_total),
            tally.unknown_examples,
        ));
    }
    if tally.dupe_total > 0 {
        warnings.push(format!(
            "DAG repair: removed {} duplicate dep(s).",
            tally.dupe_total
        ));
    }
    if cycle_total > 0 {
        warnings.push(warning_with_examples(
            &format!("DAG repair: removed {cycle_total} dep(s) to break cycle(s)"),
            cycle_examples,
        ));
    }
    warnings
}

// Parallel-only auto-repair, three passes:
//   1. Writer tasks (`writes=true`) with unresolved deps AND zero resolved
//      deps → redirect deps to every propose/research task. Recovers the
//      common failure mode where the planner writes
//      `integrate.deps = ["judge"]` against a parallel template that has
//      no judge phase.
//   2. Verifier tasks (`test`/`review`) with EMPTY deps → wire deps to all
//      `integrate` tasks. Without this guard a `test` task starts in `Ready`
//      state alongside the proposers and dispatches before any writer has
//      run — the operator sees the test agent fire pre-plan and report
//      nothing-to-test.
//   3. `judge` tasks with EMPTY deps → wire deps to all propose/research
//      tasks (analogous to integrate's fan-in).
// Writers with any resolved dep are left alone — they surface through the
// Layer 1 warning path instead.
pub(super) fn ensure_deps_resolve(tasks: &mut [SwarmTask], template: SwarmTemplate) -> Vec<String> {
    if !matches!(template, SwarmTemplate::Parallel) {
        return Vec::new();
    }
    let task_ids: HashSet<String> = tasks.iter().map(|t| t.id.clone()).collect();
    let propose_ids = collect_role_ids(tasks, is_proposer_role);
    let integrate_ids = collect_role_ids(tasks, |role| role == "integrate");

    let mut repairs = Vec::new();
    redirect_unresolved_writers(tasks, &task_ids, &propose_ids, &mut repairs);
    wire_empty_verifiers_to_integrate(tasks, &integrate_ids, &mut repairs);
    wire_empty_judges_to_proposers(tasks, &propose_ids, &mut repairs);
    repairs
}

fn is_proposer_role(role: &str) -> bool {
    matches!(role, "propose" | "research" | COMPUTATIONAL_RESEARCH_ROLE)
}

fn redirect_unresolved_writers(
    tasks: &mut [SwarmTask],
    task_ids: &HashSet<String>,
    propose_ids: &[String],
    repairs: &mut Vec<String>,
) {
    if propose_ids.is_empty() {
        return;
    }
    for task in tasks.iter_mut() {
        if !task.writes || task.deps.is_empty() {
            continue;
        }
        let has_resolved = task.deps.iter().any(|d| task_ids.contains(d.as_str()));
        if has_resolved {
            continue;
        }
        let original_deps = task.deps.join(",");
        task.deps = propose_ids.to_vec();
        repairs.push(format!(
            "parallel auto-repair: {} deps [{}] unresolved -> redirected to propose tasks {:?}",
            task.id, original_deps, propose_ids
        ));
    }
}

fn wire_empty_verifiers_to_integrate(
    tasks: &mut [SwarmTask],
    integrate_ids: &[String],
    repairs: &mut Vec<String>,
) {
    if integrate_ids.is_empty() {
        return;
    }
    for task in tasks.iter_mut() {
        if !task.deps.is_empty() {
            continue;
        }
        let role = task.role.as_deref().and_then(normalize_role_label);
        if !matches!(role.as_deref(), Some("test") | Some("review")) {
            continue;
        }
        // Skip self-deps — a verifier task carrying integrate as a secondary
        // intent in hand-edited plans must not point to its own id.
        let deps: Vec<String> = integrate_ids
            .iter()
            .filter(|id| id.as_str() != task.id.as_str())
            .cloned()
            .collect();
        if deps.is_empty() {
            continue;
        }
        task.deps = deps.clone();
        repairs.push(format!(
            "parallel auto-repair: verifier {} (role={}) had empty deps -> wired to integrate tasks {:?}",
            task.id,
            role.as_deref().unwrap_or("?"),
            deps,
        ));
    }
}

fn wire_empty_judges_to_proposers(
    tasks: &mut [SwarmTask],
    propose_ids: &[String],
    repairs: &mut Vec<String>,
) {
    if propose_ids.is_empty() {
        return;
    }
    for task in tasks.iter_mut() {
        if !task.deps.is_empty() {
            continue;
        }
        let role = task.role.as_deref().and_then(normalize_role_label);
        if role.as_deref() != Some("judge") {
            continue;
        }
        let deps: Vec<String> = propose_ids
            .iter()
            .filter(|id| id.as_str() != task.id.as_str())
            .cloned()
            .collect();
        if deps.is_empty() {
            continue;
        }
        task.deps = deps.clone();
        repairs.push(format!(
            "parallel auto-repair: judge {} had empty deps -> wired to propose tasks {:?}",
            task.id, deps,
        ));
    }
}

fn collect_role_ids(tasks: &[SwarmTask], role_match: impl Fn(&str) -> bool) -> Vec<String> {
    tasks
        .iter()
        .filter(|t| {
            t.role
                .as_deref()
                .and_then(normalize_role_label)
                .map(|r| role_match(r.as_str()))
                .unwrap_or(false)
        })
        .map(|t| t.id.clone())
        .collect()
}
