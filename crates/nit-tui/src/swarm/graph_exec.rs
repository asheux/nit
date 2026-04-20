use std::collections::HashSet;

use nit_core::AppState;

use super::{
    dependency_payload_text, dependency_payload_text_full, find_swarm_cycle_path,
    merge_task_artifacts, normalize_role_label, parse_task_artifacts, truncate_chars,
    wrap_task_prompt, SwarmDispatch, SwarmRun, SwarmStage, SwarmTask, SwarmTaskState,
    SwarmTemplate, SWARM_DEP_OUTPUT_MAX_CHARS, SWARM_DEP_OUTPUT_MAX_CHARS_FULL,
    SWARM_DEP_OUTPUT_TOTAL_MAX_CHARS_FULL,
};

pub(super) fn initialize_task_graph(run: &mut SwarmRun) {
    let ids = run
        .tasks
        .iter()
        .map(|task| task.id.clone())
        .collect::<HashSet<_>>();
    for task in run.tasks.iter_mut() {
        let mut seen = HashSet::new();
        task.deps.retain(|dep| {
            if dep == &task.id {
                return false;
            }
            if !ids.contains(dep) {
                return false;
            }
            seen.insert(dep.clone())
        });
        if task.state.is_terminal() {
            continue;
        }
        task.state = if task.deps.is_empty() {
            SwarmTaskState::Ready
        } else {
            SwarmTaskState::Pending
        };
    }
}

pub(super) fn tasks_terminal_count(tasks: &[SwarmTask]) -> usize {
    tasks.iter().filter(|task| task.state.is_terminal()).count()
}

pub(super) fn mark_task_running(run: &mut SwarmRun, agent_id: &str) {
    if !matches!(run.stage, SwarmStage::Executing) {
        return;
    }
    if let Some(task) = run
        .tasks
        .iter_mut()
        .find(|task| task.agent_id == agent_id && matches!(task.state, SwarmTaskState::Dispatched))
    {
        task.state = SwarmTaskState::Running;
    }
}

pub(super) struct TaskCompletion {
    pub(super) task_id: String,
    pub(super) expected_artifacts_missing: bool,
    /// Task declared `writes: true` — it was supposed to modify files.
    pub(super) writes_expected: bool,
    /// At least one file was attributed to this agent via `FileWrite` events.
    pub(super) writes_detected: bool,
}

/// Post-integrate structural compliance: walks the integrator's proposer/
/// judge dependencies, collects declared file paths from their parsed
/// artifacts, and returns any that the integrator did not actually touch
/// (not on disk, or not in `genome_mission_modified`). Empty return means
/// the integrator honored the declared file outputs; non-empty means the
/// proposer plan specified files the writer silently skipped.
pub(super) fn structural_compliance_missing_files(
    run: &SwarmRun,
    integrator_task_id: &str,
    state: &AppState,
) -> Vec<String> {
    let Some(task) = run.tasks.iter().find(|t| t.id == integrator_task_id) else {
        return Vec::new();
    };
    if !task.writes {
        return Vec::new();
    }
    let mission_writes: Option<&HashSet<std::path::PathBuf>> =
        state.genome_mission_modified.get(&run.mission_id);
    let workspace = state.workspace_root.as_path();
    let mut missing: Vec<String> = Vec::new();
    let mut declared_count = 0usize;
    for dep_id in task.deps.iter() {
        let Some(dep) = run.tasks.iter().find(|t| &t.id == dep_id) else {
            continue;
        };
        let role = dep.role.as_deref().and_then(normalize_role_label);
        if !matches!(role.as_deref(), Some("propose") | Some("judge")) {
            continue;
        }
        let Some(artifacts) = dep.parsed_artifacts.as_ref() else {
            continue;
        };
        for entry in artifacts.files.iter() {
            let rel = entry.path.trim();
            if rel.is_empty() {
                continue;
            }
            declared_count += 1;
            let abs = if std::path::Path::new(rel).is_absolute() {
                std::path::PathBuf::from(rel)
            } else {
                workspace.join(rel)
            };
            let touched = mission_writes
                .map(|set| set.contains(&abs))
                .unwrap_or(false);
            if !touched {
                missing.push(rel.to_string());
            }
        }
    }
    // If there were no declarations at all, don't flag — proposers aren't
    // required to enumerate every file they recommend (it's stronger when
    // they do, but absence isn't non-compliance).
    if declared_count == 0 {
        return Vec::new();
    }
    missing
}

pub(super) fn mark_task_finished(
    run: &mut SwarmRun,
    agent_id: &str,
    message: String,
    failed: bool,
    agent_has_file_writes: bool,
) -> Option<TaskCompletion> {
    // Look for an active (Running or Dispatched) task first.
    let pos_active = run
        .tasks
        .iter()
        .position(|task| task.agent_id == agent_id && matches!(task.state, SwarmTaskState::Running))
        .or_else(|| {
            run.tasks.iter().position(|task| {
                task.agent_id == agent_id && matches!(task.state, SwarmTaskState::Dispatched)
            })
        });

    // Fall back to an already-finished task so late/intermediate responses
    // can still append artifacts.
    let pos_finished = || {
        run.tasks.iter().position(|task| {
            task.agent_id == agent_id
                && matches!(task.state, SwarmTaskState::Done | SwarmTaskState::Failed)
        })
    };
    let pos = pos_active.or_else(pos_finished)?;
    let already_finished = pos_active.is_none();

    let parsed_artifacts = parse_task_artifacts(&run.tasks[pos].id, &message);
    // Write-role tasks (integrate) produce file modifications as their primary
    // output — the structured artifacts JSON is optional metadata.  Only flag
    // missing artifacts for read-only tasks where the JSON is the sole output.
    let expected_artifacts_missing = !run.tasks[pos].artifacts.is_empty()
        && parsed_artifacts.is_none()
        && !run.tasks[pos].writes;

    let task = &mut run.tasks[pos];

    if already_finished {
        // Append output to existing response.
        if let Some(existing) = task.output.as_mut() {
            existing.push_str("\n\n---\n\n");
            existing.push_str(&message);
        } else {
            task.output = Some(message);
        }
    } else {
        // Reporting-failure rescue: if the subprocess exited non-zero but the
        // task is a write-role task AND FileWrite events fired for this agent,
        // the work likely landed on disk and the crash was in the agent's
        // final summary step (classic end-of-turn context overflow). Downgrade
        // to Done so the swarm doesn't discard a completed refactor. The
        // original failure message is kept in `output` for inspection.
        let reporting_failure_rescue = failed && task.writes && agent_has_file_writes;
        let effective_failed = failed && !reporting_failure_rescue;
        task.output = Some(if reporting_failure_rescue {
            format!(
                "(rescue) subprocess reported failure but FileWrite events fired for this agent — \
treating as success. Inspect on-disk artifacts to confirm.\n\n\
original failure message:\n{message}"
            )
        } else {
            message
        });
        task.failed = effective_failed;
        task.state = if effective_failed {
            SwarmTaskState::Failed
        } else {
            SwarmTaskState::Done
        };
        task.expected_artifacts_missing = expected_artifacts_missing;
    }

    // Merge artifacts with any previously collected ones instead of overwriting.
    match (task.parsed_artifacts.as_mut(), parsed_artifacts) {
        (Some(existing), Some(new)) => merge_task_artifacts(existing, new),
        (None, new @ Some(_)) => task.parsed_artifacts = new,
        _ => {}
    }

    let writes_expected = task.writes;

    Some(TaskCompletion {
        task_id: task.id.clone(),
        expected_artifacts_missing: if already_finished {
            false
        } else {
            expected_artifacts_missing
        },
        writes_expected,
        writes_detected: agent_has_file_writes,
    })
}

pub(super) fn refresh_task_readiness(run: &mut SwarmRun) {
    let all_ids = run
        .tasks
        .iter()
        .map(|task| task.id.clone())
        .collect::<HashSet<_>>();
    let terminal_ids = run
        .tasks
        .iter()
        .filter(|task| task.state.is_terminal())
        .map(|task| task.id.clone())
        .collect::<HashSet<_>>();
    for task in run.tasks.iter_mut() {
        if !matches!(task.state, SwarmTaskState::Pending) {
            continue;
        }
        let ready = task.deps.iter().all(|dep| {
            terminal_ids
                .contains(dep)
                // Unknown dep shouldn't happen after sanitize; treat as satisfied to avoid deadlocks.
                || !all_ids.contains(dep)
        });
        if ready {
            task.state = SwarmTaskState::Ready;
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct SwarmDeadlock {
    pub(super) skipped: Vec<String>,
    pub(super) message: String,
}

pub(super) fn maybe_resolve_deadlock(run: &mut SwarmRun) -> Option<SwarmDeadlock> {
    let has_active_or_ready = run.tasks.iter().any(|task| {
        matches!(
            task.state,
            SwarmTaskState::Ready | SwarmTaskState::Dispatched | SwarmTaskState::Running
        )
    });
    if has_active_or_ready {
        return None;
    }

    let pending = run
        .tasks
        .iter()
        .filter(|task| matches!(task.state, SwarmTaskState::Pending))
        .map(|task| task.id.clone())
        .collect::<Vec<_>>();
    if pending.is_empty() {
        return None;
    }

    let pending_ids = pending.iter().map(|id| id.as_str()).collect::<HashSet<_>>();
    let pending_tasks = run
        .tasks
        .iter()
        .filter(|task| matches!(task.state, SwarmTaskState::Pending))
        .cloned()
        .collect::<Vec<_>>();

    let mut message = String::new();
    message.push_str(&format!(
        "Swarm deadlock: skipping tasks with unresolvable deps: {}",
        pending.join(", ")
    ));

    if let Some(cycle) = find_swarm_cycle_path(pending_tasks.as_slice()) {
        let mut cycle = cycle;
        if cycle.len() > 12 {
            cycle.truncate(12);
            cycle.push("…".into());
        }
        message.push_str(&format!("\nCycle detected: {}", cycle.join(" -> ")));
    }

    message.push_str("\nBlocked on:");
    for task in pending_tasks.iter().take(12) {
        let deps = task
            .deps
            .iter()
            .filter(|dep| pending_ids.contains(dep.as_str()))
            .cloned()
            .collect::<Vec<_>>();
        if deps.is_empty() {
            message.push_str(&format!("\n- {} waits on: (none)", task.id));
        } else {
            message.push_str(&format!("\n- {} waits on: {}", task.id, deps.join(", ")));
        }
    }

    for task in run.tasks.iter_mut() {
        if matches!(task.state, SwarmTaskState::Pending) {
            task.state = SwarmTaskState::Skipped;
            task.failed = true;
            if task.output.is_none() {
                task.output = Some("SKIPPED (unresolvable deps)".into());
            }
        }
    }

    Some(SwarmDeadlock {
        skipped: pending,
        message,
    })
}

pub(super) fn dispatch_ready_tasks(run: &mut SwarmRun) -> Vec<SwarmDispatch> {
    let indices = select_dispatchable_ready_task_indices(run);
    let mut dispatches = Vec::new();
    for idx in indices {
        let task = &run.tasks[idx];
        let deps_payload = collect_dependency_payload(run, task);
        let prompt = if deps_payload.is_empty() {
            wrap_task_prompt(
                &run.root_prompt,
                run.mission_kind,
                task,
                None,
                &run.scope_files,
            )
        } else {
            wrap_task_prompt(
                &run.root_prompt,
                run.mission_kind,
                task,
                Some(deps_payload.as_slice()),
                &run.scope_files,
            )
        };
        let agent_id = task.agent_id.clone();
        let task_role = task.role.clone();
        run.tasks[idx].state = SwarmTaskState::Dispatched;
        dispatches.push(SwarmDispatch {
            agent_id,
            mission_id: run.mission_id.clone(),
            prompt,
            task_role,
        });
    }
    dispatches
}

fn select_dispatchable_ready_task_indices(run: &SwarmRun) -> Vec<usize> {
    // Lab and Bulk templates rely on a global single-writer invariant — only
    // one task with writes=true can be Dispatched/Running at a time, with
    // other writer tasks queued behind it. Parallel explicitly allows write
    // fan-out: multiple integrate tasks can execute concurrently (their work
    // regions are expected to be disjoint per the planner prompt, and the
    // substrate's claim lattice surfaces any conflicts that do arise via
    // ClaimViolation signals + auto-retries).
    let enforce_single_writer = !matches!(run.template, SwarmTemplate::Parallel);
    let mut writer_taken = enforce_single_writer
        && run.tasks.iter().any(|task| {
            task.writes
                && matches!(
                    task.state,
                    SwarmTaskState::Dispatched | SwarmTaskState::Running
                )
        });
    // Proposer pre-scan gate: hold propose-role dispatches until the genome
    // worker has produced baseline reports for scope files. Without this,
    // proposers see an empty GENOME LANDSCAPE section (no reports exist
    // yet on fresh workspaces) and fall back to surface-level advice.
    let prescan_active = !run.prescan_pending.is_empty();

    let mut indices = Vec::new();
    for (idx, task) in run.tasks.iter().enumerate() {
        if !matches!(task.state, SwarmTaskState::Ready) {
            continue;
        }
        if prescan_active {
            // Hold any role that consumes the genome landscape (propose +
            // integrate + judge) until the pre-scan produces reports for
            // scope files. Applies across every template — parallel, lab,
            // bulk — because each of these roles reads the landscape from
            // its augmented prompt.
            let role_wants_landscape = matches!(
                task.role
                    .as_deref()
                    .and_then(normalize_role_label)
                    .as_deref(),
                Some("propose") | Some("integrate") | Some("judge")
            );
            if role_wants_landscape {
                continue;
            }
        }
        if task.writes && enforce_single_writer {
            if writer_taken {
                continue;
            }
            writer_taken = true;
        }
        indices.push(idx);
    }
    indices
}

fn collect_dependency_payload(run: &SwarmRun, task: &SwarmTask) -> Vec<(String, String)> {
    let role = task.role.as_deref().and_then(normalize_role_label);
    // Tasks that must ACT on dependency outputs need the full raw text —
    // compact artifact summaries strip reasoning and implementation details,
    // causing agents to describe changes instead of executing them.
    //
    // Full output for: judge (comparing proposals), integrate (implementing),
    // and any task with `writes: true` (custom write-role tasks from planner).
    let needs_full_output = matches!(role.as_deref(), Some("judge" | "integrate")) || task.writes;

    // Full-output roles share a total budget across deps so a fan-in from
    // many proposers can't blow past the downstream model's context window.
    // Non-full roles keep the per-dep cap (their payloads are already
    // compact artifact summaries).
    let dep_count = task.deps.len().max(1);
    let per_dep_cap = if needs_full_output {
        (SWARM_DEP_OUTPUT_TOTAL_MAX_CHARS_FULL / dep_count).min(SWARM_DEP_OUTPUT_MAX_CHARS_FULL)
    } else {
        SWARM_DEP_OUTPUT_MAX_CHARS
    };

    let mut out = Vec::new();
    for dep_id in task.deps.iter() {
        let Some(dep) = run.tasks.iter().find(|t| t.id == *dep_id) else {
            continue;
        };
        let status = match dep.state {
            SwarmTaskState::Done => "DONE",
            SwarmTaskState::Failed => "FAILED",
            SwarmTaskState::Skipped => "SKIPPED",
            _ => "PENDING",
        };
        let label = format!("{} [{}] (agent {})", dep.id, status, dep.agent_id);
        let text = if needs_full_output {
            dependency_payload_text_full(dep)
        } else {
            dependency_payload_text(run, dep)
        };
        out.push((label, truncate_chars(&text, per_dep_cap)));
    }
    out
}
