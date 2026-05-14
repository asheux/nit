use std::sync::mpsc;

use nit_core::{AppState, MissionPhase, MissionRecord};

use super::{
    assign_clone_roles_for_parallel_coverage, blocked_on, build_planner_prompt,
    build_verify_prompt, classify_swarm_mission_kind, dashboard_gate_rows,
    deduplicate_inherited_role_hints, direct_role_hint_for_agent, ensure_size_clones,
    enumerate_scope_files, gate_bundle_label, is_priority_agent, next_mission_id,
    planner_role_hint_for_agent, push_system_message_to_mission, read_workspace_custom_gates,
    run_gates_label, stage_label, swarm_mission_title, task_state_dashboard_label, timestamp_label,
    Gate, GateBundle, SwarmDashboardView, SwarmDispatch, SwarmMissionKind, SwarmPersistenceView,
    SwarmRun, SwarmRuntime, SwarmSessionConfig, SwarmSize, SwarmStage, SwarmTaskDashboardRow,
    SwarmTaskPersistenceView, SwarmTaskState, SwarmTemplate,
};

impl SwarmRuntime {
    /// Used by the dispatch layer to inject role-specific context (e.g.
    /// the genome landscape appended to propose-role prompts) without
    /// threading scope through every dispatch.
    pub fn scope_files_for_mission(&self, mission_id: &str) -> Option<&[String]> {
        self.runs
            .get(mission_id)
            .map(|run| run.scope_files.as_slice())
    }

    #[cfg(test)]
    pub(crate) fn set_scope_files_for_test(&mut self, mission_id: &str, scope: Vec<String>) {
        if let Some(run) = self.runs.get_mut(mission_id) {
            run.scope_files = scope;
        }
    }

    /// Polls pending genome gate evaluations. When a background thread
    /// finishes, stores the result and returns dispatches that the main
    /// loop kicks off — without ever blocking.
    pub fn poll_genome_gates(&mut self, state: &mut AppState) -> Vec<SwarmDispatch> {
        let mut dispatches = Vec::new();
        for run in self.runs.values_mut() {
            let Some(pending) = run.genome_gate_pending.take() else {
                continue;
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
                    dispatches.push(SwarmDispatch {
                        agent_id: pending.verifier,
                        mission_id: run.mission_id.clone(),
                        prompt: build_verify_prompt(state, run),
                        task_role: None,
                    });
                }
                Err(mpsc::TryRecvError::Empty) => {
                    run.genome_gate_pending = Some(pending);
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    // Worker dropped — dispatch verifier without genome
                    // results so the swarm doesn't stall.
                    push_system_message_to_mission(
                        state,
                        &run.mission_id,
                        format!(
                            "Genome gate evaluation failed; starting VERIFY ({}) on agent {}",
                            pending.label, pending.verifier,
                        ),
                    );
                    dispatches.push(SwarmDispatch {
                        agent_id: pending.verifier,
                        mission_id: run.mission_id.clone(),
                        prompt: build_verify_prompt(state, run),
                        task_role: None,
                    });
                }
            }
        }
        dispatches
    }

    /// Polls pending genome review prompt builds. Empty prompt = nothing
    /// to evaluate; reviewer is silently skipped. Disconnected channels
    /// (worker panic / drop) are also skipped silently so the swarm never
    /// stalls.
    pub fn poll_genome_reviews(&mut self, state: &mut AppState) -> Vec<SwarmDispatch> {
        let mut dispatches = Vec::new();
        for run in self.runs.values_mut() {
            let Some(pending) = run.genome_review_pending.take() else {
                continue;
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
                    run.genome_review_pending = Some(pending);
                }
                Err(mpsc::TryRecvError::Disconnected) => {}
            }
        }
        dispatches
    }

    pub fn is_active_mission(&self, mission_id: &str) -> bool {
        self.runs.contains_key(mission_id)
    }

    /// Mission ids of every still-running swarm, in insertion order. Used
    /// by the abort orchestrator to find a fallback target when the
    /// operator's "current" mission has already terminated.
    pub fn active_mission_ids(&self) -> Vec<String> {
        self.runs.keys().cloned().collect()
    }

    /// Source of truth for "is the swarm done" — per-agent message scans
    /// can miss clones whose tasks were skipped or never dispatched.
    pub fn mission_is_complete(&self, mission_id: &str) -> bool {
        self.completed_runs.contains_key(mission_id)
    }

    /// True when terminated by an explicit operator abort rather than
    /// finishing naturally. Lets the breather show "Aborted" vs "Done".
    pub fn mission_was_aborted(&self, mission_id: &str) -> bool {
        self.completed_runs
            .get(mission_id)
            .is_some_and(|run| run.report_status.as_deref() == Some("ABORTED"))
    }

    pub fn swarm_stage_label(&self, mission_id: &str) -> Option<&'static str> {
        self.run_for_mission(mission_id)
            .map(|run| stage_label(run.stage))
    }

    /// Hint describing background work blocking the current stage (e.g.
    /// `"genome gate"` while the pre-verify evaluation is running). Lets
    /// the UI explain why `Verifying …` appears to hang with no visible
    /// agent activity.
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

    pub fn session_config(&self, mission_id: &str) -> Option<SwarmSessionConfig> {
        let run = self.run_for_mission(mission_id)?;
        Some(SwarmSessionConfig {
            template: run.template.label().to_string(),
            size: run.agent_ids.len(),
            planner_agent_id: run.planner_agent_id.clone(),
        })
    }

    pub fn build_followup_planner_prompt(
        &self,
        state: &AppState,
        mission_id: &str,
        user_prompt: &str,
    ) -> Option<String> {
        let run = self.run_for_mission(mission_id)?;
        let role_hints = role_hints_for_followup(state, run);
        let priority_agent_ids = priority_agents_for_followup(state, run);
        // Phase 8: cross-mission memory. Reuse the mission's spawn cwd so a
        // multipane followup keeps its pane's workspace.
        let spawn_cwd = run.spawn_cwd.as_path();
        let memory_hits = load_memory_hits(spawn_cwd, user_prompt, &[mission_id]);
        Some(build_planner_prompt(
            user_prompt,
            run.template,
            run.mission_kind,
            &run.planner_agent_id,
            &run.agent_ids,
            run.integrator_agent_id.as_deref(),
            &role_hints,
            &priority_agent_ids,
            spawn_cwd,
            &memory_hits,
        ))
    }

    /// Re-activates a completed swarm so the planner can produce a new
    /// plan for a follow-up prompt. Clears prior tasks/outputs while
    /// keeping agent assignments and gate config intact.
    pub fn reactivate_for_followup(&mut self, state: &mut AppState, mission_id: &str) -> bool {
        let Some(mut run) = self.completed_runs.remove(mission_id) else {
            return self.runs.contains_key(mission_id);
        };

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
        run.repair_round = 0;
        run.last_plan_json = None;
        run.prior_violations.clear();
        // Re-anchor baselines so the follow-up's genome review and gate
        // measure deltas from THIS follow-up's work — not cumulative
        // deltas from the original swarm's starting point.
        run.initial_genome_baselines = state.genome_reports.clone();
        state.genome_mission_modified.remove(mission_id);
        self.runs.insert(mission_id.to_string(), run);
        true
    }

    pub fn swarm_dashboard(
        &self,
        state: &AppState,
        mission_id: &str,
    ) -> Option<SwarmDashboardView> {
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

        let counts = TaskStateCounts::from_tasks(&run.tasks);

        Some(SwarmDashboardView {
            mission_id: run.mission_id.clone(),
            template: run.template.label().into(),
            phase: stage_label(run.stage).into(),
            done: counts.done,
            failed: counts.failed,
            skipped: counts.skipped,
            running: counts.running,
            queued: counts.queued,
            pending: counts.pending,
            tasks,
            gate_bundle: run_gates_label(run),
            gates: dashboard_gate_rows(state, run),
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
        self.start_with_budget_overrides(
            state,
            planner_agent_id,
            agent_ids,
            size,
            template,
            mission_kind,
            root_prompt,
            std::collections::HashMap::new(),
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn start_with_budget_overrides(
        &mut self,
        state: &mut AppState,
        planner_agent_id: String,
        agent_ids: Vec<String>,
        size: SwarmSize,
        template: Option<String>,
        mission_kind: Option<SwarmMissionKind>,
        root_prompt: String,
        prompt_budgets: std::collections::HashMap<String, usize>,
    ) -> Option<(String, Vec<SwarmDispatch>)> {
        let mut agents = filter_dispatchable_agents(state, &planner_agent_id, agent_ids);

        let template_kind = super::parse_swarm_template(template.as_deref());
        let mission_kind = classify_swarm_mission_kind(&root_prompt, mission_kind);
        let mission_id = next_mission_id(state);

        scale_clones_preserving_roster(
            state,
            &mission_id,
            template_kind,
            size,
            &planner_agent_id,
            &mut agents,
        );

        if agents.len() < 2 {
            return None;
        }

        record_mission_in_state(
            state,
            &mission_id,
            &root_prompt,
            template_kind,
            mission_kind,
            &agents,
        );

        let (integrator_agent_id, integrator_locked) =
            select_integrator(state, template_kind, &planner_agent_id, &agents);

        announce_parallel_coverage(
            state,
            template_kind,
            &planner_agent_id,
            integrator_agent_id.as_deref(),
            &agents,
            &mission_id,
        );

        let role_hints = compute_role_hints(
            state,
            template_kind,
            &planner_agent_id,
            &agents,
            integrator_agent_id.as_deref(),
            mission_kind,
        );
        let priority_agent_ids =
            compute_priority_agent_ids(state, template_kind, &planner_agent_id, &agents);

        // Resolve the spawn cwd once per mission. Single-pane:
        // `state.workspace_root`; multipane: the dispatching pane's cwd.
        // Every prompt builder, scope walk, gate detector and mission-memory
        // load is keyed off this — never `state.workspace_root` directly —
        // so a non-Rust pane doesn't inherit Rust gates from the harness.
        let spawn_cwd = crate::app::resolve_dispatch_cwd(state, &planner_agent_id);
        let memory_hits =
            load_memory_hits(spawn_cwd.as_path(), &root_prompt, &[mission_id.as_str()]);

        let plan_prompt = build_planner_prompt(
            &root_prompt,
            template_kind,
            mission_kind,
            &planner_agent_id,
            &agents,
            integrator_agent_id.as_deref(),
            &role_hints,
            &priority_agent_ids,
            spawn_cwd.as_path(),
            &memory_hits,
        );

        let scope_files = enumerate_scope_files(spawn_cwd.as_path(), &root_prompt);
        let gate_setup = resolve_gates(spawn_cwd.as_path());
        let verifier_agent_id = select_verifier(
            state,
            &planner_agent_id,
            integrator_agent_id.as_deref(),
            &agents,
            gate_setup.has_gates(),
        );

        push_system_message_to_mission(
            state,
            &mission_id,
            format!(
                "Swarm template: {} | mission: {} | integrator: {} | verifier: {} | gates: {}",
                template_kind.label(),
                mission_kind.label(),
                integrator_agent_id.as_deref().unwrap_or("(none)"),
                verifier_agent_id.as_deref().unwrap_or("(none)"),
                gate_bundle_label(gate_setup.bundle.as_ref(), &gate_setup.selection_source)
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
                spawn_cwd,
                planner_agent_id: planner_agent_id.clone(),
                integrator_agent_id: integrator_agent_id.clone(),
                integrator_locked,
                verifier_agent_id,
                gate_bundle: gate_setup.bundle,
                gate_custom: gate_setup.custom,
                gate_selection: gate_setup.selection_source,
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
                verifier_retry_budget: super::constants::VERIFIER_RETRY_BUDGET_DEFAULT,
                repair_round: 0,
                last_plan_json: None,
                prior_violations: Vec::new(),
                prompt_budget_defaults: self.prompt_budgets.clone(),
                prompt_budgets,
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

    /// Aborts a single in-flight swarm. Marks the run cancelled (moves it
    /// from `runs` to `completed_runs` with a synthetic "ABORTED" status),
    /// drains queued turns for every agent in the run, pushes a
    /// `SYSTEM_ALERT_KIND` message, and returns agent ids whose in-flight
    /// subprocess turns the caller still needs to kill via `CancelTurn`.
    ///
    /// Idempotent — empty Vec when the mission is unknown or already
    /// complete. Doesn't tear down clones; keeps them around so the
    /// operator can inspect what happened.
    pub fn abort_mission(&mut self, state: &mut AppState, mission_id: &str) -> Vec<String> {
        let Some(mut run) = self.runs.remove(mission_id) else {
            return Vec::new();
        };
        let agent_ids: Vec<String> = run.agent_ids.clone();
        run.report_status = Some("ABORTED".into());
        for task in run.tasks.iter_mut() {
            if !task.state.is_terminal() {
                task.state = SwarmTaskState::Skipped;
            }
        }
        self.completed_runs.insert(mission_id.to_string(), run);

        // Drop queued turns for this mission's agents and bring queue_len
        // back to zero — otherwise the UI shows ghost queues.
        super::clones::drain_queued_turns_for_mission_agents(state, &agent_ids);

        // Compute the timestamp first to release the &mut state borrow
        // before grabbing the mission record.
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

    /// Aborts every active swarm. Returns the union of agent ids across
    /// runs so the caller can dispatch a single `CancelAll` per runner
    /// instead of N CancelTurn calls.
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

#[derive(Default)]
struct TaskStateCounts {
    done: usize,
    failed: usize,
    skipped: usize,
    running: usize,
    queued: usize,
    pending: usize,
}

impl TaskStateCounts {
    fn from_tasks(tasks: &[super::SwarmTask]) -> Self {
        let mut counts = Self::default();
        for task in tasks.iter() {
            match task.state {
                SwarmTaskState::Done => counts.done += 1,
                SwarmTaskState::Failed => counts.failed += 1,
                SwarmTaskState::Skipped => counts.skipped += 1,
                SwarmTaskState::Running => counts.running += 1,
                SwarmTaskState::Ready | SwarmTaskState::Dispatched => counts.queued += 1,
                SwarmTaskState::Pending => counts.pending += 1,
            }
        }
        counts
    }
}

struct GateSetup {
    bundle: Option<GateBundle>,
    custom: Option<Vec<Gate>>,
    selection_source: String,
}

impl GateSetup {
    fn has_gates(&self) -> bool {
        self.bundle.is_some() || self.custom.is_some()
    }
}

fn filter_dispatchable_agents(
    state: &AppState,
    planner_agent_id: &str,
    agent_ids: Vec<String>,
) -> Vec<String> {
    let mut agents = Vec::new();
    for agent_id in agent_ids {
        if agents.iter().any(|id: &String| id == &agent_id) {
            continue;
        }
        let is_codex_or_claude = state
            .agents
            .agents
            .iter()
            .find(|lane| lane.id == agent_id)
            .is_some_and(|lane| lane.is_codex() || lane.is_claude());
        if is_codex_or_claude {
            agents.push(agent_id);
        }
    }
    if !agents.iter().any(|id| id == planner_agent_id) {
        agents.insert(0, planner_agent_id.to_string());
    }
    agents
}

fn scale_clones_preserving_roster(
    state: &mut AppState,
    mission_id: &str,
    template: SwarmTemplate,
    size: SwarmSize,
    planner_agent_id: &str,
    agents: &mut Vec<String>,
) {
    let restore_id = state
        .agents
        .agents
        .get(state.agents.roster_selected)
        .map(|lane| lane.id.clone());

    ensure_size_clones(state, mission_id, template, size, planner_agent_id, agents);

    if let Some(selected_id) = restore_id {
        if let Some(idx) = state
            .agents
            .agents
            .iter()
            .position(|lane| lane.id == selected_id)
        {
            state.agents.roster_selected = idx;
        }
    }
}

fn record_mission_in_state(
    state: &mut AppState,
    mission_id: &str,
    root_prompt: &str,
    template: SwarmTemplate,
    mission_kind: SwarmMissionKind,
    agents: &[String],
) {
    let title = swarm_mission_title(root_prompt, mission_id, template, mission_kind);
    let at = timestamp_label(state);
    state.agents.missions.push(MissionRecord {
        id: mission_id.to_string(),
        title,
        phase: MissionPhase::Plan,
        swarm: true,
        assigned_agents: agents.to_vec(),
        status: "PLAN".into(),
        updated_at: at.clone(),
    });
    state.agents.mission_selected = state.agents.missions.len().saturating_sub(1);
    state.agents.selected_mission = Some(mission_id.to_string());

    state.agents.alerts.push(nit_core::AgentAlert {
        severity: nit_core::AgentAlertSeverity::Info,
        source: "swarm".into(),
        message: format!(
            "Created swarm mission {mission_id} with agents: {}",
            agents.join(", ")
        ),
        at,
    });
}

fn select_integrator(
    state: &AppState,
    template: SwarmTemplate,
    planner_agent_id: &str,
    agents: &[String],
) -> (Option<String>, bool) {
    let mut integrator_locked = false;
    let mut integrator: Option<String> = None;

    if matches!(template, SwarmTemplate::Parallel) {
        integrator = agents
            .iter()
            .filter(|id| id.as_str() != planner_agent_id)
            .find(|id| {
                direct_role_hint_for_agent(&state.agents.swarm_role_by_agent_id, id.as_str())
                    .as_deref()
                    == Some("integrate")
            })
            .cloned();
    }

    let needs_fallback_pick = matches!(template, SwarmTemplate::Lab | SwarmTemplate::Bulk)
        || (matches!(template, SwarmTemplate::Parallel) && integrator.is_none());

    if needs_fallback_pick {
        let candidates = candidate_pool(state, planner_agent_id, agents);
        integrator = candidates.first().cloned();

        if matches!(template, SwarmTemplate::Bulk) {
            if let Some(forced) = candidates
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
                integrator = Some(forced);
                integrator_locked = true;
            }
        }
    }

    (integrator, integrator_locked)
}

fn candidate_pool(state: &AppState, planner_agent_id: &str, agents: &[String]) -> Vec<String> {
    let eligible: Vec<String> = agents
        .iter()
        .filter(|id| id.as_str() != planner_agent_id)
        .cloned()
        .collect();
    let priority_eligible: Vec<String> = eligible
        .iter()
        .filter(|id| is_priority_agent(state, id.as_str()))
        .cloned()
        .collect();
    if !priority_eligible.is_empty() {
        priority_eligible
    } else {
        eligible
    }
}

fn announce_parallel_coverage(
    state: &mut AppState,
    template: SwarmTemplate,
    planner_agent_id: &str,
    integrator_agent_id: Option<&str>,
    agents: &[String],
    mission_id: &str,
) {
    let assignments = assign_clone_roles_for_parallel_coverage(
        state,
        template,
        planner_agent_id,
        integrator_agent_id,
        agents,
    );
    if assignments.is_empty() {
        return;
    }
    let summary = assignments
        .iter()
        .map(|(id, role)| format!("{id}={role}"))
        .collect::<Vec<_>>()
        .join(", ");
    push_system_message_to_mission(
        state,
        mission_id,
        format!(
            "Parallel role coverage: assigned clone roles to satisfy propose + review/test ({summary})"
        ),
    );
}

fn compute_role_hints(
    state: &AppState,
    template: SwarmTemplate,
    planner_agent_id: &str,
    agents: &[String],
    integrator_agent_id: Option<&str>,
    mission_kind: SwarmMissionKind,
) -> Vec<(String, String)> {
    if !matches!(template, SwarmTemplate::Parallel | SwarmTemplate::Bulk) {
        return Vec::new();
    }
    let mut role_hints: Vec<(String, String)> = agents
        .iter()
        .filter(|id| id.as_str() != planner_agent_id)
        .map(|id| {
            (
                id.clone(),
                planner_role_hint_for_agent(
                    &state.agents.swarm_role_by_agent_id,
                    id.as_str(),
                    integrator_agent_id,
                    mission_kind,
                ),
            )
        })
        .collect();
    deduplicate_inherited_role_hints(&mut role_hints, &state.agents.swarm_role_by_agent_id);
    role_hints
}

fn compute_priority_agent_ids(
    state: &AppState,
    template: SwarmTemplate,
    planner_agent_id: &str,
    agents: &[String],
) -> Vec<String> {
    if !matches!(template, SwarmTemplate::Parallel | SwarmTemplate::Bulk) {
        return Vec::new();
    }
    agents
        .iter()
        .filter(|id| id.as_str() != planner_agent_id)
        .filter(|id| is_priority_agent(state, id.as_str()))
        .cloned()
        .collect()
}

fn role_hints_for_followup(state: &AppState, run: &SwarmRun) -> Vec<(String, String)> {
    compute_role_hints(
        state,
        run.template,
        &run.planner_agent_id,
        &run.agent_ids,
        run.integrator_agent_id.as_deref(),
        run.mission_kind,
    )
}

fn priority_agents_for_followup(state: &AppState, run: &SwarmRun) -> Vec<String> {
    compute_priority_agent_ids(state, run.template, &run.planner_agent_id, &run.agent_ids)
}

fn load_memory_hits(
    spawn_cwd: &std::path::Path,
    prompt: &str,
    exclude_mission_ids: &[&str],
) -> Vec<nit_core::MissionHit> {
    let scope_files = enumerate_scope_files(spawn_cwd, prompt);
    let scope_tokens = nit_core::mission_memory::path_tokens(&scope_files);
    let index = nit_core::mission_memory::load_or_build(spawn_cwd);
    nit_core::mission_memory::retrieve_similar(
        &index,
        prompt,
        &scope_tokens,
        exclude_mission_ids,
        3,
    )
}

fn resolve_gates(spawn_cwd: &std::path::Path) -> GateSetup {
    let custom_gates_result = read_workspace_custom_gates(spawn_cwd);
    let custom = match custom_gates_result.as_ref() {
        Ok(gates) => gates.clone(),
        Err(_) => None,
    };
    let gate_selection = GateBundle::detect(spawn_cwd);
    let bundle = gate_selection.bundle.clone();
    let selection_source = match (custom_gates_result.as_ref(), custom.as_ref()) {
        (Err(err), _) => format!("config-error:{err}|{}", gate_selection.source),
        (_, Some(gates)) => format!("custom({} gates)|{}", gates.len(), gate_selection.source),
        _ => gate_selection.source.clone(),
    };
    GateSetup {
        bundle,
        custom,
        selection_source,
    }
}

fn select_verifier(
    state: &AppState,
    planner_agent_id: &str,
    integrator_agent_id: Option<&str>,
    agents: &[String],
    has_gates: bool,
) -> Option<String> {
    if !has_gates {
        return None;
    }
    let candidates = candidate_pool(state, planner_agent_id, agents);
    if let Some(integrator) = integrator_agent_id {
        candidates
            .iter()
            .find(|id| id.as_str() != integrator)
            .cloned()
            .or_else(|| candidates.first().cloned())
    } else {
        candidates.first().cloned()
    }
}
