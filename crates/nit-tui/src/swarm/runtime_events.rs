use std::collections::HashMap;

use nit_core::{AgentBusEvent, AppState, MissionPhase};

use super::repair::{build_repair_prompt, evaluate_repair_round};
use super::validator::{must_fix, validate_plan, Severity, ValidationContext, Violation};
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
    repair_swarm_dag, run_gates_label, shard_integrate_for_large_scope, spawn_genome_gate_eval,
    structural_compliance_missing_files, structural_split_gaps, tag_last_agent_message_kind,
    tasks_terminal_count, try_dispatch_gate_retry, update_mission_final, update_mission_phase,
    update_mission_status, GenomeGatePending, ParsedSwarmPlan, SwarmArtifactFocus,
    SwarmDagValidationMode, SwarmDispatch, SwarmEventOutcome, SwarmRun, SwarmRuntime, SwarmStage,
    SwarmTaskState, DEFAULT_DAG_VALIDATION_MODE, REPAIR_RETRY_LIMIT,
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
            SwarmStage::Planning if agent_id == run.planner_agent_id => handle_completed_planning(
                state,
                outcome,
                &mut run,
                agent_id,
                message,
                self.legacy_planner,
            ),
            SwarmStage::Executing => {
                handle_completed_executing(state, outcome, &mut run, agent_id, message)
            }
            SwarmStage::Verifying => {
                handle_completed_verifying(state, outcome, &mut run, agent_id, message)
            }
            SwarmStage::Synthesizing if agent_id == run.planner_agent_id => {
                handle_completed_synthesizing(state, outcome, &mut run, agent_id, message)
            }
            SwarmStage::Synthesizing if run.verifier_agent_id.as_deref() == Some(agent_id) => {
                handle_genome_review_terminal(state, &mut run, agent_id, message, false)
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
            SwarmStage::Synthesizing if run.verifier_agent_id.as_deref() == Some(agent_id) => {
                handle_genome_review_terminal(state, &mut run, agent_id, message, true)
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
    legacy_planner: bool,
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

    if legacy_planner {
        return finalize_plan(state, outcome, run, parsed, &available, multi_integrator);
    }

    match dispatch_repair_or_finalize(state, run, message, &parsed, &available) {
        RepairDecision::Finalize => {
            finalize_plan(state, outcome, run, parsed, &available, multi_integrator)
        }
        RepairDecision::DispatchRepair(prompt) => {
            outcome.dispatches.push(SwarmDispatch {
                agent_id: run.planner_agent_id.clone(),
                mission_id: run.mission_id.clone(),
                prompt,
                task_role: None,
            });
            RunFate::Active
        }
        RepairDecision::ExhaustedFallback => {
            // The exhaustion branch already pushed a system message naming
            // the surviving violation set; finalize with whatever the
            // planner gave us so the swarm can still make progress.
            finalize_plan(state, outcome, run, parsed, &available, multi_integrator)
        }
    }
}

enum RepairDecision {
    Finalize,
    DispatchRepair(String),
    ExhaustedFallback,
}

// Stashes the raw planner JSON (when extractable) so the repair prompt can
// quote the prior plan verbatim instead of asking the planner to remember
// what it produced. Stored on the run, not in the closure, so a subsequent
// failed planner turn doesn't lose the last good draft.
fn stash_plan_json(run: &mut SwarmRun, planner_message: &str) {
    if let Some(json) = super::extract_json_code_block(planner_message) {
        run.last_plan_json = Some(json);
    }
}

fn dispatch_repair_or_finalize(
    state: &mut AppState,
    run: &mut SwarmRun,
    planner_message: &str,
    parsed: &ParsedSwarmPlan,
    available: &[String],
) -> RepairDecision {
    let role_hints = collect_role_hints(state, available);
    // Detect operator-intent (ticket count, structured-list flag) so
    // the parallel-template fanout invariant has something to check
    // against. Heuristic only — see `swarm/intent.rs`. Recomputed per
    // repair round; the prompt doesn't change so the result is stable.
    let intent = super::intent::detect_intent(run.root_prompt.as_str());
    let ctx = ValidationContext {
        tasks: parsed.tasks.as_slice(),
        available_agents: available,
        integrator_agent_id: run.integrator_agent_id.as_deref(),
        role_hints: &role_hints,
        template: run.template,
        mission_kind: run.mission_kind,
        root_prompt: run.root_prompt.as_str(),
        intent,
    };
    let all_violations = validate_plan(&ctx);
    surface_advisory_violations(state, run, &all_violations);
    let blockers = must_fix(&all_violations);
    if blockers.is_empty() {
        return RepairDecision::Finalize;
    }

    if run.repair_round >= REPAIR_RETRY_LIMIT {
        announce_repair_exhausted(state, run, &blockers);
        return RepairDecision::ExhaustedFallback;
    }

    if run.repair_round > 0 {
        let outcome = evaluate_repair_round(&run.prior_violations, &blockers);
        if !outcome.strictly_improved && outcome.same_violations_persist {
            announce_repair_stuck(state, run, &blockers);
            return RepairDecision::ExhaustedFallback;
        }
    }

    stash_plan_json(run, planner_message);
    let prior_json = run.last_plan_json.clone().unwrap_or_default();
    let repair_prompt = build_repair_prompt(
        run.root_prompt.as_str(),
        prior_json.as_str(),
        &blockers,
        run.repair_round + 1,
        REPAIR_RETRY_LIMIT,
    );
    run.repair_round = run.repair_round.saturating_add(1);
    run.prior_violations = blockers.clone();
    announce_repair_round(state, run, &blockers);
    RepairDecision::DispatchRepair(repair_prompt)
}

fn collect_role_hints(state: &AppState, available: &[String]) -> HashMap<String, String> {
    let mut hints: HashMap<String, String> = HashMap::new();
    for agent_id in available.iter() {
        let Some(raw) = state.agents.swarm_role_by_agent_id.get(agent_id) else {
            continue;
        };
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            continue;
        }
        hints.insert(agent_id.clone(), trimmed.to_string());
    }
    hints
}

fn surface_advisory_violations(state: &mut AppState, run: &SwarmRun, all: &[Violation]) {
    let advisories: Vec<&Violation> = all
        .iter()
        .filter(|v| matches!(v.severity, Severity::Advisory))
        .collect();
    if advisories.is_empty() {
        return;
    }
    let preview: Vec<String> = advisories
        .iter()
        .take(3)
        .map(|v| format!("[{}] {}", v.id, v.human))
        .collect();
    let more = if advisories.len() > preview.len() {
        format!(" (+{} more)", advisories.len() - preview.len())
    } else {
        String::new()
    };
    push_system_message_to_mission(
        state,
        &run.mission_id,
        format!(
            "PLANNER advisories ({}): {}{more}",
            advisories.len(),
            preview.join("; "),
        ),
    );
}

fn announce_repair_round(state: &mut AppState, run: &SwarmRun, blockers: &[Violation]) {
    let preview: Vec<String> = blockers
        .iter()
        .take(3)
        .map(|v| format!("[{}] {}", v.id, v.human))
        .collect();
    let more = if blockers.len() > preview.len() {
        format!(" (+{} more)", blockers.len() - preview.len())
    } else {
        String::new()
    };
    push_system_message_to_mission(
        state,
        &run.mission_id,
        format!(
            "PLAN repair {round}/{max}: {count} violation(s); re-dispatching planner. {preview}{more}",
            round = run.repair_round + 1,
            max = REPAIR_RETRY_LIMIT,
            count = blockers.len(),
            preview = preview.join("; "),
        ),
    );
}

fn announce_repair_exhausted(state: &mut AppState, run: &SwarmRun, blockers: &[Violation]) {
    let preview: Vec<String> = blockers
        .iter()
        .take(3)
        .map(|v| format!("[{}] {}", v.id, v.human))
        .collect();
    push_system_message_to_mission(
        state,
        &run.mission_id,
        format!(
            "PLAN repair: budget exhausted after {} round(s); accepting plan with {} residual violation(s): {}.",
            REPAIR_RETRY_LIMIT,
            blockers.len(),
            preview.join("; "),
        ),
    );
}

fn announce_repair_stuck(state: &mut AppState, run: &SwarmRun, blockers: &[Violation]) {
    let preview: Vec<String> = blockers
        .iter()
        .take(3)
        .map(|v| format!("[{}] {}", v.id, v.human))
        .collect();
    push_system_message_to_mission(
        state,
        &run.mission_id,
        format!(
            "PLAN repair: planner stopped making progress (same violations across rounds). Accepting plan with {} residual violation(s): {}.",
            blockers.len(),
            preview.join("; "),
        ),
    );
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
    // Runtime invariant: large-scope Parallel runs with a single integrate
    // task get fanned into N sequential shards (one writer agent, smaller
    // per-shard scope). Decision is workload-driven — the planner is not
    // told to shard, the runtime makes the call after the plan lands.
    parsed.warnings.extend(shard_integrate_for_large_scope(
        &mut parsed.tasks,
        run.template,
        run.scope_files.len(),
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
        handle_structural_compliance_gap(state, run, &completed.task_id);
        // Verifier-findings retry: when a test/review task lands with
        // structured `findings`, synthesise a writer turn to fix them.
        // See `verifier_retry.rs` for the full contract; this short-
        // circuit returns before the normal readiness refresh so the
        // freshly-enqueued integrator task lands at the top of the
        // dispatch list rather than racing with whatever else just
        // unblocked.
        if let Some(dispatch) = super::verifier_retry::try_dispatch_verifier_findings_retry(
            run,
            state,
            &completed.task_id,
        ) {
            outcome.dispatches.push(dispatch);
            refresh_task_readiness(run);
            update_mission_status(state, run, Some(tasks_terminal_count(&run.tasks)));
            return RunFate::Active;
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

// Emits a substrate signal for cross-mission observability when an
// integrator's coverage falls short. Chat-side messaging is owned by
// `handle_structural_compliance_gap` so the operator sees a single message
// per gap, framed by what the runtime is doing about it (retry / accept).
fn report_structural_compliance(state: &mut AppState, run: &SwarmRun, task_id: &str) {
    let missing = structural_compliance_missing_files(run, task_id, state);
    if missing.is_empty() {
        return;
    }
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

// Re-readies an integrate task whose previous turn left blueprint files
// untouched. Reuses the same `retries` budget as sign-off retries — sharing
// the budget caps total re-dispatches and prevents runaway loops if both
// failure modes fire on the same turn.
//
// Coordinates with `handle_incomplete_signoff`: if signoff already re-readied
// the task on this turn (state == Ready), we attach the missing files to the
// existing retry slot instead of bumping `retries` again. Without this, two
// failure modes firing on the same turn would consume two attempts for a
// single re-dispatch.
fn handle_structural_compliance_gap(state: &mut AppState, run: &mut SwarmRun, task_id: &str) {
    // Combine three failure modes:
    //   1. files declared but never touched (mission_writes miss)
    //   2. newly-created stub files (existence-only compliance)
    //   3. declared "huge" sources whose split didn't actually move content
    // All three feed into the same retry slot — the agent is told about
    // every gap on the next dispatch.
    let mut entries: Vec<String> = structural_compliance_missing_files(run, task_id, state);
    let split_gaps = structural_split_gaps(run, task_id, state);
    entries.extend(split_gaps);
    if entries.is_empty() {
        return;
    }
    let Some(task) = run.tasks.iter_mut().find(|t| t.id == task_id) else {
        return;
    };
    if !task.writes {
        return;
    }
    // Signoff just re-readied us → piggyback on its retry slot.
    if matches!(task.state, SwarmTaskState::Ready) {
        task.compliance_missing_files = entries.clone();
        let task_id_owned = task.id.clone();
        push_system_message_to_mission(
            state,
            &run.mission_id,
            format!(
                "STRUCTURAL COMPLIANCE: task '{task_id_owned}' has {} blueprint gap(s); attaching to in-flight sign-off retry.",
                entries.len(),
            ),
        );
        return;
    }
    if task.retries < MAX_CONTINUATION_RETRIES {
        task.retries = task.retries.saturating_add(1);
        task.state = SwarmTaskState::Ready;
        task.failed = false;
        task.expected_artifacts_missing = false;
        task.compliance_missing_files = entries.clone();
        let attempt = task.retries;
        let task_id_owned = task.id.clone();
        let preview: Vec<String> = entries.iter().take(5).cloned().collect();
        let more = if entries.len() > preview.len() {
            format!(" (+{} more)", entries.len() - preview.len())
        } else {
            String::new()
        };
        push_system_message_to_mission(
            state,
            &run.mission_id,
            format!(
                "STRUCTURAL COMPLIANCE: task '{task_id_owned}' has {} blueprint gap(s): {}{more}. Re-dispatching as continuation (attempt {attempt}/{MAX_CONTINUATION_RETRIES}).",
                entries.len(),
                preview.join("; "),
            ),
        );
    } else {
        task.compliance_missing_files = entries.clone();
        push_system_message_to_mission(
            state,
            &run.mission_id,
            format!(
                "STRUCTURAL COMPLIANCE: task '{task_id}' still has {} blueprint gap(s) after {MAX_CONTINUATION_RETRIES} continuation attempt(s) — accepting partial result and letting the verifier flag the gap.",
                entries.len(),
            ),
        );
    }
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
        prompt: build_synthesis_prompt(state, run),
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
    drop_pending_genome_review_at_synth_terminal(state, run);
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
        prompt: build_synthesis_prompt(state, run),
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
    drop_pending_genome_review_at_synth_terminal(state, run);
    cleanup_swarm_clones_for_mission(state, &run.mission_id);
    RunFate::Completed
}

// `swarm_stage_hint` falls back to `completed_runs`; a leftover
// `genome_review_pending` keeps the breather pinned to "genome review"
// after the run terminates. Drop it + leave a console breadcrumb.
fn drop_pending_genome_review_at_synth_terminal(state: &mut AppState, run: &mut SwarmRun) {
    if run.genome_review_pending.take().is_some() {
        push_system_message_to_mission(
            state,
            &run.mission_id,
            "Genome review skipped (synth completed first).".into(),
        );
    }
}

// Reviewer is advisory; never propagate to mission failure.
fn handle_genome_review_terminal(
    state: &mut AppState,
    run: &mut SwarmRun,
    agent_id: &str,
    message: &str,
    failed: bool,
) -> RunFate {
    tag_last_agent_message_kind(state, agent_id, &run.mission_id, "genome-review");
    let preview: String = message.chars().take(180).collect();
    let (verb, tail) = if failed {
        ("failed ", " — synthesis continues.")
    } else {
        ("", "")
    };
    push_system_message_to_mission(
        state,
        &run.mission_id,
        format!("Genome review {verb}({agent_id}): {preview}{tail}"),
    );
    RunFate::Active
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
                prompt: build_verify_prompt(state, run),
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
            prompt: build_synthesis_prompt(state, run),
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
