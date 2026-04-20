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
            let mut examples = self
                .unknown_deps
                .iter()
                .take(6)
                .map(|(task, dep)| format!("{task}->{dep}"))
                .collect::<Vec<_>>();
            if self.unknown_deps.len() > examples.len() {
                examples.push("…".into());
            }
            parts.push(format!(
                "unknown deps: {} ({} total)",
                examples.join(", "),
                self.unknown_deps.len()
            ));
        }

        if let Some(cycle) = self.cycle.as_ref() {
            let mut items = cycle.clone();
            if items.len() > 12 {
                items.truncate(12);
                items.push("…".into());
            }
            parts.push(format!("cycle: {}", items.join(" -> ")));
        }

        if parts.is_empty() {
            "ok".into()
        } else {
            parts.join("; ")
        }
    }
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
    let idx_by_id = tasks
        .iter()
        .enumerate()
        .map(|(idx, task)| (task.id.as_str(), idx))
        .collect::<HashMap<_, _>>();
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
    let idx_by_id = tasks
        .iter()
        .enumerate()
        .map(|(idx, task)| (task.id.as_str(), idx))
        .collect::<HashMap<_, _>>();
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

pub(super) fn repair_swarm_dag(tasks: &mut [SwarmTask]) -> Vec<String> {
    if tasks.is_empty() {
        return Vec::new();
    }

    let ids = tasks
        .iter()
        .map(|task| task.id.clone())
        .collect::<HashSet<_>>();

    let mut removed_unknown_total = 0usize;
    let mut removed_unknown_examples: Vec<(String, String)> = Vec::new();
    let mut removed_dupe_total = 0usize;
    for task in tasks.iter_mut() {
        let mut seen: HashSet<String> = HashSet::new();
        task.deps.retain(|dep| {
            if dep == &task.id {
                return false;
            }
            if !ids.contains(dep) {
                removed_unknown_total = removed_unknown_total.saturating_add(1);
                if removed_unknown_examples.len() < 6 {
                    removed_unknown_examples.push((task.id.clone(), dep.clone()));
                }
                return false;
            }
            if !seen.insert(dep.clone()) {
                removed_dupe_total = removed_dupe_total.saturating_add(1);
                return false;
            }
            true
        });
    }

    let mut removed_cycle_total = 0usize;
    let mut removed_cycle_examples: Vec<(String, String)> = Vec::new();
    while let Some((task_idx, dep_id)) = find_swarm_cycle_back_edge(tasks) {
        let Some(pos) = tasks[task_idx].deps.iter().position(|dep| dep == &dep_id) else {
            break;
        };
        tasks[task_idx].deps.remove(pos);
        removed_cycle_total = removed_cycle_total.saturating_add(1);
        if removed_cycle_examples.len() < 6 {
            removed_cycle_examples.push((tasks[task_idx].id.clone(), dep_id));
        }
    }

    let mut warnings = Vec::new();
    if removed_unknown_total > 0 {
        let examples = removed_unknown_examples
            .into_iter()
            .map(|(task, dep)| format!("{task}->{dep}"))
            .collect::<Vec<_>>()
            .join(", ");
        warnings.push(format!(
            "DAG repair: removed {removed_unknown_total} unknown dep(s){}",
            if examples.is_empty() {
                ".".into()
            } else {
                format!(" (examples: {examples}).")
            }
        ));
    }
    if removed_dupe_total > 0 {
        warnings.push(format!(
            "DAG repair: removed {removed_dupe_total} duplicate dep(s)."
        ));
    }
    if removed_cycle_total > 0 {
        let examples = removed_cycle_examples
            .into_iter()
            .map(|(task, dep)| format!("{task}->{dep}"))
            .collect::<Vec<_>>()
            .join(", ");
        warnings.push(format!(
            "DAG repair: removed {removed_cycle_total} dep(s) to break cycle(s){}",
            if examples.is_empty() {
                ".".into()
            } else {
                format!(" (examples: {examples}).")
            }
        ));
    }

    warnings
}

// Parallel-only auto-repair: when a writer task has unresolved dep ids AND
// zero resolved deps, redirect its deps to every propose/research task.
// Recovers the common failure mode where the planner writes
// `integrate.deps = ["judge"]` against a parallel template that has no judge
// phase. Non-writer tasks, and writers with any resolved dep, are left alone —
// they surface through the Layer 1 warning path instead. Returns a per-repair
// description; the caller emits one substrate signal per entry for traceability.
pub(super) fn ensure_deps_resolve(tasks: &mut [SwarmTask], template: SwarmTemplate) -> Vec<String> {
    if !matches!(template, SwarmTemplate::Parallel) {
        return Vec::new();
    }
    let task_ids: HashSet<String> = tasks.iter().map(|t| t.id.clone()).collect();
    let propose_ids: Vec<String> = tasks
        .iter()
        .filter(|t| {
            t.role
                .as_deref()
                .and_then(normalize_role_label)
                .map(|r| {
                    matches!(
                        r.as_str(),
                        "propose" | "research" | COMPUTATIONAL_RESEARCH_ROLE
                    )
                })
                .unwrap_or(false)
        })
        .map(|t| t.id.clone())
        .collect();
    if propose_ids.is_empty() {
        return Vec::new();
    }
    let mut repairs = Vec::new();
    for task in tasks.iter_mut() {
        if !task.writes || task.deps.is_empty() {
            continue;
        }
        let has_resolved = task.deps.iter().any(|d| task_ids.contains(d.as_str()));
        if has_resolved {
            continue;
        }
        let original_deps = task.deps.join(",");
        task.deps = propose_ids.clone();
        repairs.push(format!(
            "parallel auto-repair: {} deps [{}] unresolved -> redirected to propose tasks {:?}",
            task.id, original_deps, propose_ids
        ));
    }
    repairs
}
