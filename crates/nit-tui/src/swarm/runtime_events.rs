use nit_core::{AgentBusEvent, AppState, MissionPhase};

use super::{
    abort_swarm_plan_preflight, analyze_swarm_dag, apply_role_dependency_ordering,
    build_synthesis_prompt, build_verify_prompt, cleanup_swarm_clones_for_mission,
    detect_incomplete_signoff, dispatch_ready_tasks, drain_queued_turns_for_agent,
    emit_parallel_deps_auto_repair_signals, emit_unresolved_dep_signals, ensure_agent_coverage,
    ensure_deps_resolve, ensure_integrate_task, ensure_judge_task_for_multi_proposer,
    ensure_proposer_task, fallback_tasks, gate_bundle_label, initialize_task_graph,
    is_chat_clone_agent_id, is_provider_rate_limit_failure, mark_task_finished, mark_task_running,
    maybe_resolve_deadlock, maybe_spawn_genome_review, parse_gate_report, parse_plan_from_planner,
    push_system_message_to_mission, read_workspace_dag_validation_mode, refresh_task_readiness,
    repair_swarm_dag, run_gates_label, spawn_genome_gate_eval, structural_compliance_missing_files,
    tag_last_agent_message_kind, tasks_terminal_count, try_dispatch_gate_retry,
    update_mission_final, update_mission_phase, update_mission_status, GenomeGatePending,
    ParsedSwarmPlan, SwarmArtifactFocus, SwarmDagValidationMode, SwarmDispatch, SwarmEventOutcome,
    SwarmRun, SwarmRuntime, SwarmStage, SwarmTaskState, DEFAULT_DAG_VALIDATION_MODE,
};

// Cap on re-dispatches when an output fails the sign-off check. Past this
// we accept the partial result and move on.
const MAX_CONTINUATION_RETRIES: u8 = 2;

impl SwarmRuntime {
    pub fn handle_event(
        &mut self,
        state: &mut AppState,
        event: &AgentBusEvent,
    ) -> Vec<SwarmDispatch> {
        self.handle_event_outcome(state, event).dispatches
    }

    pub(crate) fn handle_event_outcome(
        &mut self,
        state: &mut AppState,
        event: &AgentBusEvent,
    ) -> SwarmEventOutcome {
        let mut outcome = SwarmEventOutcome::default();

        // Chat clones are ad-hoc; they must never interact with swarm runs.
        if event_agent_id(event).is_some_and(is_chat_clone_agent_id) {
            return outcome;
        }

        match event {
            AgentBusEvent::TurnStarted {
                agent_id,
                mission_id: Some(mid),
                ..
            } => {
                self.handle_turn_started(state, mid, agent_id);
            }
            AgentBusEvent::TurnCompleted {
                agent_id,
                mission_id: Some(mid),
                message,
                ..
            } => {
                self.handle_turn_completed(state, &mut outcome, mid, agent_id, message);
            }
            AgentBusEvent::TurnFailed {
                agent_id,
                mission_id: Some(mid),
                message,
                ..
            } => {
                self.handle_turn_failed(state, &mut outcome, mid, agent_id, message);
            }
            _ => {}
        }

        outcome
    }

    fn handle_turn_started(&mut self, state: &mut AppState, mid: &str, agent_id: &str) {
        if let Some(run) = self.runs.get_mut(mid) {
            mark_task_running(run, agent_id);
            update_mission_status(state, run, None);
        }
    }

    fn handle_turn_completed(
        &mut self,
        state: &mut AppState,
        outcome: &mut SwarmEventOutcome,
        mid: &str,
        agent_id: &str,
        message: &str,
    ) {
        let Some(mut run) = self.runs.remove(mid) else {
            return;
        };
        let fate = match run.stage {
            SwarmStage::Planning if agent_id == run.planner_agent_id => {
                handle_completed_planning(state, outcome, &mut run, agent_id, message)
            }
            SwarmStage::Executing => {
                handle_completed_executing(state, outcome, &mut run, agent_id, message)
            }
            SwarmStage::Verifying => {
                handle_completed_verifying(state, outcome, &mut run, agent_id, message)
            }
            SwarmStage::Synthesizing if agent_id == run.planner_agent_id => {
                handle_completed_synthesizing(state, outcome, &mut run, agent_id, message)
            }
            _ => RunFate::Active,
        };
        self.commit_fate(mid, run, fate);
    }

    fn handle_turn_failed(
        &mut self,
        state: &mut AppState,
        outcome: &mut SwarmEventOutcome,
        mid: &str,
        agent_id: &str,
        message: &str,
    ) {
        let Some(mut run) = self.runs.remove(mid) else {
            return;
        };
        let fate = match run.stage {
            SwarmStage::Planning if agent_id == run.planner_agent_id => {
                handle_failed_planning(state, outcome, &mut run, agent_id, message)
            }
            SwarmStage::Executing => {
                handle_failed_executing(state, outcome, &mut run, agent_id, message)
            }
            SwarmStage::Verifying => {
                handle_failed_verifying(state, outcome, &mut run, agent_id, message)
            }
            SwarmStage::Synthesizing if agent_id == run.planner_agent_id => {
                handle_failed_synthesizing(state, outcome, &mut run, agent_id, message)
            }
            _ => RunFate::Active,
        };
        self.commit_fate(mid, run, fate);
    }

    fn commit_fate(&mut self, mid: &str, run: SwarmRun, fate: RunFate) {
        match fate {
            RunFate::Active => {
                self.runs.insert(mid.to_string(), run);
            }
            RunFate::Completed => {
                self.completed_runs.insert(mid.to_string(), run);
            }
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum RunFate {
    Active,
    Completed,
}

fn event_agent_id(event: &AgentBusEvent) -> Option<&str> {
    match event {
        AgentBusEvent::TurnStarted { agent_id, .. }
        | AgentBusEvent::TurnCompleted { agent_id, .. }
        | AgentBusEvent::TurnFailed { agent_id, .. } => Some(agent_id.as_str()),
        _ => None,
    }
}

fn handle_completed_planning(
    state: &mut AppState,
    outcome: &mut SwarmEventOutcome,
    run: &mut SwarmRun,
    agent_id: &str,
    message: &str,
) -> RunFate {
    tag_last_agent_message_kind(state, agent_id, &run.mission_id, "plan");
    let available = non_planner_agents(run);
    let multi_integrator = run.scope_files.len() > 15;
    let parsed = parse_plan_from_planner(
        message,
        run.template,
        run.mission_kind,
        &run.root_prompt,
        &available,
        run.integrator_agent_id.as_deref(),
        run.integrator_locked,
        multi_integrator,
    );
    finalize_plan(state, outcome, run, parsed, &available, multi_integrator)
}

fn handle_failed_planning(
    state: &mut AppState,
    outcome: &mut SwarmEventOutcome,
    run: &mut SwarmRun,
    agent_id: &str,
    message: &str,
) -> RunFate {
    tag_last_agent_message_kind(state, agent_id, &run.mission_id, "plan");
    let available = non_planner_agents(run);
    let multi_integrator = run.scope_files.len() > 15;
    let parsed = fallback_tasks(
        run.template,
        run.mission_kind,
        &run.root_prompt,
        &available,
        Some(message),
        run.integrator_agent_id.as_deref(),
    );
    finalize_plan(state, outcome, run, parsed, &available, multi_integrator)
}

fn finalize_plan(
    state: &mut AppState,
    outcome: &mut SwarmEventOutcome,
    run: &mut SwarmRun,
    mut parsed: ParsedSwarmPlan,
    available: &[String],
    multi_integrator: bool,
) -> RunFate {
    enrich_plan_with_required_tasks(state, run, &mut parsed, available, multi_integrator);
    if validate_plan_dag(state, run, &mut parsed) == DagValidationOutcome::Abort {
        for warning in parsed.warnings.iter() {
            push_system_message_to_mission(
                state,
                &run.mission_id,
                format!("PLAN warning: {warning}"),
            );
        }
        abort_swarm_plan_preflight(state, run, parsed);
        cleanup_swarm_clones_for_mission(state, &run.mission_id);
        return RunFate::Completed;
    }
    for warning in parsed.warnings.iter() {
        push_system_message_to_mission(state, &run.mission_id, format!("PLAN warning: {warning}"));
    }
    apply_post_plan_integrator_and_verifier(state, run, &parsed);
    run.tasks = parsed.tasks;
    run.synthesis_prompt = parsed.synthesis_prompt;
    run.stage = SwarmStage::Executing;
    update_mission_phase(state, &run.mission_id, MissionPhase::Execute);
    initialize_task_graph(run);
    refresh_task_readiness(run);
    emit_unresolved_dep_signals(state, run);
    outcome.dispatches.extend(dispatch_ready_tasks(run));
    if let Some(deadlock) = maybe_resolve_deadlock(run) {
        push_system_message_to_mission(state, &run.mission_id, deadlock.message);
    }
    let done = tasks_terminal_count(&run.tasks);
    update_mission_status(state, run, Some(done));
    transition_after_tasks(state, outcome, run, done);
    RunFate::Active
}

fn enrich_plan_with_required_tasks(
    state: &mut AppState,
    run: &SwarmRun,
    parsed: &mut ParsedSwarmPlan,
    available: &[String],
    multi_integrator: bool,
) {
    // Synthesize integrate / proposer / judge BEFORE role-dep ordering so
    // downstream roles see them as producers and get the correct deps
    // wired. If synthesis ran after role-deps, test/review would have no
    // integrate dep when the planner omitted the task and would dispatch
    // in parallel with proposers.
    parsed.warnings.extend(ensure_integrate_task(
        &mut parsed.tasks,
        run.mission_kind,
        run.integrator_agent_id
            .as_deref()
            .or(parsed.integrator_agent_id.as_deref()),
    ));
    parsed.warnings.extend(ensure_proposer_task(
        &mut parsed.tasks,
        run.template,
        run.mission_kind,
        run.integrator_agent_id
            .as_deref()
            .or(parsed.integrator_agent_id.as_deref()),
    ));
    parsed.warnings.extend(ensure_judge_task_for_multi_proposer(
        &mut parsed.tasks,
        run.template,
        available,
        run.integrator_agent_id
            .as_deref()
            .or(parsed.integrator_agent_id.as_deref()),
    ));
    parsed.warnings.extend(apply_role_dependency_ordering(
        state.workspace_root.as_path(),
        &state.agents.swarm_role_by_agent_id,
        run.mission_kind,
        run.integrator_agent_id.as_deref(),
        parsed.tasks.as_mut_slice(),
        multi_integrator,
    ));
    let deps_repairs = ensure_deps_resolve(&mut parsed.tasks, run.template);
    if !deps_repairs.is_empty() {
        for desc in &deps_repairs {
            parsed.warnings.push(format!("Plan safety net: {desc}"));
        }
        emit_parallel_deps_auto_repair_signals(
            state,
            &run.planner_agent_id,
            &run.mission_id,
            run.template.label(),
            &deps_repairs,
        );
    }
    parsed.warnings.extend(ensure_agent_coverage(
        &mut parsed.tasks,
        run.template,
        run.mission_kind,
        available,
    ));
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum DagValidationOutcome {
    Continue,
    Abort,
}

fn validate_plan_dag(
    state: &mut AppState,
    run: &SwarmRun,
    parsed: &mut ParsedSwarmPlan,
) -> DagValidationOutcome {
    let dag_mode = match read_workspace_dag_validation_mode(state.workspace_root.as_path()) {
        Ok(Some(mode)) => mode,
        Ok(None) => DEFAULT_DAG_VALIDATION_MODE,
        Err(err) => {
            push_system_message_to_mission(
                state,
                &run.mission_id,
                format!(
                    "PLAN warning: DAG validation config error: {err}; using default mode 'strict'."
                ),
            );
            DEFAULT_DAG_VALIDATION_MODE
        }
    };

    let dag_issues = analyze_swarm_dag(parsed.tasks.as_slice());
    if dag_issues.is_empty() {
        return DagValidationOutcome::Continue;
    }
    match dag_mode {
        SwarmDagValidationMode::Strict => {
            push_system_message_to_mission(
                state,
                &run.mission_id,
                format!(
                    "PLAN error: invalid task DAG ({}). Set `[swarm] dag_validation = \"repair\"` in `.nit/config.toml` to auto-repair.",
                    dag_issues.summary()
                ),
            );
            mark_unfinished_tasks_skipped(parsed, "SKIPPED (preflight: invalid task DAG)");
            DagValidationOutcome::Abort
        }
        SwarmDagValidationMode::Repair => {
            let mut warnings = repair_swarm_dag(parsed.tasks.as_mut_slice());
            let after = analyze_swarm_dag(parsed.tasks.as_slice());
            if !after.is_empty() {
                push_system_message_to_mission(
                    state,
                    &run.mission_id,
                    format!(
                        "PLAN error: DAG auto-repair failed ({}); aborting.",
                        after.summary()
                    ),
                );
                mark_unfinished_tasks_skipped(
                    parsed,
                    "SKIPPED (preflight: DAG auto-repair failed)",
                );
                return DagValidationOutcome::Abort;
            }
            if warnings.is_empty() {
                warnings.push("DAG repair: plan had DAG issues; no changes needed.".into());
            }
            parsed.warnings.append(&mut warnings);
            DagValidationOutcome::Continue
        }
    }
}

fn mark_unfinished_tasks_skipped(parsed: &mut ParsedSwarmPlan, output: &str) {
    for task in parsed.tasks.iter_mut() {
        if task.state.is_terminal() {
            continue;
        }
        task.state = SwarmTaskState::Skipped;
        task.failed = true;
        if task.output.is_none() {
            task.output = Some(output.to_string());
        }
    }
}

fn apply_post_plan_integrator_and_verifier(
    state: &mut AppState,
    run: &mut SwarmRun,
    parsed: &ParsedSwarmPlan,
) {
    let prev_integrator = run.integrator_agent_id.clone();
    let prev_verifier = run.verifier_agent_id.clone();
    if parsed.integrator_agent_id.is_some() {
        run.integrator_agent_id = parsed.integrator_agent_id.clone();
    }
    if run.gate_bundle.is_some() {
        run.verifier_agent_id = pick_verifier_after_plan(run);
    }
    if prev_integrator != run.integrator_agent_id || prev_verifier != run.verifier_agent_id {
        push_system_message_to_mission(
            state,
            &run.mission_id,
            format!(
                "Swarm template: {} | integrator: {} | verifier: {} | gates: {}",
                run.template.label(),
                run.integrator_agent_id.as_deref().unwrap_or("(none)"),
                run.verifier_agent_id.as_deref().unwrap_or("(none)"),
                gate_bundle_label(run.gate_bundle.as_ref(), run.gate_selection.as_str())
            ),
        );
    }
}

fn pick_verifier_after_plan(run: &SwarmRun) -> Option<String> {
    if let Some(integrator) = run.integrator_agent_id.as_deref() {
        run.agent_ids
            .iter()
            .find(|id| id.as_str() != run.planner_agent_id.as_str() && id.as_str() != integrator)
            .cloned()
            .or_else(|| {
                run.agent_ids
                    .iter()
                    .find(|id| id.as_str() != run.planner_agent_id.as_str())
                    .cloned()
            })
    } else {
        run.agent_ids
            .iter()
            .find(|id| id.as_str() != run.planner_agent_id.as_str())
            .cloned()
    }
}

fn handle_completed_executing(
    state: &mut AppState,
    outcome: &mut SwarmEventOutcome,
    run: &mut SwarmRun,
    agent_id: &str,
    message: &str,
) -> RunFate {
    let agent_has_writes = state
        .genome_turn_modified
        .get(agent_id)
        .is_some_and(|files| !files.is_empty());
    if let Some(completed) =
        mark_task_finished(run, agent_id, message.to_string(), false, agent_has_writes)
    {
        outcome.artifact_focus = Some(SwarmArtifactFocus::Task {
            mission_id: run.mission_id.clone(),
            task_id: completed.task_id.clone(),
        });
        if completed.expected_artifacts_missing {
            push_system_message_to_mission(
                state,
                &run.mission_id,
                format!(
                    "Swarm artifacts: task '{}' declared artifacts but no parseable swarm_artifacts JSON block was found.",
                    completed.task_id
                ),
            );
        }
        if completed.writes_expected && !completed.writes_detected {
            push_system_message_to_mission(
                state,
                &run.mission_id,
                format!(
                    "WARNING: task '{}' was expected to write files but no file modifications were detected. \
                     The agent may have described changes without executing them.",
                    completed.task_id
                ),
            );
        }
        report_structural_compliance(state, run, &completed.task_id);
        if let Some(reason) = detect_incomplete_signoff(message) {
            handle_incomplete_signoff(state, run, &completed.task_id, reason);
        }
    }
    refresh_task_readiness(run);
    emit_unresolved_dep_signals(state, run);
    outcome.dispatches.extend(dispatch_ready_tasks(run));
    if let Some(deadlock) = maybe_resolve_deadlock(run) {
        push_system_message_to_mission(state, &run.mission_id, deadlock.message);
    }
    let done = tasks_terminal_count(&run.tasks);
    update_mission_status(state, run, Some(done));
    transition_after_tasks(state, outcome, run, done);
    RunFate::Active
}

fn report_structural_compliance(state: &mut AppState, run: &SwarmRun, task_id: &str) {
    let missing = structural_compliance_missing_files(run, task_id, state);
    if missing.is_empty() {
        return;
    }
    let preview: Vec<String> = missing.iter().take(8).cloned().collect();
    let more = if missing.len() > preview.len() {
        format!(" (+{} more)", missing.len() - preview.len())
    } else {
        String::new()
    };
    push_system_message_to_mission(
        state,
        &run.mission_id,
        format!(
            "STRUCTURAL COMPLIANCE: task '{task_id}' finished but {} proposer-declared file(s) were not modified: {}{more}. \
             Silent divergence — integrator skipped part of the plan.",
            missing.len(),
            preview.join(", "),
        ),
    );
    let id = state.substrate.next_signal_id(task_id);
    let posted_at_gen = state.substrate.current_generation();
    state.substrate.emit_signal(nit_core::substrate::Signal {
        id,
        kind: nit_core::substrate::SignalKind::Warning,
        posted_by: format!("swarm:compliance:{task_id}"),
        posted_at_gen,
        target: nit_core::substrate::SignalTarget::Global,
        initial_strength: nit_core::substrate::SubstrateState::DEFAULT_INITIAL_STRENGTH,
        payload: serde_json::json!({
            "reason": "structural_compliance_missing_files",
            "task_id": task_id,
            "missing_files": missing,
            "mission_id": run.mission_id,
        }),
    });
}

fn handle_incomplete_signoff(
    state: &mut AppState,
    run: &mut SwarmRun,
    task_id: &str,
    reason: &'static str,
) {
    let Some(task) = run.tasks.iter_mut().find(|t| t.id == task_id) else {
        return;
    };
    if task.retries < MAX_CONTINUATION_RETRIES {
        task.retries = task.retries.saturating_add(1);
        task.state = SwarmTaskState::Ready;
        task.failed = false;
        task.expected_artifacts_missing = false;
        let attempt = task.retries;
        let task_id_owned = task.id.clone();
        push_system_message_to_mission(
            state,
            &run.mission_id,
            format!(
                "SIGN-OFF: task '{task_id_owned}' output flagged as incomplete ({reason}). Re-dispatching with continuation (attempt {attempt}/{MAX_CONTINUATION_RETRIES})."
            ),
        );
    } else {
        push_system_message_to_mission(
            state,
            &run.mission_id,
            format!(
                "SIGN-OFF: task '{task_id}' output flagged as incomplete ({reason}), retry budget exhausted — accepting partial result."
            ),
        );
    }
}

fn handle_completed_verifying(
    state: &mut AppState,
    outcome: &mut SwarmEventOutcome,
    run: &mut SwarmRun,
    agent_id: &str,
    message: &str,
) -> RunFate {
    if run.verifier_agent_id.as_deref() != Some(agent_id) {
        return RunFate::Active;
    }
    run.gate_output = Some(message.to_string());
    run.gate_report = parse_gate_report(message);
    if let Some(report) = run.gate_report.as_ref() {
        let label = if report.overall_ok { "PASS" } else { "FAIL" };
        push_system_message_to_mission(state, &run.mission_id, format!("VERIFY result: {label}"));
        let gate_summary = report
            .gates
            .iter()
            .map(|gate| format!("{} {}", gate.name, gate.ui_status()))
            .collect::<Vec<_>>()
            .join(" | ");
        if !gate_summary.is_empty() {
            push_system_message_to_mission(
                state,
                &run.mission_id,
                format!("Swarm gates: {gate_summary}"),
            );
        }
    } else {
        push_system_message_to_mission(
            state,
            &run.mission_id,
            "VERIFY result: ERROR (no parseable JSON report)".into(),
        );
    }

    if let Some(retry_dispatch) = try_dispatch_gate_retry(run, state) {
        outcome.dispatches.push(retry_dispatch);
        return RunFate::Active;
    }

    maybe_spawn_genome_review(run, state);

    run.stage = SwarmStage::Synthesizing;
    update_mission_phase(state, &run.mission_id, MissionPhase::Report);
    update_mission_status(state, run, Some(tasks_terminal_count(&run.tasks)));
    outcome.dispatches.push(SwarmDispatch {
        agent_id: run.planner_agent_id.clone(),
        mission_id: run.mission_id.clone(),
        prompt: build_synthesis_prompt(run),
        task_role: None,
    });
    RunFate::Active
}

fn handle_completed_synthesizing(
    state: &mut AppState,
    outcome: &mut SwarmEventOutcome,
    run: &mut SwarmRun,
    agent_id: &str,
    message: &str,
) -> RunFate {
    run.report_status = Some("DONE".into());
    run.report_output = Some(message.to_string());
    tag_last_agent_message_kind(state, agent_id, &run.mission_id, "synth");
    outcome.artifact_focus = Some(SwarmArtifactFocus::Report {
        mission_id: run.mission_id.clone(),
    });
    run.stage = SwarmStage::Synthesizing;
    update_mission_phase(state, &run.mission_id, MissionPhase::Report);

    let tasks_ok = run
        .tasks
        .iter()
        .all(|task| matches!(task.state, SwarmTaskState::Done));
    let verify_ok = run.gate_bundle.is_none()
        || run
            .gate_report
            .as_ref()
            .is_some_and(|report| report.overall_ok);
    let verify_error = run.gate_bundle.is_some() && run.gate_report.is_none();
    let final_status = if verify_error {
        "ERROR"
    } else if !tasks_ok {
        "FAILED"
    } else if verify_ok {
        "DONE"
    } else {
        "FAILED"
    };
    update_mission_final(state, &run.mission_id, final_status);
    cleanup_swarm_clones_for_mission(state, &run.mission_id);
    RunFate::Completed
}

fn handle_failed_executing(
    state: &mut AppState,
    outcome: &mut SwarmEventOutcome,
    run: &mut SwarmRun,
    agent_id: &str,
    message: &str,
) -> RunFate {
    let agent_has_writes = state
        .genome_turn_modified
        .get(agent_id)
        .is_some_and(|files| !files.is_empty());

    // Retry write-role tasks once on failure — agent may have crashed
    // due to a transient issue (rate limit, context overflow, etc.).
    // Reset to Ready so it gets re-dispatched with its dep outputs.
    if try_retry_failed_task(state, run, agent_id, message) {
        refresh_task_readiness(run);
        emit_unresolved_dep_signals(state, run);
        outcome.dispatches.extend(dispatch_ready_tasks(run));
        return RunFate::Active;
    }

    if let Some(completed) =
        mark_task_finished(run, agent_id, message.to_string(), true, agent_has_writes)
    {
        outcome.artifact_focus = Some(SwarmArtifactFocus::Task {
            mission_id: run.mission_id.clone(),
            task_id: completed.task_id.clone(),
        });
        if completed.expected_artifacts_missing {
            push_system_message_to_mission(
                state,
                &run.mission_id,
                format!(
                    "Swarm artifacts: task '{}' declared artifacts but no parseable swarm_artifacts JSON block was found.",
                    completed.task_id
                ),
            );
        }
        if completed.writes_expected && !completed.writes_detected {
            push_system_message_to_mission(
                state,
                &run.mission_id,
                format!(
                    "WARNING: task '{}' was expected to write files but no file modifications were detected. \
                     The agent may have described changes without executing them.",
                    completed.task_id
                ),
            );
        }
    }
    // Drain orphaned queued turns for the failed agent so queue_len
    // doesn't leak.
    drain_queued_turns_for_agent(state, agent_id);
    refresh_task_readiness(run);
    emit_unresolved_dep_signals(state, run);
    outcome.dispatches.extend(dispatch_ready_tasks(run));
    if let Some(deadlock) = maybe_resolve_deadlock(run) {
        push_system_message_to_mission(state, &run.mission_id, deadlock.message);
    }
    let done = tasks_terminal_count(&run.tasks);
    update_mission_status(state, run, Some(done));
    transition_after_tasks(state, outcome, run, done);
    RunFate::Active
}

// Returns `true` when the failed task was reset to Ready for retry.
fn try_retry_failed_task(
    state: &mut AppState,
    run: &mut SwarmRun,
    agent_id: &str,
    message: &str,
) -> bool {
    let task_idx = run.tasks.iter().position(|t| {
        t.agent_id == agent_id
            && matches!(
                t.state,
                SwarmTaskState::Running | SwarmTaskState::Dispatched
            )
    });
    let Some(idx) = task_idx else {
        return false;
    };
    // Provider 429 → retrying immediately just burns the budget on an
    // exhausted quota. Skip the retry and surface a clear message.
    if is_provider_rate_limit_failure(message) {
        let task = &run.tasks[idx];
        push_system_message_to_mission(
            state,
            &run.mission_id,
            format!(
                "Task '{}' failed: provider rate-limited (429). Not retrying — wait for the quota window to reset.",
                task.id,
            ),
        );
        return false;
    }
    let task = &mut run.tasks[idx];
    if !task.writes || task.retries >= 1 {
        return false;
    }
    task.retries += 1;
    task.state = SwarmTaskState::Ready;
    task.output = None;
    push_system_message_to_mission(
        state,
        &run.mission_id,
        format!(
            "Task '{}' failed ({}); retrying (attempt {}).",
            task.id,
            message.chars().take(120).collect::<String>(),
            task.retries + 1,
        ),
    );
    true
}

fn handle_failed_verifying(
    state: &mut AppState,
    outcome: &mut SwarmEventOutcome,
    run: &mut SwarmRun,
    agent_id: &str,
    message: &str,
) -> RunFate {
    if run.verifier_agent_id.as_deref() != Some(agent_id) {
        return RunFate::Active;
    }
    run.gate_output = Some(message.to_string());
    run.gate_report = None;
    push_system_message_to_mission(
        state,
        &run.mission_id,
        format!("VERIFY result: ERROR ({message})"),
    );

    maybe_spawn_genome_review(run, state);

    run.stage = SwarmStage::Synthesizing;
    update_mission_phase(state, &run.mission_id, MissionPhase::Report);
    update_mission_status(state, run, Some(tasks_terminal_count(&run.tasks)));
    outcome.dispatches.push(SwarmDispatch {
        agent_id: run.planner_agent_id.clone(),
        mission_id: run.mission_id.clone(),
        prompt: build_synthesis_prompt(run),
        task_role: None,
    });
    RunFate::Active
}

fn handle_failed_synthesizing(
    state: &mut AppState,
    outcome: &mut SwarmEventOutcome,
    run: &mut SwarmRun,
    agent_id: &str,
    message: &str,
) -> RunFate {
    run.report_status = Some("ERROR".into());
    run.report_output = Some(message.to_string());
    tag_last_agent_message_kind(state, agent_id, &run.mission_id, "synth");
    outcome.artifact_focus = Some(SwarmArtifactFocus::Report {
        mission_id: run.mission_id.clone(),
    });
    update_mission_final(state, &run.mission_id, "ERROR");
    cleanup_swarm_clones_for_mission(state, &run.mission_id);
    RunFate::Completed
}

// After every task is terminal: open verify (with optional async genome
// gate) or jump straight to synth when there are no gates / no verifier.
fn transition_after_tasks(
    state: &mut AppState,
    outcome: &mut SwarmEventOutcome,
    run: &mut SwarmRun,
    done: usize,
) {
    if done != run.tasks.len() {
        return;
    }
    let label_and_verifier = run_gates_label(run).zip(run.verifier_agent_id.clone());
    if let Some((label, verifier)) = label_and_verifier {
        run.stage = SwarmStage::Verifying;
        update_mission_phase(state, &run.mission_id, MissionPhase::Verify);
        update_mission_status(state, run, Some(done));
        if state.settings.genome.genome_gate_enabled {
            // Verifier dispatch happens once the background gate result
            // arrives (poll_genome_gates).
            run.genome_gate_pending = Some(GenomeGatePending {
                rx: spawn_genome_gate_eval(state, &run.mission_id, &run.initial_genome_baselines),
                label: label.clone(),
                verifier: verifier.clone(),
            });
            push_system_message_to_mission(
                state,
                &run.mission_id,
                format!(
                    "Genome gate evaluating\u{2026} VERIFY ({label}) will start on agent {verifier} when complete",
                ),
            );
        } else {
            push_system_message_to_mission(
                state,
                &run.mission_id,
                format!("Starting VERIFY ({label}) on agent {verifier}"),
            );
            outcome.dispatches.push(SwarmDispatch {
                agent_id: verifier,
                mission_id: run.mission_id.clone(),
                prompt: build_verify_prompt(run),
                task_role: None,
            });
        }
    } else {
        run.stage = SwarmStage::Synthesizing;
        update_mission_phase(state, &run.mission_id, MissionPhase::Report);
        update_mission_status(state, run, Some(done));
        outcome.dispatches.push(SwarmDispatch {
            agent_id: run.planner_agent_id.clone(),
            mission_id: run.mission_id.clone(),
            prompt: build_synthesis_prompt(run),
            task_role: None,
        });
    }
}

fn non_planner_agents(run: &SwarmRun) -> Vec<String> {
    run.agent_ids
        .iter()
        .filter(|id| *id != &run.planner_agent_id)
        .cloned()
        .collect()
}
