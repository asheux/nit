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
    SwarmArtifactFocus, SwarmDagValidationMode, SwarmDispatch, SwarmEventOutcome, SwarmRuntime,
    SwarmStage, SwarmTaskState, DEFAULT_DAG_VALIDATION_MODE,
};

/// Max number of times a task can be re-dispatched because its output failed
/// the completion sign-off check. After this many attempts we accept whatever
/// the agent produced and move on.
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

        // Chat clones are ad-hoc agents; they must never interact with swarm runs.
        let event_agent_id = match event {
            AgentBusEvent::TurnStarted { agent_id, .. }
            | AgentBusEvent::TurnCompleted { agent_id, .. }
            | AgentBusEvent::TurnFailed { agent_id, .. } => Some(agent_id.as_str()),
            _ => None,
        };
        if event_agent_id.is_some_and(is_chat_clone_agent_id) {
            return outcome;
        }

        let dispatches = &mut outcome.dispatches;

        match event {
            AgentBusEvent::TurnStarted {
                agent_id,
                mission_id: Some(mid),
                ..
            } => {
                if let Some(run) = self.runs.get_mut(mid) {
                    mark_task_running(run, agent_id);
                    update_mission_status(state, run, None);
                }
            }
            AgentBusEvent::TurnCompleted {
                agent_id,
                mission_id: Some(mid),
                message,
                ..
            } => {
                let Some(mut run) = self.runs.remove(mid) else {
                    return outcome;
                };
                match run.stage {
                    SwarmStage::Planning if agent_id == &run.planner_agent_id => {
                        tag_last_agent_message_kind(state, agent_id, &run.mission_id, "plan");
                        let available = run
                            .agent_ids
                            .iter()
                            .filter(|id| *id != &run.planner_agent_id)
                            .cloned()
                            .collect::<Vec<_>>();
                        let multi_integrator = run.scope_files.len() > 15;
                        let mut parsed = parse_plan_from_planner(
                            message,
                            run.template,
                            run.mission_kind,
                            &run.root_prompt,
                            &available,
                            run.integrator_agent_id.as_deref(),
                            run.integrator_locked,
                            multi_integrator,
                        );
                        // Synthesize missing critical tasks (integrate,
                        // proposer) BEFORE role-dep ordering so downstream
                        // roles (test, review) see them as producers and get
                        // the correct deps wired. Previously the synthesis
                        // ran after role-deps, so test/review had no
                        // integrate dep when the planner omitted the task and
                        // dispatched in parallel with proposers.
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
                            &available,
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
                            &available,
                        ));

                        let dag_mode = match read_workspace_dag_validation_mode(
                            state.workspace_root.as_path(),
                        ) {
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

                        let mut abort_execution = false;
                        let dag_issues = analyze_swarm_dag(parsed.tasks.as_slice());
                        if !dag_issues.is_empty() {
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
                                    for task in parsed.tasks.iter_mut() {
                                        if task.state.is_terminal() {
                                            continue;
                                        }
                                        task.state = SwarmTaskState::Skipped;
                                        task.failed = true;
                                        if task.output.is_none() {
                                            task.output = Some(
                                                "SKIPPED (preflight: invalid task DAG)".into(),
                                            );
                                        }
                                    }
                                    abort_execution = true;
                                }
                                SwarmDagValidationMode::Repair => {
                                    let mut warnings =
                                        repair_swarm_dag(parsed.tasks.as_mut_slice());
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
                                        for task in parsed.tasks.iter_mut() {
                                            if task.state.is_terminal() {
                                                continue;
                                            }
                                            task.state = SwarmTaskState::Skipped;
                                            task.failed = true;
                                            if task.output.is_none() {
                                                task.output = Some(
                                                    "SKIPPED (preflight: DAG auto-repair failed)"
                                                        .into(),
                                                );
                                            }
                                        }
                                        abort_execution = true;
                                    } else if warnings.is_empty() {
                                        warnings.push(
                                            "DAG repair: plan had DAG issues; no changes needed."
                                                .into(),
                                        );
                                    }
                                    parsed.warnings.append(&mut warnings);
                                }
                            }
                        }

                        for warning in parsed.warnings.iter() {
                            push_system_message_to_mission(
                                state,
                                &run.mission_id,
                                format!("PLAN warning: {warning}"),
                            );
                        }
                        if abort_execution {
                            abort_swarm_plan_preflight(state, &mut run, parsed);
                            cleanup_swarm_clones_for_mission(state, &run.mission_id);
                            self.completed_runs.insert(mid.clone(), run);
                            return outcome;
                        }
                        let prev_integrator = run.integrator_agent_id.clone();
                        let prev_verifier = run.verifier_agent_id.clone();
                        if parsed.integrator_agent_id.is_some() {
                            run.integrator_agent_id = parsed.integrator_agent_id.clone();
                        }
                        if run.gate_bundle.is_some() {
                            run.verifier_agent_id = {
                                if let Some(integrator) = run.integrator_agent_id.as_deref() {
                                    run.agent_ids
                                        .iter()
                                        .find(|id| {
                                            id.as_str() != run.planner_agent_id.as_str()
                                                && id.as_str() != integrator
                                        })
                                        .cloned()
                                        .or_else(|| {
                                            run.agent_ids
                                                .iter()
                                                .find(|id| {
                                                    id.as_str() != run.planner_agent_id.as_str()
                                                })
                                                .cloned()
                                        })
                                } else {
                                    run.agent_ids
                                        .iter()
                                        .find(|id| id.as_str() != run.planner_agent_id.as_str())
                                        .cloned()
                                }
                            };
                        }
                        if prev_integrator != run.integrator_agent_id
                            || prev_verifier != run.verifier_agent_id
                        {
                            push_system_message_to_mission(
                                state,
                                &run.mission_id,
                                format!(
                                    "Swarm template: {} | integrator: {} | verifier: {} | gates: {}",
                                    run.template.label(),
                                    run.integrator_agent_id
                                        .as_deref()
                                        .unwrap_or("(none)"),
                                    run.verifier_agent_id.as_deref().unwrap_or("(none)"),
                                    gate_bundle_label(
                                        run.gate_bundle.as_ref(),
                                        run.gate_selection.as_str()
                                    )
                                ),
                            );
                        }
                        run.tasks = parsed.tasks;
                        initialize_task_graph(&mut run);
                        run.synthesis_prompt = parsed.synthesis_prompt;
                        run.stage = SwarmStage::Executing;
                        update_mission_phase(state, &run.mission_id, MissionPhase::Execute);
                        refresh_task_readiness(&mut run);
                        emit_unresolved_dep_signals(state, &run);
                        dispatches.extend(dispatch_ready_tasks(&mut run));
                        if let Some(deadlock) = maybe_resolve_deadlock(&mut run) {
                            push_system_message_to_mission(
                                state,
                                &run.mission_id,
                                deadlock.message,
                            );
                        }
                        let done = tasks_terminal_count(&run.tasks);
                        update_mission_status(state, &run, Some(done));
                        if done == run.tasks.len() {
                            if let (Some(label), Some(verifier)) =
                                (run_gates_label(&run), run.verifier_agent_id.clone())
                            {
                                run.stage = SwarmStage::Verifying;
                                update_mission_phase(state, &run.mission_id, MissionPhase::Verify);
                                update_mission_status(state, &run, Some(done));
                                if state.settings.genome.genome_gate_enabled {
                                    // Dispatch genome gate to background thread;
                                    // verifier will be dispatched when result arrives
                                    // (polled via poll_genome_gates).
                                    run.genome_gate_pending = Some(GenomeGatePending {
                                        rx: spawn_genome_gate_eval(
                                            state,
                                            &run.mission_id,
                                            &run.initial_genome_baselines,
                                        ),
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
                                        format!("Starting VERIFY ({label}) on agent {verifier}",),
                                    );
                                    let prompt = build_verify_prompt(&run);
                                    dispatches.push(SwarmDispatch {
                                        agent_id: verifier,
                                        mission_id: run.mission_id.clone(),
                                        prompt,
                                        task_role: None,
                                    });
                                }
                            } else {
                                run.stage = SwarmStage::Synthesizing;
                                update_mission_phase(state, &run.mission_id, MissionPhase::Report);
                                update_mission_status(state, &run, Some(done));
                                let prompt = build_synthesis_prompt(&run);
                                dispatches.push(SwarmDispatch {
                                    agent_id: run.planner_agent_id.clone(),
                                    mission_id: run.mission_id.clone(),
                                    prompt,
                                    task_role: None,
                                });
                            }
                        }
                        self.runs.insert(mid.clone(), run);
                    }
                    SwarmStage::Executing => {
                        let agent_has_writes = state
                            .genome_turn_modified
                            .get(agent_id)
                            .is_some_and(|files| !files.is_empty());
                        if let Some(completed) = mark_task_finished(
                            &mut run,
                            agent_id,
                            message.clone(),
                            false,
                            agent_has_writes,
                        ) {
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
                            // Structural compliance: did the integrator touch
                            // every file the proposer declared?
                            let missing = structural_compliance_missing_files(
                                &run,
                                &completed.task_id,
                                state,
                            );
                            if !missing.is_empty() {
                                let preview: Vec<String> =
                                    missing.iter().take(8).cloned().collect();
                                let more = if missing.len() > preview.len() {
                                    format!(" (+{} more)", missing.len() - preview.len())
                                } else {
                                    String::new()
                                };
                                push_system_message_to_mission(
                                    state,
                                    &run.mission_id,
                                    format!(
                                        "STRUCTURAL COMPLIANCE: task '{}' finished but {} proposer-declared file(s) were not modified: {}{more}. \
                                         Silent divergence — integrator skipped part of the plan.",
                                        completed.task_id,
                                        missing.len(),
                                        preview.join(", "),
                                    ),
                                );
                                // Also emit a substrate Warning signal so
                                // observers/arbiters can react.
                                let id = state.substrate.next_signal_id(&completed.task_id);
                                let posted_at_gen = state.substrate.current_generation();
                                state.substrate.emit_signal(nit_core::substrate::Signal {
                                    id,
                                    kind: nit_core::substrate::SignalKind::Warning,
                                    posted_by: format!("swarm:compliance:{}", completed.task_id),
                                    posted_at_gen,
                                    target: nit_core::substrate::SignalTarget::Global,
                                    initial_strength:
                                        nit_core::substrate::SubstrateState::DEFAULT_INITIAL_STRENGTH,
                                    payload: serde_json::json!({
                                        "reason": "structural_compliance_missing_files",
                                        "task_id": completed.task_id,
                                        "missing_files": missing,
                                        "mission_id": run.mission_id,
                                    }),
                                });
                            }
                            // Completion sign-off check: if the agent's output
                            // looks like an early exit (no sentinel, or ends
                            // with an "shall I proceed?" style question) and
                            // we still have retry budget, revert the task to
                            // Ready so the next `dispatch_ready_tasks` cycle
                            // re-dispatches it with a continuation prompt.
                            if let Some(reason) = detect_incomplete_signoff(message) {
                                if let Some(task) = run
                                    .tasks
                                    .iter_mut()
                                    .find(|t| t.id == completed.task_id)
                                {
                                    if task.retries < MAX_CONTINUATION_RETRIES {
                                        task.retries = task.retries.saturating_add(1);
                                        task.state = SwarmTaskState::Ready;
                                        task.failed = false;
                                        task.expected_artifacts_missing = false;
                                        let attempt = task.retries;
                                        let task_id = task.id.clone();
                                        push_system_message_to_mission(
                                            state,
                                            &run.mission_id,
                                            format!(
                                                "SIGN-OFF: task '{task_id}' output flagged as incomplete ({reason}). Re-dispatching with continuation (attempt {attempt}/{MAX_CONTINUATION_RETRIES})."
                                            ),
                                        );
                                    } else {
                                        push_system_message_to_mission(
                                            state,
                                            &run.mission_id,
                                            format!(
                                                "SIGN-OFF: task '{}' output flagged as incomplete ({reason}), retry budget exhausted — accepting partial result.",
                                                completed.task_id
                                            ),
                                        );
                                    }
                                }
                            }
                        }
                        refresh_task_readiness(&mut run);
                        emit_unresolved_dep_signals(state, &run);
                        dispatches.extend(dispatch_ready_tasks(&mut run));
                        if let Some(deadlock) = maybe_resolve_deadlock(&mut run) {
                            push_system_message_to_mission(
                                state,
                                &run.mission_id,
                                deadlock.message,
                            );
                        }
                        let done = tasks_terminal_count(&run.tasks);
                        update_mission_status(state, &run, Some(done));
                        if done == run.tasks.len() {
                            if let (Some(label), Some(verifier)) =
                                (run_gates_label(&run), run.verifier_agent_id.clone())
                            {
                                run.stage = SwarmStage::Verifying;
                                update_mission_phase(state, &run.mission_id, MissionPhase::Verify);
                                update_mission_status(state, &run, Some(done));
                                if state.settings.genome.genome_gate_enabled {
                                    // Dispatch genome gate to background thread;
                                    // verifier will be dispatched when result arrives
                                    // (polled via poll_genome_gates).
                                    run.genome_gate_pending = Some(GenomeGatePending {
                                        rx: spawn_genome_gate_eval(
                                            state,
                                            &run.mission_id,
                                            &run.initial_genome_baselines,
                                        ),
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
                                        format!("Starting VERIFY ({label}) on agent {verifier}",),
                                    );
                                    let prompt = build_verify_prompt(&run);
                                    dispatches.push(SwarmDispatch {
                                        agent_id: verifier,
                                        mission_id: run.mission_id.clone(),
                                        prompt,
                                        task_role: None,
                                    });
                                }
                            } else {
                                run.stage = SwarmStage::Synthesizing;
                                update_mission_phase(state, &run.mission_id, MissionPhase::Report);
                                update_mission_status(state, &run, Some(done));
                                let prompt = build_synthesis_prompt(&run);
                                dispatches.push(SwarmDispatch {
                                    agent_id: run.planner_agent_id.clone(),
                                    mission_id: run.mission_id.clone(),
                                    prompt,
                                    task_role: None,
                                });
                            }
                        }
                        self.runs.insert(mid.clone(), run);
                    }
                    SwarmStage::Verifying => {
                        if run.verifier_agent_id.as_deref() != Some(agent_id.as_str()) {
                            self.runs.insert(mid.clone(), run);
                            return outcome;
                        }
                        run.gate_output = Some(message.clone());
                        run.gate_report = parse_gate_report(message);
                        if let Some(report) = run.gate_report.as_ref() {
                            let outcome = if report.overall_ok { "PASS" } else { "FAIL" };
                            push_system_message_to_mission(
                                state,
                                &run.mission_id,
                                format!("VERIFY result: {outcome}"),
                            );
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

                        if let Some(retry_dispatch) = try_dispatch_gate_retry(&mut run, state) {
                            dispatches.push(retry_dispatch);
                            self.runs.insert(mid.clone(), run);
                            return outcome;
                        }

                        maybe_spawn_genome_review(&mut run, state);

                        run.stage = SwarmStage::Synthesizing;
                        update_mission_phase(state, &run.mission_id, MissionPhase::Report);
                        update_mission_status(state, &run, Some(tasks_terminal_count(&run.tasks)));
                        let prompt = build_synthesis_prompt(&run);
                        dispatches.push(SwarmDispatch {
                            agent_id: run.planner_agent_id.clone(),
                            mission_id: run.mission_id.clone(),
                            prompt,
                            task_role: None,
                        });
                        self.runs.insert(mid.clone(), run);
                    }
                    SwarmStage::Synthesizing if agent_id == &run.planner_agent_id => {
                        run.report_status = Some("DONE".into());
                        run.report_output = Some(message.clone());
                        // Tag the message as a synthesis report.
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
                        self.completed_runs.insert(mid.clone(), run);
                    }
                    _ => {
                        self.runs.insert(mid.clone(), run);
                    }
                }
            }
            AgentBusEvent::TurnFailed {
                agent_id,
                mission_id: Some(mid),
                message,
                ..
            } => {
                let Some(mut run) = self.runs.remove(mid) else {
                    return outcome;
                };

                match run.stage {
                    SwarmStage::Planning if agent_id == &run.planner_agent_id => {
                        tag_last_agent_message_kind(state, agent_id, &run.mission_id, "plan");
                        let available = run
                            .agent_ids
                            .iter()
                            .filter(|id| *id != &run.planner_agent_id)
                            .cloned()
                            .collect::<Vec<_>>();
                        let mut parsed = fallback_tasks(
                            run.template,
                            run.mission_kind,
                            &run.root_prompt,
                            &available,
                            Some(message),
                            run.integrator_agent_id.as_deref(),
                        );
                        // Synthesize critical tasks BEFORE role-dep ordering
                        // (see matching block above for why).
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
                            &available,
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
                            run.scope_files.len() > 15,
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
                            &available,
                        ));

                        let dag_mode = match read_workspace_dag_validation_mode(
                            state.workspace_root.as_path(),
                        ) {
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

                        let mut abort_execution = false;
                        let dag_issues = analyze_swarm_dag(parsed.tasks.as_slice());
                        if !dag_issues.is_empty() {
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
                                    for task in parsed.tasks.iter_mut() {
                                        if task.state.is_terminal() {
                                            continue;
                                        }
                                        task.state = SwarmTaskState::Skipped;
                                        task.failed = true;
                                        if task.output.is_none() {
                                            task.output = Some(
                                                "SKIPPED (preflight: invalid task DAG)".into(),
                                            );
                                        }
                                    }
                                    abort_execution = true;
                                }
                                SwarmDagValidationMode::Repair => {
                                    let mut warnings =
                                        repair_swarm_dag(parsed.tasks.as_mut_slice());
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
                                        for task in parsed.tasks.iter_mut() {
                                            if task.state.is_terminal() {
                                                continue;
                                            }
                                            task.state = SwarmTaskState::Skipped;
                                            task.failed = true;
                                            if task.output.is_none() {
                                                task.output = Some(
                                                    "SKIPPED (preflight: DAG auto-repair failed)"
                                                        .into(),
                                                );
                                            }
                                        }
                                        abort_execution = true;
                                    } else if warnings.is_empty() {
                                        warnings.push(
                                            "DAG repair: plan had DAG issues; no changes needed."
                                                .into(),
                                        );
                                    }
                                    parsed.warnings.append(&mut warnings);
                                }
                            }
                        }

                        for warning in parsed.warnings.iter() {
                            push_system_message_to_mission(
                                state,
                                &run.mission_id,
                                format!("PLAN warning: {warning}"),
                            );
                        }
                        if abort_execution {
                            abort_swarm_plan_preflight(state, &mut run, parsed);
                            cleanup_swarm_clones_for_mission(state, &run.mission_id);
                            self.completed_runs.insert(mid.clone(), run);
                            return outcome;
                        }
                        let prev_integrator = run.integrator_agent_id.clone();
                        let prev_verifier = run.verifier_agent_id.clone();
                        if parsed.integrator_agent_id.is_some() {
                            run.integrator_agent_id = parsed.integrator_agent_id.clone();
                        }
                        if run.gate_bundle.is_some() {
                            run.verifier_agent_id = {
                                if let Some(integrator) = run.integrator_agent_id.as_deref() {
                                    run.agent_ids
                                        .iter()
                                        .find(|id| {
                                            id.as_str() != run.planner_agent_id.as_str()
                                                && id.as_str() != integrator
                                        })
                                        .cloned()
                                        .or_else(|| {
                                            run.agent_ids
                                                .iter()
                                                .find(|id| {
                                                    id.as_str() != run.planner_agent_id.as_str()
                                                })
                                                .cloned()
                                        })
                                } else {
                                    run.agent_ids
                                        .iter()
                                        .find(|id| id.as_str() != run.planner_agent_id.as_str())
                                        .cloned()
                                }
                            };
                        }
                        if prev_integrator != run.integrator_agent_id
                            || prev_verifier != run.verifier_agent_id
                        {
                            push_system_message_to_mission(
                                state,
                                &run.mission_id,
                                format!(
                                    "Swarm template: {} | integrator: {} | verifier: {} | gates: {}",
                                    run.template.label(),
                                    run.integrator_agent_id
                                        .as_deref()
                                        .unwrap_or("(none)"),
                                    run.verifier_agent_id.as_deref().unwrap_or("(none)"),
                                    gate_bundle_label(
                                        run.gate_bundle.as_ref(),
                                        run.gate_selection.as_str()
                                    )
                                ),
                            );
                        }
                        run.tasks = parsed.tasks;
                        run.synthesis_prompt = parsed.synthesis_prompt;
                        run.stage = SwarmStage::Executing;
                        update_mission_phase(state, &run.mission_id, MissionPhase::Execute);
                        initialize_task_graph(&mut run);
                        refresh_task_readiness(&mut run);
                        emit_unresolved_dep_signals(state, &run);
                        dispatches.extend(dispatch_ready_tasks(&mut run));
                        if let Some(deadlock) = maybe_resolve_deadlock(&mut run) {
                            push_system_message_to_mission(
                                state,
                                &run.mission_id,
                                deadlock.message,
                            );
                        }
                        let done = tasks_terminal_count(&run.tasks);
                        update_mission_status(state, &run, Some(done));
                        if done == run.tasks.len() {
                            if let (Some(label), Some(verifier)) =
                                (run_gates_label(&run), run.verifier_agent_id.clone())
                            {
                                run.stage = SwarmStage::Verifying;
                                update_mission_phase(state, &run.mission_id, MissionPhase::Verify);
                                update_mission_status(state, &run, Some(done));
                                if state.settings.genome.genome_gate_enabled {
                                    // Dispatch genome gate to background thread;
                                    // verifier will be dispatched when result arrives
                                    // (polled via poll_genome_gates).
                                    run.genome_gate_pending = Some(GenomeGatePending {
                                        rx: spawn_genome_gate_eval(
                                            state,
                                            &run.mission_id,
                                            &run.initial_genome_baselines,
                                        ),
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
                                        format!("Starting VERIFY ({label}) on agent {verifier}",),
                                    );
                                    let prompt = build_verify_prompt(&run);
                                    dispatches.push(SwarmDispatch {
                                        agent_id: verifier,
                                        mission_id: run.mission_id.clone(),
                                        prompt,
                                        task_role: None,
                                    });
                                }
                            } else {
                                run.stage = SwarmStage::Synthesizing;
                                update_mission_phase(state, &run.mission_id, MissionPhase::Report);
                                update_mission_status(state, &run, Some(done));
                                let prompt = build_synthesis_prompt(&run);
                                dispatches.push(SwarmDispatch {
                                    agent_id: run.planner_agent_id.clone(),
                                    mission_id: run.mission_id.clone(),
                                    prompt,
                                    task_role: None,
                                });
                            }
                        }
                        self.runs.insert(mid.clone(), run);
                    }
                    SwarmStage::Executing => {
                        let agent_has_writes = state
                            .genome_turn_modified
                            .get(agent_id)
                            .is_some_and(|files| !files.is_empty());

                        // Retry write-role tasks once on failure — the agent may
                        // have crashed due to a transient issue (rate limit,
                        // context overflow, etc.).  Reset the task to Ready so it
                        // gets re-dispatched with its dependency outputs.
                        let mut retried = false;
                        let task_idx = run.tasks.iter().position(|t| {
                            t.agent_id == *agent_id
                                && matches!(
                                    t.state,
                                    SwarmTaskState::Running | SwarmTaskState::Dispatched
                                )
                        });
                        // Rate-limit check: if the failure was a 429 from
                        // the provider, retrying immediately just burns the
                        // retry budget on an exhausted quota. Skip the retry
                        // and surface a clear message so the operator knows
                        // why the task stalled.
                        let rate_limited = is_provider_rate_limit_failure(message);
                        if let Some(idx) = task_idx {
                            let task = &mut run.tasks[idx];
                            if rate_limited {
                                push_system_message_to_mission(
                                    state,
                                    &run.mission_id,
                                    format!(
                                        "Task '{}' failed: provider rate-limited (429). Not retrying — wait for the quota window to reset.",
                                        task.id,
                                    ),
                                );
                            } else if task.writes && task.retries < 1 {
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
                                retried = true;
                            }
                        }

                        if retried {
                            refresh_task_readiness(&mut run);
                            emit_unresolved_dep_signals(state, &run);
                            dispatches.extend(dispatch_ready_tasks(&mut run));
                            self.runs.insert(mid.clone(), run);
                        } else {
                            if let Some(completed) = mark_task_finished(
                                &mut run,
                                agent_id,
                                message.clone(),
                                true,
                                agent_has_writes,
                            ) {
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
                            // Drain orphaned queued turns for the failed agent so
                            // queue_len doesn't leak.
                            drain_queued_turns_for_agent(state, agent_id);
                            refresh_task_readiness(&mut run);
                            emit_unresolved_dep_signals(state, &run);
                            dispatches.extend(dispatch_ready_tasks(&mut run));
                            if let Some(deadlock) = maybe_resolve_deadlock(&mut run) {
                                push_system_message_to_mission(
                                    state,
                                    &run.mission_id,
                                    deadlock.message,
                                );
                            }
                            let done = tasks_terminal_count(&run.tasks);
                            update_mission_status(state, &run, Some(done));
                            if done == run.tasks.len() {
                                if let (Some(label), Some(verifier)) =
                                    (run_gates_label(&run), run.verifier_agent_id.clone())
                                {
                                    run.stage = SwarmStage::Verifying;
                                    update_mission_phase(
                                        state,
                                        &run.mission_id,
                                        MissionPhase::Verify,
                                    );
                                    update_mission_status(state, &run, Some(done));
                                    if state.settings.genome.genome_gate_enabled {
                                        run.genome_gate_pending = Some(GenomeGatePending {
                                            rx: spawn_genome_gate_eval(
                                                state,
                                                &run.mission_id,
                                                &run.initial_genome_baselines,
                                            ),
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
                                            format!(
                                                "Starting VERIFY ({label}) on agent {verifier}",
                                            ),
                                        );
                                        let prompt = build_verify_prompt(&run);
                                        dispatches.push(SwarmDispatch {
                                            agent_id: verifier,
                                            mission_id: run.mission_id.clone(),
                                            prompt,
                                            task_role: None,
                                        });
                                    }
                                } else {
                                    run.stage = SwarmStage::Synthesizing;
                                    update_mission_phase(
                                        state,
                                        &run.mission_id,
                                        MissionPhase::Report,
                                    );
                                    update_mission_status(state, &run, Some(done));
                                    let prompt = build_synthesis_prompt(&run);
                                    dispatches.push(SwarmDispatch {
                                        agent_id: run.planner_agent_id.clone(),
                                        mission_id: run.mission_id.clone(),
                                        prompt,
                                        task_role: None,
                                    });
                                }
                            }
                            self.runs.insert(mid.clone(), run);
                        }
                    }
                    SwarmStage::Verifying => {
                        if run.verifier_agent_id.as_deref() != Some(agent_id.as_str()) {
                            self.runs.insert(mid.clone(), run);
                            return outcome;
                        }
                        run.gate_output = Some(message.clone());
                        run.gate_report = None;
                        push_system_message_to_mission(
                            state,
                            &run.mission_id,
                            format!("VERIFY result: ERROR ({message})"),
                        );

                        maybe_spawn_genome_review(&mut run, state);

                        run.stage = SwarmStage::Synthesizing;
                        update_mission_phase(state, &run.mission_id, MissionPhase::Report);
                        update_mission_status(state, &run, Some(tasks_terminal_count(&run.tasks)));
                        let prompt = build_synthesis_prompt(&run);
                        dispatches.push(SwarmDispatch {
                            agent_id: run.planner_agent_id.clone(),
                            mission_id: run.mission_id.clone(),
                            prompt,
                            task_role: None,
                        });
                        self.runs.insert(mid.clone(), run);
                    }
                    SwarmStage::Synthesizing if agent_id == &run.planner_agent_id => {
                        run.report_status = Some("ERROR".into());
                        run.report_output = Some(message.clone());
                        tag_last_agent_message_kind(state, agent_id, &run.mission_id, "synth");
                        outcome.artifact_focus = Some(SwarmArtifactFocus::Report {
                            mission_id: run.mission_id.clone(),
                        });
                        update_mission_final(state, &run.mission_id, "ERROR");
                        cleanup_swarm_clones_for_mission(state, &run.mission_id);
                        self.completed_runs.insert(mid.clone(), run);
                    }
                    _ => {
                        self.runs.insert(mid.clone(), run);
                    }
                }
            }
            _ => {}
        }

        outcome
    }
}
