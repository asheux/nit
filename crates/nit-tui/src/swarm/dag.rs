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

// Parallel-only auto-repair, three passes:
//   1. Writer tasks (`writes=true`) with unresolved deps AND zero resolved
//      deps → redirect deps to every propose/research task. Recovers the
//      common failure mode where the planner writes
//      `integrate.deps = ["judge"]` against a parallel template that has
//      no judge phase.
//   2. Verifier tasks (`test`/`review`) with EMPTY deps → wire deps to all
//      `integrate` tasks. Without this guard a `test` task starts in
//      `Ready` state alongside the proposers and dispatches before any
//      writer has run — the operator sees the test agent fire pre-plan
//      and report nothing-to-test. The planner prompt steers compliant
//      planners to set these deps, but this pass catches drift.
//   3. `judge` tasks with EMPTY deps → wire deps to all propose/research
//      tasks (analogous to integrate's fan-in).
// Writers with any resolved dep are left alone — they surface through the
// Layer 1 warning path instead. Returns a per-repair description; the
// caller emits one substrate signal per entry for traceability.
pub(super) fn ensure_deps_resolve(tasks: &mut [SwarmTask], template: SwarmTemplate) -> Vec<String> {
    if !matches!(template, SwarmTemplate::Parallel) {
        return Vec::new();
    }
    let task_ids: HashSet<String> = tasks.iter().map(|t| t.id.clone()).collect();
    let propose_ids: Vec<String> = collect_role_ids(tasks, |role| {
        matches!(
            role,
            "propose" | "research" | COMPUTATIONAL_RESEARCH_ROLE
        )
    });
    let integrate_ids: Vec<String> =
        collect_role_ids(tasks, |role| matches!(role, "integrate"));
    let mut repairs = Vec::new();

    // Pass 1: writers with all-unresolved deps → redirect to proposers.
    if !propose_ids.is_empty() {
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
    }

    // Pass 2: test / review with empty deps → wire to integrate tasks so
    // verifiers don't start before any writer has produced output.
    if !integrate_ids.is_empty() {
        for task in tasks.iter_mut() {
            if !task.deps.is_empty() {
                continue;
            }
            let role = task.role.as_deref().and_then(normalize_role_label);
            let is_verifier = matches!(role.as_deref(), Some("test") | Some("review"));
            if !is_verifier {
                continue;
            }
            // Don't self-dep — the integrate task itself sometimes
            // carries a `review`/`test` secondary intent in plans the
            // operator hand-edits; skip wiring deps that would point
            // to the task's own id.
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

    // Pass 3: judge with empty deps → wire to proposers (judges fan in
    // over proposer outputs in the same shape integrate would).
    if !propose_ids.is_empty() {
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

    repairs
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

#[cfg(test)]
mod ensure_deps_tests {
    use super::*;
    use crate::swarm::SwarmTaskState;

    fn task(id: &str, role: &str, deps: Vec<&str>, writes: bool) -> SwarmTask {
        SwarmTask {
            id: id.into(),
            agent_id: id.into(),
            role: Some(role.into()),
            title: id.into(),
            task_prompt: String::new(),
            deps: deps.into_iter().map(String::from).collect(),
            writes,
            artifacts: Vec::new(),
            done_when: None,
            state: SwarmTaskState::Pending,
            output: None,
            parsed_artifacts: None,
            expected_artifacts_missing: false,
            failed: false,
            retries: 0,
        }
    }

    #[test]
    fn parallel_test_role_with_empty_deps_gets_wired_to_integrate() {
        // Reproduces the operator-reported bug: planner emits a `test`
        // task with no deps under the parallel template; without this
        // repair, the test agent dispatches before the integrator and
        // fires against an unchanged tree.
        let mut tasks = vec![
            task("propose-01", "propose", vec![], false),
            task("integrate-01", "integrate", vec!["propose-01"], true),
            task("test-01", "test", vec![], false),
        ];
        let repairs = ensure_deps_resolve(&mut tasks, SwarmTemplate::Parallel);
        assert_eq!(tasks[2].deps, vec!["integrate-01"]);
        assert!(
            repairs.iter().any(|r| r.contains("verifier test-01")),
            "expected a per-repair description, got {repairs:?}",
        );
    }

    #[test]
    fn parallel_review_role_with_empty_deps_gets_wired_to_integrate() {
        let mut tasks = vec![
            task("propose-01", "propose", vec![], false),
            task("integrate-01", "integrate", vec!["propose-01"], true),
            task("integrate-02", "integrate", vec!["propose-01"], true),
            task("review-01", "review", vec![], false),
        ];
        let _ = ensure_deps_resolve(&mut tasks, SwarmTemplate::Parallel);
        let mut wired = tasks[3].deps.clone();
        wired.sort();
        assert_eq!(wired, vec!["integrate-01", "integrate-02"]);
    }

    #[test]
    fn parallel_judge_role_with_empty_deps_gets_wired_to_proposers() {
        let mut tasks = vec![
            task("propose-01", "propose", vec![], false),
            task("propose-02", "propose", vec![], false),
            task("judge-01", "judge", vec![], false),
        ];
        let _ = ensure_deps_resolve(&mut tasks, SwarmTemplate::Parallel);
        let mut wired = tasks[2].deps.clone();
        wired.sort();
        assert_eq!(wired, vec!["propose-01", "propose-02"]);
    }

    #[test]
    fn parallel_verifier_with_explicit_deps_is_left_alone() {
        // Plans that already wire deps must NOT be rewritten — the
        // operator's intent (which integrate task this verifier covers)
        // wins over the auto-fan-out heuristic.
        let mut tasks = vec![
            task("integrate-01", "integrate", vec![], true),
            task("integrate-02", "integrate", vec![], true),
            task("test-01", "test", vec!["integrate-01"], false),
        ];
        let _ = ensure_deps_resolve(&mut tasks, SwarmTemplate::Parallel);
        assert_eq!(tasks[2].deps, vec!["integrate-01"]);
    }

    #[test]
    fn lab_template_unaffected_by_verifier_repair() {
        let mut tasks = vec![
            task("integrate-01", "integrate", vec![], true),
            task("test-01", "test", vec![], false),
        ];
        let repairs = ensure_deps_resolve(&mut tasks, SwarmTemplate::Lab);
        assert!(repairs.is_empty());
        assert!(tasks[1].deps.is_empty());
    }
}
