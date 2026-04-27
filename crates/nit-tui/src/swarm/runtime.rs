use std::sync::mpsc;

use nit_core::{AppState, MissionPhase, MissionRecord};

use super::{
    assign_clone_roles_for_parallel_coverage, blocked_on, build_planner_prompt,
    build_verify_prompt, classify_swarm_mission_kind, dashboard_gate_rows,
    deduplicate_inherited_role_hints, direct_role_hint_for_agent, ensure_size_clones,
    enumerate_scope_files, gate_bundle_label, is_priority_agent, next_mission_id,
    planner_role_hint_for_agent, push_system_message_to_mission, read_workspace_custom_gates,
    run_gates_label, stage_label, swarm_mission_title, task_state_dashboard_label, timestamp_label,
    GateBundle, SwarmDashboardView, SwarmDispatch, SwarmMissionKind, SwarmPersistenceView,
    SwarmRun, SwarmRuntime, SwarmSessionConfig, SwarmSize, SwarmStage, SwarmTaskDashboardRow,
    SwarmTaskPersistenceView, SwarmTaskState, SwarmTemplate,
};

impl SwarmRuntime {
    /// Look up the scope files for a running mission. Used by the TUI
    /// dispatch layer to inject role-specific context (e.g. the genome
    /// landscape appended to propose-role prompts) without threading the
    /// scope through every SwarmDispatch.
    pub fn scope_files_for_mission(&self, mission_id: &str) -> Option<&[String]> {
        self.runs
            .get(mission_id)
            .map(|run| run.scope_files.as_slice())
    }

    /// Test-only helper: overwrite the `scope_files` of an active run so
    /// landscape-augmentation tests don't have to drive the full planner
    /// flow. No-op when the mission id is unknown.
    #[cfg(test)]
    pub(crate) fn set_scope_files_for_test(&mut self, mission_id: &str, scope: Vec<String>) {
        if let Some(run) = self.runs.get_mut(mission_id) {
            run.scope_files = scope;
        }
    }

    /// Poll all pending genome gate evaluations.  When a background thread
    /// finishes, store the result in the run and return a `SwarmDispatch` so
    /// the main loop can kick off the verifier agent — without ever blocking.
    pub fn poll_genome_gates(&mut self, state: &mut AppState) -> Vec<SwarmDispatch> {
        let mut dispatches = Vec::new();
        for run in self.runs.values_mut() {
            let pending = match run.genome_gate_pending.take() {
                Some(p) => p,
                None => continue,
            };
            match pending.rx.try_recv() {
                Ok(result) => {
                    run.genome_gate_results = Some(result);
                    push_system_message_to_mission(
                        state,
                        &run.mission_id,
                        format!(
                            "Starting VERIFY ({}) on agent {}",
                            pending.label, pending.verifier,
                        ),
                    );
                    let prompt = build_verify_prompt(run);
                    dispatches.push(SwarmDispatch {
                        agent_id: pending.verifier,
                        mission_id: run.mission_id.clone(),
                        prompt,
                        task_role: None,
                    });
                }
                Err(mpsc::TryRecvError::Empty) => {
                    // Still computing — put it back.
                    run.genome_gate_pending = Some(pending);
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    // Thread panicked or was dropped — dispatch verifier
                    // without genome gate results so the swarm doesn't stall.
                    push_system_message_to_mission(
                        state,
                        &run.mission_id,
                        format!(
                            "Genome gate evaluation failed; starting VERIFY ({}) on agent {}",
                            pending.label, pending.verifier,
                        ),
                    );
                    let prompt = build_verify_prompt(run);
                    dispatches.push(SwarmDispatch {
                        agent_id: pending.verifier,
                        mission_id: run.mission_id.clone(),
                        prompt,
                        task_role: None,
                    });
                }
            }
        }
        dispatches
    }

    /// Poll all pending genome review prompt builds.  When a background
    /// thread finishes, dispatch the reviewer agent.  An empty result means
    /// the worker had nothing to evaluate (no modified files) — the reviewer
    /// is silently skipped.  Disconnected channels (worker panic / drop) are
    /// also skipped silently so the swarm never stalls.
    pub fn poll_genome_reviews(&mut self, state: &mut AppState) -> Vec<SwarmDispatch> {
        let mut dispatches = Vec::new();
        for run in self.runs.values_mut() {
            let pending = match run.genome_review_pending.take() {
                Some(p) => p,
                None => continue,
            };
            match pending.rx.try_recv() {
                Ok(prompt) => {
                    if prompt.is_empty() {
                        continue;
                    }
                    push_system_message_to_mission(
                        state,
                        &run.mission_id,
                        format!("Dispatching genome review to {}", pending.reviewer_id),
                    );
                    dispatches.push(SwarmDispatch {
                        agent_id: pending.reviewer_id,
                        mission_id: run.mission_id.clone(),
                        prompt,
                        task_role: Some("genome-reviewer".into()),
                    });
                }
                Err(mpsc::TryRecvError::Empty) => {
                    // Still computing — put it back.
                    run.genome_review_pending = Some(pending);
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    // Worker died — skip silently, don't stall the swarm.
                }
            }
        }
        dispatches
    }

    pub fn is_active_mission(&self, mission_id: &str) -> bool {
        self.runs.contains_key(mission_id)
    }

    /// Mission ids of every still-running swarm, in insertion order. Used
    /// by the abort orchestrator to find a fallback target when the
    /// operator's "current" mission has already terminated — e.g. after
    /// they aborted once, started a new swarm, then triggered abort
    /// again. Without this fallback, `selected_mission` keeps pointing
    /// at the *first* aborted mission and `/abort` keeps replying "not
    /// active" instead of killing the actual running work.
    pub fn active_mission_ids(&self) -> Vec<String> {
        self.runs.keys().cloned().collect()
    }

    /// True once the planner's synthesis step has moved the run into
    /// `completed_runs`. Use this as the source of truth for "is the swarm
    /// done" — per-agent message scans can miss clones whose tasks were
    /// skipped or never dispatched.
    pub fn mission_is_complete(&self, mission_id: &str) -> bool {
        self.completed_runs.contains_key(mission_id)
    }

    /// True when the mission was terminated by an explicit operator
    /// abort (`/abort`, Ctrl+C, Esc-Esc, Mission-tab `x`) rather than
    /// finishing naturally. Used by the chat-console breather to show
    /// "Aborted" instead of "Done", so the operator can tell at a
    /// glance whether the run completed or was cancelled.
    pub fn mission_was_aborted(&self, mission_id: &str) -> bool {
        self.completed_runs
            .get(mission_id)
            .is_some_and(|run| run.report_status.as_deref() == Some("ABORTED"))
    }

    /// Returns the current swarm stage label (e.g. "VERIFY", "SYNTH") for a mission.
    pub fn swarm_stage_label(&self, mission_id: &str) -> Option<&'static str> {
        self.run_for_mission(mission_id)
            .map(|run| stage_label(run.stage))
    }

    /// Optional hint describing what background work is blocking the current
    /// stage, e.g. `"genome gate"` while the pre-verify genome evaluation is
    /// running or `"genome review"` while the post-verify reviewer prompt is
    /// being built. Returning `Some` lets the UI explain why `Verifying …` or
    /// `Synthesizing …` appears to hang with no visible agent activity.
    pub fn swarm_stage_hint(&self, mission_id: &str) -> Option<&'static str> {
        let run = self.run_for_mission(mission_id)?;
        if run.genome_gate_pending.is_some() {
            return Some("genome gate");
        }
        if run.genome_review_pending.is_some() {
            return Some("genome review");
        }
        None
    }

    /// Returns the swarm configuration for a mission (active or completed)
    /// so follow-up prompts can reuse the same template and agent count.
    pub fn session_config(&self, mission_id: &str) -> Option<SwarmSessionConfig> {
        let run = self.run_for_mission(mission_id)?;
        Some(SwarmSessionConfig {
            template: run.template.label().to_string(),
            size: run.agent_ids.len(),
            planner_agent_id: run.planner_agent_id.clone(),
        })
    }

    /// Build a planner prompt for a follow-up, wrapping the user's raw text
    /// with the same planning instructions used for the initial `@swarm`.
    pub fn build_followup_planner_prompt(
        &self,
        state: &AppState,
        mission_id: &str,
        user_prompt: &str,
    ) -> Option<String> {
        let run = self.run_for_mission(mission_id)?;
        let mut role_hints: Vec<(String, String)> = Vec::new();
        if matches!(run.template, SwarmTemplate::Parallel | SwarmTemplate::Bulk) {
            for id in run
                .agent_ids
                .iter()
                .filter(|id| id.as_str() != run.planner_agent_id.as_str())
            {
                role_hints.push((
                    id.clone(),
                    planner_role_hint_for_agent(
                        &state.agents.swarm_role_by_agent_id,
                        id.as_str(),
                        run.integrator_agent_id.as_deref(),
                        run.mission_kind,
                    ),
                ));
            }
            deduplicate_inherited_role_hints(&mut role_hints, &state.agents.swarm_role_by_agent_id);
        }
        let mut priority_agent_ids: Vec<String> = Vec::new();
        if matches!(run.template, SwarmTemplate::Parallel | SwarmTemplate::Bulk) {
            for id in run
                .agent_ids
                .iter()
                .filter(|id| id.as_str() != run.planner_agent_id.as_str())
            {
                if is_priority_agent(state, id.as_str()) {
                    priority_agent_ids.push(id.clone());
                }
            }
        }
        // Phase 8: also inject cross-mission memory into follow-up prompts.
        let memory_scope_files = enumerate_scope_files(state.workspace_root.as_path(), user_prompt);
        let memory_scope_tokens = nit_core::mission_memory::path_tokens(&memory_scope_files);
        let memory_index = nit_core::mission_memory::load_or_build(state.workspace_root.as_path());
        let memory_exclude: Vec<&str> = vec![mission_id];
        let memory_hits = nit_core::mission_memory::retrieve_similar(
            &memory_index,
            user_prompt,
            &memory_scope_tokens,
            &memory_exclude,
            3,
        );
        Some(build_planner_prompt(
            user_prompt,
            run.template,
            run.mission_kind,
            &run.planner_agent_id,
            &run.agent_ids,
            run.integrator_agent_id.as_deref(),
            &role_hints,
            &priority_agent_ids,
            state.workspace_root.as_path(),
            &memory_hits,
        ))
    }

    /// Re-activate a completed swarm run so the planner can generate a new
    /// plan for a follow-up prompt.  Clears previous tasks/outputs while
    /// keeping agent assignments and gate config intact.
    pub fn reactivate_for_followup(&mut self, state: &mut AppState, mission_id: &str) -> bool {
        let Some(mut run) = self.completed_runs.remove(mission_id) else {
            // Already active or doesn't exist.
            return self.runs.contains_key(mission_id);
        };

        // Push the swarm meta message so the footer shows Mission/Gates.
        push_system_message_to_mission(
            state,
            mission_id,
            format!(
                "Swarm template: {} | mission: {} | integrator: {} | verifier: {} | gates: {}",
                run.template.label(),
                run.mission_kind.label(),
                run.integrator_agent_id.as_deref().unwrap_or("(none)"),
                run.verifier_agent_id.as_deref().unwrap_or("(none)"),
                gate_bundle_label(run.gate_bundle.as_ref(), &run.gate_selection),
            ),
        );

        run.stage = SwarmStage::Planning;
        run.tasks.clear();
        run.synthesis_prompt = None;
        run.gate_output = None;
        run.gate_report = None;
        run.report_status = None;
        run.report_output = None;
        run.gate_retry_count = 0;
        // Re-anchor mission baselines to the current state so the follow-up's
        // genome review and gate measure deltas from THIS follow-up's work —
        // not cumulative deltas from the original swarm's starting point.
        run.initial_genome_baselines = state.genome_reports.clone();
        // Drop files accumulated from the prior run; the follow-up should
        // only report on files it actually touches.
        state.genome_mission_modified.remove(mission_id);
        self.runs.insert(mission_id.to_string(), run);
        true
    }

    pub fn swarm_dashboard(&self, mission_id: &str) -> Option<SwarmDashboardView> {
        let run = self.run_for_mission(mission_id)?;
        let tasks = run
            .tasks
            .iter()
            .map(|task| SwarmTaskDashboardRow {
                id: task.id.clone(),
                title: task.title.clone(),
                role: task.role.clone(),
                agent_id: task.agent_id.clone(),
                state: task_state_dashboard_label(task.state).into(),
                deps: task.deps.clone(),
                blocked_on: blocked_on(run, task),
                writes: task.writes,
                done_when: task.done_when.clone(),
                output_present: task.output.is_some(),
            })
            .collect::<Vec<_>>();

        let mut done = 0usize;
        let mut failed = 0usize;
        let mut skipped = 0usize;
        let mut running = 0usize;
        let mut queued = 0usize;
        let mut pending = 0usize;
        for task in run.tasks.iter() {
            match task.state {
                SwarmTaskState::Done => done += 1,
                SwarmTaskState::Failed => failed += 1,
                SwarmTaskState::Skipped => skipped += 1,
                SwarmTaskState::Running => running += 1,
                SwarmTaskState::Ready | SwarmTaskState::Dispatched => queued += 1,
                SwarmTaskState::Pending => pending += 1,
            }
        }

        Some(SwarmDashboardView {
            mission_id: run.mission_id.clone(),
            template: run.template.label().into(),
            phase: stage_label(run.stage).into(),
            done,
            failed,
            skipped,
            running,
            queued,
            pending,
            tasks,
            gate_bundle: run_gates_label(run),
            gates: dashboard_gate_rows(run),
        })
    }

    pub fn swarm_persistence(&self, mission_id: &str) -> Option<SwarmPersistenceView> {
        let run = self.run_for_mission(mission_id)?;
        let tasks = run
            .tasks
            .iter()
            .map(|task| SwarmTaskPersistenceView {
                id: task.id.clone(),
                title: task.title.clone(),
                role: task.role.clone(),
                agent_id: task.agent_id.clone(),
                state: task_state_dashboard_label(task.state).into(),
                deps: task.deps.clone(),
                blocked_on: blocked_on(run, task),
                writes: task.writes,
                done_when: task.done_when.clone(),
                expected_artifacts: task.artifacts.clone(),
                expected_artifacts_missing: task.expected_artifacts_missing,
                output_present: task.output.is_some(),
                output: task.output.clone(),
                artifacts: task.parsed_artifacts.clone(),
            })
            .collect::<Vec<_>>();
        Some(SwarmPersistenceView {
            mission_id: run.mission_id.clone(),
            template: run.template.label().into(),
            phase: stage_label(run.stage).into(),
            gate_bundle: run_gates_label(run),
            gate_selection: run.gate_selection.clone(),
            gate_report: run.gate_report.clone(),
            gate_output: run.gate_output.clone(),
            report_status: run.report_status.clone(),
            report_agent_id: run
                .report_output
                .as_ref()
                .map(|_| run.planner_agent_id.clone()),
            report_output: run.report_output.clone(),
            tasks,
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub fn start(
        &mut self,
        state: &mut AppState,
        planner_agent_id: String,
        agent_ids: Vec<String>,
        size: SwarmSize,
        template: Option<String>,
        mission_kind: Option<SwarmMissionKind>,
        root_prompt: String,
    ) -> Option<(String, Vec<SwarmDispatch>)> {
        let mut agents = Vec::new();
        for agent_id in agent_ids {
            if agents.iter().any(|id: &String| id == &agent_id) {
                continue;
            }
            let is_codex = state
                .agents
                .agents
                .iter()
                .find(|lane| lane.id == agent_id)
                .is_some_and(|lane| lane.is_codex() || lane.is_claude());
            if is_codex {
                agents.push(agent_id);
            }
        }
        if !agents.iter().any(|id| id == &planner_agent_id) {
            agents.insert(0, planner_agent_id.clone());
        }

        let template_kind = super::parse_swarm_template(template.as_deref());
        let mission_kind = classify_swarm_mission_kind(&root_prompt, mission_kind);
        let mission_id = next_mission_id(state);

        let restore_roster_selected = state
            .agents
            .agents
            .get(state.agents.roster_selected)
            .map(|lane| lane.id.clone());

        ensure_size_clones(
            state,
            &mission_id,
            template_kind,
            size,
            planner_agent_id.as_str(),
            &mut agents,
        );

        if let Some(selected_id) = restore_roster_selected {
            if let Some(idx) = state
                .agents
                .agents
                .iter()
                .position(|lane| lane.id == selected_id)
            {
                state.agents.roster_selected = idx;
            }
        }
        if agents.len() < 2 {
            return None;
        }

        let title = swarm_mission_title(&root_prompt, &mission_id, template_kind, mission_kind);
        let at = timestamp_label(state);
        state.agents.missions.push(MissionRecord {
            id: mission_id.clone(),
            title,
            phase: MissionPhase::Plan,
            swarm: true,
            assigned_agents: agents.clone(),
            status: "PLAN".into(),
            updated_at: at.clone(),
        });
        state.agents.mission_selected = state.agents.missions.len().saturating_sub(1);
        state.agents.selected_mission = Some(mission_id.clone());

        state.agents.alerts.push(nit_core::AgentAlert {
            severity: nit_core::AgentAlertSeverity::Info,
            source: "swarm".into(),
            message: format!(
                "Created swarm mission {mission_id} with agents: {}",
                agents.join(", ")
            ),
            at,
        });

        let mut integrator_locked = false;
        let mut integrator_agent_id: Option<String> = None;
        if matches!(template_kind, SwarmTemplate::Parallel) {
            integrator_agent_id = agents
                .iter()
                .filter(|id| id.as_str() != planner_agent_id.as_str())
                .find(|id| {
                    direct_role_hint_for_agent(&state.agents.swarm_role_by_agent_id, id.as_str())
                        .as_deref()
                        == Some("integrate")
                })
                .cloned();
        }
        if matches!(template_kind, SwarmTemplate::Lab | SwarmTemplate::Bulk)
            || (matches!(template_kind, SwarmTemplate::Parallel) && integrator_agent_id.is_none())
        {
            let eligible = agents
                .iter()
                .filter(|id| id.as_str() != planner_agent_id.as_str())
                .cloned()
                .collect::<Vec<_>>();
            let priority_eligible = eligible
                .iter()
                .filter(|id| is_priority_agent(state, id.as_str()))
                .cloned()
                .collect::<Vec<_>>();
            let candidates = if !priority_eligible.is_empty() {
                priority_eligible
            } else {
                eligible
            };

            integrator_agent_id = candidates.first().cloned();

            if matches!(template_kind, SwarmTemplate::Bulk) {
                if let Some(integrator) = candidates
                    .iter()
                    .find(|id| {
                        state
                            .agents
                            .swarm_role_by_agent_id
                            .get(*id)
                            .map(|role| role.trim())
                            .filter(|role| !role.is_empty())
                            .is_some_and(|role| role.eq_ignore_ascii_case("integrate"))
                    })
                    .cloned()
                {
                    integrator_agent_id = Some(integrator);
                    integrator_locked = true;
                }
            }
        }

        // Parallel-template only: ensure clones cover propose + review/test
        // when the planner has a deliberate role hint. No-op when planner is
        // `all`/unset, so the existing "let the LLM decide" path is preserved.
        let coverage_assignments = assign_clone_roles_for_parallel_coverage(
            state,
            template_kind,
            planner_agent_id.as_str(),
            integrator_agent_id.as_deref(),
            &agents,
        );
        if !coverage_assignments.is_empty() {
            let summary = coverage_assignments
                .iter()
                .map(|(id, role)| format!("{id}={role}"))
                .collect::<Vec<_>>()
                .join(", ");
            push_system_message_to_mission(
                state,
                &mission_id,
                format!(
                    "Parallel role coverage: assigned clone roles to satisfy propose + review/test ({summary})"
                ),
            );
        }

        let mut role_hints: Vec<(String, String)> = Vec::new();
        if matches!(template_kind, SwarmTemplate::Parallel | SwarmTemplate::Bulk) {
            for id in agents
                .iter()
                .filter(|id| id.as_str() != planner_agent_id.as_str())
            {
                role_hints.push((
                    id.clone(),
                    planner_role_hint_for_agent(
                        &state.agents.swarm_role_by_agent_id,
                        id.as_str(),
                        integrator_agent_id.as_deref(),
                        mission_kind,
                    ),
                ));
            }
            deduplicate_inherited_role_hints(&mut role_hints, &state.agents.swarm_role_by_agent_id);
        }
        let mut priority_agent_ids: Vec<String> = Vec::new();
        if matches!(template_kind, SwarmTemplate::Parallel | SwarmTemplate::Bulk) {
            for id in agents
                .iter()
                .filter(|id| id.as_str() != planner_agent_id.as_str())
            {
                if is_priority_agent(state, id.as_str()) {
                    priority_agent_ids.push(id.clone());
                }
            }
        }

        // Phase 8: cross-mission structural memory — retrieve top-K
        // precedents to inject into the planner prompt.
        let memory_scope_files =
            enumerate_scope_files(state.workspace_root.as_path(), &root_prompt);
        let memory_scope_tokens = nit_core::mission_memory::path_tokens(&memory_scope_files);
        let memory_index = nit_core::mission_memory::load_or_build(state.workspace_root.as_path());
        let memory_exclude: Vec<&str> = vec![mission_id.as_str()];
        let memory_hits = nit_core::mission_memory::retrieve_similar(
            &memory_index,
            &root_prompt,
            &memory_scope_tokens,
            &memory_exclude,
            3,
        );

        let plan_prompt = build_planner_prompt(
            &root_prompt,
            template_kind,
            mission_kind,
            &planner_agent_id,
            &agents,
            integrator_agent_id.as_deref(),
            &role_hints,
            &priority_agent_ids,
            state.workspace_root.as_path(),
            &memory_hits,
        );

        let scope_files = enumerate_scope_files(state.workspace_root.as_path(), &root_prompt);

        // Load project-specific custom gates first; if defined, they fully
        // override the auto-detected language bundle.
        let custom_gates_result = read_workspace_custom_gates(state.workspace_root.as_path());
        let gate_custom = match custom_gates_result.as_ref() {
            Ok(gates) => gates.clone(),
            Err(_) => None,
        };
        let gate_selection = GateBundle::detect(state);
        let gate_bundle = gate_selection.bundle.clone();
        let gate_selection_source = match (custom_gates_result.as_ref(), gate_custom.as_ref()) {
            (Err(err), _) => format!("config-error:{err}|{}", gate_selection.source),
            (_, Some(gates)) => format!("custom({} gates)|{}", gates.len(), gate_selection.source),
            _ => gate_selection.source.clone(),
        };
        // Verifier is needed when we have either a bundle or custom gates.
        let has_gates = gate_custom.is_some() || gate_bundle.is_some();
        let verifier_agent_id = has_gates.then_some(()).and_then(|_| {
            let eligible = agents
                .iter()
                .filter(|id| id.as_str() != planner_agent_id.as_str())
                .cloned()
                .collect::<Vec<_>>();
            let priority_eligible = eligible
                .iter()
                .filter(|id| is_priority_agent(state, id.as_str()))
                .cloned()
                .collect::<Vec<_>>();
            let candidates = if !priority_eligible.is_empty() {
                priority_eligible
            } else {
                eligible
            };

            if let Some(integrator) = integrator_agent_id.as_deref() {
                candidates
                    .iter()
                    .find(|id| id.as_str() != integrator)
                    .cloned()
                    .or_else(|| candidates.first().cloned())
            } else {
                candidates.first().cloned()
            }
        });

        push_system_message_to_mission(
            state,
            &mission_id,
            format!(
                "Swarm template: {} | mission: {} | integrator: {} | verifier: {} | gates: {}",
                template_kind.label(),
                mission_kind.label(),
                integrator_agent_id.as_deref().unwrap_or("(none)"),
                verifier_agent_id.as_deref().unwrap_or("(none)"),
                gate_bundle_label(gate_bundle.as_ref(), &gate_selection_source)
            ),
        );

        self.completed_runs.remove(&mission_id);
        state.genome_mission_modified.remove(&mission_id);
        self.runs.insert(
            mission_id.clone(),
            SwarmRun {
                mission_id: mission_id.clone(),
                root_prompt,
                template: template_kind,
                mission_kind,
                planner_agent_id: planner_agent_id.clone(),
                integrator_agent_id: integrator_agent_id.clone(),
                integrator_locked,
                verifier_agent_id,
                gate_bundle,
                gate_custom,
                gate_selection: gate_selection_source,
                agent_ids: agents,
                stage: SwarmStage::Planning,
                tasks: Vec::new(),
                synthesis_prompt: None,
                gate_output: None,
                gate_report: None,
                genome_gate_results: None,
                genome_gate_pending: None,
                genome_review_pending: None,
                report_status: None,
                report_output: None,
                scope_files,
                initial_genome_baselines: state.genome_reports.clone(),
                gate_retry_count: 0,
            },
        );

        Some((
            mission_id.clone(),
            vec![SwarmDispatch {
                agent_id: planner_agent_id,
                mission_id,
                prompt: plan_prompt,
                task_role: None,
            }],
        ))
    }

    pub(super) fn run_for_mission(&self, mission_id: &str) -> Option<&SwarmRun> {
        self.runs
            .get(mission_id)
            .or_else(|| self.completed_runs.get(mission_id))
    }

    /// Abort a single in-flight swarm mission. Marks the run as cancelled
    /// (moving it from `runs` to `completed_runs` with a synthetic
    /// "ABORTED" status), drains the state-side queued turns for every
    /// agent in the run, pushes a `SYSTEM_ALERT_KIND` message to the
    /// mission console, and returns the list of agent ids whose
    /// in-flight subprocess turns the caller still needs to kill via
    /// the runner-level `CancelTurn` commands.
    ///
    /// Idempotent: returns an empty Vec when the mission is unknown or
    /// already complete. Doesn't tear down clones — keep them around so
    /// the operator can inspect what happened. The next swarm dispatch
    /// or `cleanup_swarm_clones_for_mission` call (already triggered on
    /// natural completion) handles teardown.
    pub fn abort_mission(&mut self, state: &mut AppState, mission_id: &str) -> Vec<String> {
        let Some(mut run) = self.runs.remove(mission_id) else {
            return Vec::new();
        };
        let agent_ids: Vec<String> = run.agent_ids.clone();
        run.report_status = Some("ABORTED".into());
        // Mark every non-terminal task as Skipped so the dashboard reflects
        // the abort instead of leaving them in Running/Pending.
        for task in run.tasks.iter_mut() {
            if !task.state.is_terminal() {
                task.state = SwarmTaskState::Skipped;
            }
        }
        self.completed_runs.insert(mission_id.to_string(), run);

        // Drop every queued turn for this mission's agents and bring
        // their queue_len counters back to zero — otherwise the UI shows
        // ghost queues for cancelled agents.
        super::clones::drain_queued_turns_for_mission_agents(state, &agent_ids);

        // Update the mission record in state for the UI. Compute the
        // timestamp first to release the &mut state borrow before
        // grabbing the mission record.
        let now = timestamp_label(state);
        if let Some(mission) = state
            .agents
            .missions
            .iter_mut()
            .find(|m| m.id == mission_id)
        {
            mission.status = "ABORTED".into();
            mission.phase = MissionPhase::Report;
            mission.updated_at = now;
        }

        super::push_system_alert_to_mission(
            state,
            mission_id,
            "Mission aborted by operator. In-flight turns are being killed; \
             queued turns dropped."
                .into(),
        );

        agent_ids
    }

    /// Abort every active swarm mission. Returns the union of agent ids
    /// across all aborted runs so the caller can dispatch a single
    /// `CancelAll` per runner instead of N CancelTurn calls.
    pub fn abort_all(&mut self, state: &mut AppState) -> Vec<String> {
        let mission_ids: Vec<String> = self.runs.keys().cloned().collect();
        let mut all_agents: Vec<String> = Vec::new();
        for mid in mission_ids {
            for agent in self.abort_mission(state, &mid) {
                if !all_agents.contains(&agent) {
                    all_agents.push(agent);
                }
            }
        }
        all_agents
    }
}
