use std::collections::HashSet;

use nit_core::AppState;

use super::SwarmRun;

#[derive(Clone, Debug)]
pub(super) struct UnresolvedDep {
    pub(super) task_id: String,
    pub(super) task_role: Option<String>,
    pub(super) missing_dep: String,
}

/// Walks all tasks in the run and returns every dep id that doesn't
/// resolve to another task in the same run. Used by the dispatcher to
/// surface malformed plans via substrate Warning signals (Layer 1).
pub(super) fn collect_unresolved_deps(run: &SwarmRun) -> Vec<UnresolvedDep> {
    let task_ids: HashSet<&str> = run.tasks.iter().map(|t| t.id.as_str()).collect();
    let mut out = Vec::new();
    for task in &run.tasks {
        for dep in &task.deps {
            if !task_ids.contains(dep.as_str()) {
                out.push(UnresolvedDep {
                    task_id: task.id.clone(),
                    task_role: task.role.clone(),
                    missing_dep: dep.clone(),
                });
            }
        }
    }
    out
}

/// Emit a Warning signal per unresolved dep (dedup against the last 5
/// generations of matching signals). posted_by encodes the planner agent
/// id so the sparse_plan observer can group by planner.
pub(super) fn emit_unresolved_dep_signals(state: &mut AppState, run: &SwarmRun) {
    let unresolved = collect_unresolved_deps(run);
    if unresolved.is_empty() {
        return;
    }
    let posted_by = format!("planner:{}", run.planner_agent_id);
    let current_gen = state.substrate.current_generation();
    let window_start = current_gen.saturating_sub(5);
    for dep in unresolved {
        let already_emitted = state.substrate.signals.values().any(|s| {
            s.kind == nit_core::substrate::SignalKind::Warning
                && s.posted_by == posted_by
                && s.posted_at_gen >= window_start
                && s.payload.get("reason").and_then(|v| v.as_str()) == Some("unresolved_dep")
                && s.payload.get("task_id").and_then(|v| v.as_str()) == Some(dep.task_id.as_str())
                && s.payload.get("missing_dep").and_then(|v| v.as_str())
                    == Some(dep.missing_dep.as_str())
        });
        if already_emitted {
            continue;
        }
        let id = state.substrate.next_signal_id(&posted_by);
        let posted_at_gen = state.substrate.current_generation();
        state.substrate.emit_signal(nit_core::substrate::Signal {
            id,
            kind: nit_core::substrate::SignalKind::Warning,
            posted_by: posted_by.clone(),
            posted_at_gen,
            target: nit_core::substrate::SignalTarget::Agent {
                agent_id: run.planner_agent_id.clone(),
            },
            initial_strength: nit_core::substrate::SubstrateState::DEFAULT_INITIAL_STRENGTH,
            payload: serde_json::json!({
                "reason": "unresolved_dep",
                "task_id": dep.task_id,
                "task_role": dep.task_role,
                "missing_dep": dep.missing_dep,
                "mission_id": run.mission_id,
                "template": run.template.label(),
            }),
        });
    }
}

/// Emit a Warning signal per auto-repair description produced by
/// `ensure_deps_resolve`. Lower initial strength (0.8) so the repair
/// trace fades faster than the raw unresolved-dep warnings it stems from.
pub(super) fn emit_parallel_deps_auto_repair_signals(
    state: &mut AppState,
    planner_agent_id: &str,
    mission_id: &str,
    template_label: &str,
    repairs: &[String],
) {
    if repairs.is_empty() {
        return;
    }
    let posted_by = format!("planner:{planner_agent_id}");
    for desc in repairs {
        let id = state.substrate.next_signal_id(&posted_by);
        let posted_at_gen = state.substrate.current_generation();
        state.substrate.emit_signal(nit_core::substrate::Signal {
            id,
            kind: nit_core::substrate::SignalKind::Warning,
            posted_by: posted_by.clone(),
            posted_at_gen,
            target: nit_core::substrate::SignalTarget::Agent {
                agent_id: planner_agent_id.to_string(),
            },
            initial_strength: 0.8,
            payload: serde_json::json!({
                "reason": "parallel_deps_auto_repaired",
                "description": desc,
                "mission_id": mission_id,
                "template": template_label,
            }),
        });
    }
}
