use std::collections::{HashMap, HashSet};

use nit_core::{AgentBusEvent, AgentMessage, AgentStatus, AppState, MissionPhase, MissionRecord};

const DEFAULT_SWARM_SIZE: usize = 4;
const MAX_SWARM_SIZE: usize = 16;
const SWARM_VERIFY_MAX_CHARS: usize = 12_000;
const SWARM_DEP_OUTPUT_MAX_CHARS: usize = 8_000;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum SwarmSize {
    Default,
    All,
    Count(usize),
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum SwarmTemplate {
    /// Parallel task splitting (v1-style): keep tasks independent and preferably one per agent.
    Parallel,
    /// "Lab" workflow: read-only research/review feeding a single-writer integrator.
    Lab,
}

fn parse_swarm_template(value: Option<&str>) -> SwarmTemplate {
    let Some(value) = value else {
        return SwarmTemplate::Lab;
    };
    let value = value.trim();
    if value.eq_ignore_ascii_case("parallel") || value.eq_ignore_ascii_case("v1") {
        return SwarmTemplate::Parallel;
    }
    if value.eq_ignore_ascii_case("lab")
        || value.eq_ignore_ascii_case("default")
        || value.eq_ignore_ascii_case("v2")
    {
        return SwarmTemplate::Lab;
    }
    SwarmTemplate::Lab
}

impl SwarmTemplate {
    fn label(&self) -> &'static str {
        match self {
            SwarmTemplate::Parallel => "parallel",
            SwarmTemplate::Lab => "lab",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SwarmCommand {
    pub size: SwarmSize,
    pub template: Option<String>,
    pub prompt: String,
}

pub fn parse_swarm_command(raw: &str) -> Option<SwarmCommand> {
    let after = raw.trim_start().strip_prefix("@swarm")?;
    if after.is_empty() {
        return None;
    }
    if !after.starts_with(char::is_whitespace) {
        // Avoid treating "@swarmies" as a command.
        return None;
    }
    let mut rest = after.trim_start();
    if rest.is_empty() {
        return None;
    }

    let mut size = SwarmSize::Default;
    if let Some(next) = rest.split_whitespace().next() {
        if next.eq_ignore_ascii_case("all") {
            size = SwarmSize::All;
            rest = rest.strip_prefix(next).unwrap_or(rest).trim_start();
        } else if next.chars().all(|ch| ch.is_ascii_digit()) {
            if let Ok(n) = next.parse::<usize>() {
                size = SwarmSize::Count(n);
                rest = rest.strip_prefix(next).unwrap_or(rest).trim_start();
            }
        }
    }

    let mut template = None;
    if let Some(next) = rest.split_whitespace().next() {
        if let Some(value) = next
            .strip_prefix("template=")
            .or_else(|| next.strip_prefix("t="))
        {
            let value = value.trim();
            if !value.is_empty() {
                template = Some(value.to_ascii_lowercase());
            }
            rest = rest.strip_prefix(next).unwrap_or(rest).trim_start();
        }
    }

    let prompt = rest.to_string();
    if prompt.trim().is_empty() {
        return None;
    }

    Some(SwarmCommand {
        size,
        template,
        prompt,
    })
}

#[derive(Clone, Debug)]
pub struct SwarmDispatch {
    pub agent_id: String,
    pub mission_id: String,
    pub prompt: String,
}

#[derive(Default)]
pub struct SwarmRuntime {
    runs: HashMap<String, SwarmRun>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum SwarmStage {
    Planning,
    Executing,
    Verifying,
    Synthesizing,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum GateBundle {
    RustCi,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Gate {
    name: &'static str,
    command: &'static str,
}

impl GateBundle {
    fn detect(state: &AppState) -> Option<Self> {
        let mut cursor = state.workspace_root.as_path();
        loop {
            if cursor.join("Cargo.toml").exists() {
                return Some(Self::RustCi);
            }
            cursor = cursor.parent()?;
        }
    }

    fn label(&self) -> &'static str {
        match self {
            GateBundle::RustCi => "rust-ci",
        }
    }

    fn gates(&self) -> Vec<Gate> {
        match self {
            GateBundle::RustCi => vec![
                Gate {
                    name: "fmt",
                    command: "cargo fmt --all -- --check",
                },
                Gate {
                    name: "clippy",
                    command: "cargo clippy --all-targets --all-features -- -D warnings",
                },
                Gate {
                    name: "test",
                    command: "cargo test --workspace --all-features",
                },
            ],
        }
    }
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
struct GateReport {
    overall_ok: bool,
    gates: Vec<GateReportGate>,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
struct GateReportGate {
    name: String,
    command: String,
    ok: bool,
    #[serde(default)]
    notes: Option<String>,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum SwarmTaskState {
    Pending,
    Ready,
    Dispatched,
    Running,
    Done,
    Failed,
    Skipped,
}

impl SwarmTaskState {
    fn is_terminal(self) -> bool {
        matches!(
            self,
            SwarmTaskState::Done | SwarmTaskState::Failed | SwarmTaskState::Skipped
        )
    }
}

#[derive(Clone, Debug)]
struct SwarmTask {
    id: String,
    agent_id: String,
    role: Option<String>,
    title: String,
    task_prompt: String,
    deps: Vec<String>,
    writes: bool,
    artifacts: Vec<String>,
    done_when: Option<String>,
    state: SwarmTaskState,
    output: Option<String>,
    failed: bool,
}

#[derive(Clone, Debug)]
struct SwarmRun {
    mission_id: String,
    root_prompt: String,
    template: SwarmTemplate,
    planner_agent_id: String,
    integrator_agent_id: Option<String>,
    verifier_agent_id: Option<String>,
    gate_bundle: Option<GateBundle>,
    agent_ids: Vec<String>,
    stage: SwarmStage,
    tasks: Vec<SwarmTask>,
    synthesis_prompt: Option<String>,
    gate_output: Option<String>,
    gate_report: Option<GateReport>,
}

impl SwarmRuntime {
    pub fn is_active_mission(&self, mission_id: &str) -> bool {
        self.runs.contains_key(mission_id)
    }

    pub fn start(
        &mut self,
        state: &mut AppState,
        planner_agent_id: String,
        agent_ids: Vec<String>,
        template: Option<String>,
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
                .is_some_and(|lane| lane.is_codex());
            if is_codex {
                agents.push(agent_id);
            }
        }
        if agents.len() < 2 {
            return None;
        }
        if !agents.iter().any(|id| id == &planner_agent_id) {
            agents.insert(0, planner_agent_id.clone());
        }

        let mission_id = next_mission_id(state);
        let title = swarm_mission_title(&root_prompt, &mission_id);
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

        let template_kind = parse_swarm_template(template.as_deref());
        let integrator_agent_id = if matches!(template_kind, SwarmTemplate::Lab) {
            agents
                .iter()
                .find(|id| id.as_str() != planner_agent_id.as_str())
                .cloned()
        } else {
            None
        };
        let plan_prompt = build_planner_prompt(
            &root_prompt,
            template_kind,
            &planner_agent_id,
            &agents,
            integrator_agent_id.as_deref(),
        );

        let gate_bundle = GateBundle::detect(state);
        let verifier_agent_id = gate_bundle.as_ref().and_then(|_| {
            if let Some(integrator) = integrator_agent_id.as_deref() {
                agents
                    .iter()
                    .find(|id| {
                        id.as_str() != planner_agent_id.as_str() && id.as_str() != integrator
                    })
                    .cloned()
                    .or_else(|| {
                        agents
                            .iter()
                            .find(|id| id.as_str() != planner_agent_id.as_str())
                            .cloned()
                    })
            } else {
                agents
                    .iter()
                    .find(|id| id.as_str() != planner_agent_id.as_str())
                    .cloned()
            }
        });

        push_system_message_to_mission(
            state,
            &mission_id,
            format!(
                "Swarm template: {} | integrator: {} | verifier: {} | gates: {}",
                template_kind.label(),
                integrator_agent_id.as_deref().unwrap_or("(none)"),
                verifier_agent_id.as_deref().unwrap_or("(none)"),
                gate_bundle.as_ref().map(|b| b.label()).unwrap_or("(none)")
            ),
        );

        self.runs.insert(
            mission_id.clone(),
            SwarmRun {
                mission_id: mission_id.clone(),
                root_prompt,
                template: template_kind,
                planner_agent_id: planner_agent_id.clone(),
                integrator_agent_id: integrator_agent_id.clone(),
                verifier_agent_id,
                gate_bundle,
                agent_ids: agents,
                stage: SwarmStage::Planning,
                tasks: Vec::new(),
                synthesis_prompt: None,
                gate_output: None,
                gate_report: None,
            },
        );

        Some((
            mission_id.clone(),
            vec![SwarmDispatch {
                agent_id: planner_agent_id,
                mission_id,
                prompt: plan_prompt,
            }],
        ))
    }

    pub fn handle_event(
        &mut self,
        state: &mut AppState,
        event: &AgentBusEvent,
    ) -> Vec<SwarmDispatch> {
        let mut dispatches = Vec::new();

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
                    return dispatches;
                };
                match run.stage {
                    SwarmStage::Planning if agent_id == &run.planner_agent_id => {
                        let available = run
                            .agent_ids
                            .iter()
                            .filter(|id| *id != &run.planner_agent_id)
                            .cloned()
                            .collect::<Vec<_>>();
                        let parsed = parse_plan_from_planner(
                            message,
                            run.template,
                            &run.root_prompt,
                            &available,
                            run.integrator_agent_id.as_deref(),
                        );
                        for warning in parsed.warnings.iter() {
                            push_system_message_to_mission(
                                state,
                                &run.mission_id,
                                format!("PLAN warning: {warning}"),
                            );
                        }
                        if parsed.integrator_agent_id.is_some() {
                            run.integrator_agent_id = parsed.integrator_agent_id.clone();
                        }
                        run.tasks = parsed.tasks;
                        initialize_task_graph(&mut run);
                        run.synthesis_prompt = parsed.synthesis_prompt;
                        run.stage = SwarmStage::Executing;
                        update_mission_phase(state, &run.mission_id, MissionPhase::Execute);
                        refresh_task_readiness(&mut run);
                        dispatches.extend(dispatch_ready_tasks(&mut run));
                        let skipped = maybe_resolve_deadlock(&mut run);
                        if !skipped.is_empty() {
                            push_system_message_to_mission(
                                state,
                                &run.mission_id,
                                format!(
                                    "Swarm deadlock: skipping tasks with unresolvable deps: {}",
                                    skipped.join(", ")
                                ),
                            );
                        }
                        update_mission_status(state, &run, Some(tasks_terminal_count(&run.tasks)));
                        self.runs.insert(mid.clone(), run);
                    }
                    SwarmStage::Executing => {
                        let _ = mark_task_finished(&mut run, agent_id, message.clone(), false);
                        refresh_task_readiness(&mut run);
                        dispatches.extend(dispatch_ready_tasks(&mut run));
                        let skipped = maybe_resolve_deadlock(&mut run);
                        if !skipped.is_empty() {
                            push_system_message_to_mission(
                                state,
                                &run.mission_id,
                                format!(
                                    "Swarm deadlock: skipping tasks with unresolvable deps: {}",
                                    skipped.join(", ")
                                ),
                            );
                        }
                        let done = tasks_terminal_count(&run.tasks);
                        update_mission_status(state, &run, Some(done));
                        if done == run.tasks.len() {
                            if let (Some(bundle), Some(verifier)) =
                                (run.gate_bundle.clone(), run.verifier_agent_id.clone())
                            {
                                run.stage = SwarmStage::Verifying;
                                update_mission_phase(state, &run.mission_id, MissionPhase::Verify);
                                update_mission_status(state, &run, Some(done));
                                push_system_message_to_mission(
                                    state,
                                    &run.mission_id,
                                    format!(
                                        "Starting VERIFY ({}) on agent {verifier}",
                                        bundle.label()
                                    ),
                                );
                                let prompt = build_verify_prompt(&run, &bundle);
                                dispatches.push(SwarmDispatch {
                                    agent_id: verifier,
                                    mission_id: run.mission_id.clone(),
                                    prompt,
                                });
                            } else {
                                run.stage = SwarmStage::Synthesizing;
                                update_mission_phase(state, &run.mission_id, MissionPhase::Report);
                                update_mission_status(state, &run, Some(done));
                                let prompt = build_synthesis_prompt(&run);
                                dispatches.push(SwarmDispatch {
                                    agent_id: run.planner_agent_id.clone(),
                                    mission_id: run.mission_id.clone(),
                                    prompt,
                                });
                            }
                        }
                        self.runs.insert(mid.clone(), run);
                    }
                    SwarmStage::Verifying => {
                        if run.verifier_agent_id.as_deref() != Some(agent_id.as_str()) {
                            self.runs.insert(mid.clone(), run);
                            return dispatches;
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
                        } else {
                            push_system_message_to_mission(
                                state,
                                &run.mission_id,
                                "VERIFY result: ERROR (no parseable JSON report)".into(),
                            );
                        }

                        run.stage = SwarmStage::Synthesizing;
                        update_mission_phase(state, &run.mission_id, MissionPhase::Report);
                        update_mission_status(state, &run, Some(tasks_terminal_count(&run.tasks)));
                        let prompt = build_synthesis_prompt(&run);
                        dispatches.push(SwarmDispatch {
                            agent_id: run.planner_agent_id.clone(),
                            mission_id: run.mission_id.clone(),
                            prompt,
                        });
                        self.runs.insert(mid.clone(), run);
                    }
                    SwarmStage::Synthesizing if agent_id == &run.planner_agent_id => {
                        run.stage = SwarmStage::Synthesizing;
                        update_mission_phase(state, &run.mission_id, MissionPhase::Report);
                        let verify_ok = run.gate_bundle.is_none()
                            || run
                                .gate_report
                                .as_ref()
                                .is_some_and(|report| report.overall_ok);
                        let final_status = if verify_ok {
                            "DONE"
                        } else if run.gate_report.is_some() {
                            "FAILED"
                        } else {
                            "ERROR"
                        };
                        update_mission_final(state, &run.mission_id, final_status);
                        // keep messages already appended; drop runtime state.
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
                    return dispatches;
                };

                match run.stage {
                    SwarmStage::Planning if agent_id == &run.planner_agent_id => {
                        let available = run
                            .agent_ids
                            .iter()
                            .filter(|id| *id != &run.planner_agent_id)
                            .cloned()
                            .collect::<Vec<_>>();
                        let parsed = fallback_tasks(
                            run.template,
                            &run.root_prompt,
                            &available,
                            Some(message),
                        );
                        if parsed.integrator_agent_id.is_some() {
                            run.integrator_agent_id = parsed.integrator_agent_id.clone();
                        }
                        run.tasks = parsed.tasks;
                        run.synthesis_prompt = parsed.synthesis_prompt;
                        run.stage = SwarmStage::Executing;
                        update_mission_phase(state, &run.mission_id, MissionPhase::Execute);
                        initialize_task_graph(&mut run);
                        refresh_task_readiness(&mut run);
                        dispatches.extend(dispatch_ready_tasks(&mut run));
                        let skipped = maybe_resolve_deadlock(&mut run);
                        if !skipped.is_empty() {
                            push_system_message_to_mission(
                                state,
                                &run.mission_id,
                                format!(
                                    "Swarm deadlock: skipping tasks with unresolvable deps: {}",
                                    skipped.join(", ")
                                ),
                            );
                        }
                        update_mission_status(state, &run, Some(tasks_terminal_count(&run.tasks)));
                        self.runs.insert(mid.clone(), run);
                    }
                    SwarmStage::Executing => {
                        let _ = mark_task_finished(&mut run, agent_id, message.clone(), true);
                        refresh_task_readiness(&mut run);
                        dispatches.extend(dispatch_ready_tasks(&mut run));
                        let skipped = maybe_resolve_deadlock(&mut run);
                        if !skipped.is_empty() {
                            push_system_message_to_mission(
                                state,
                                &run.mission_id,
                                format!(
                                    "Swarm deadlock: skipping tasks with unresolvable deps: {}",
                                    skipped.join(", ")
                                ),
                            );
                        }
                        let done = tasks_terminal_count(&run.tasks);
                        update_mission_status(state, &run, Some(done));
                        if done == run.tasks.len() {
                            if let (Some(bundle), Some(verifier)) =
                                (run.gate_bundle.clone(), run.verifier_agent_id.clone())
                            {
                                run.stage = SwarmStage::Verifying;
                                update_mission_phase(state, &run.mission_id, MissionPhase::Verify);
                                update_mission_status(state, &run, Some(done));
                                push_system_message_to_mission(
                                    state,
                                    &run.mission_id,
                                    format!(
                                        "Starting VERIFY ({}) on agent {verifier}",
                                        bundle.label()
                                    ),
                                );
                                let prompt = build_verify_prompt(&run, &bundle);
                                dispatches.push(SwarmDispatch {
                                    agent_id: verifier,
                                    mission_id: run.mission_id.clone(),
                                    prompt,
                                });
                            } else {
                                run.stage = SwarmStage::Synthesizing;
                                update_mission_phase(state, &run.mission_id, MissionPhase::Report);
                                update_mission_status(state, &run, Some(done));
                                let prompt = build_synthesis_prompt(&run);
                                dispatches.push(SwarmDispatch {
                                    agent_id: run.planner_agent_id.clone(),
                                    mission_id: run.mission_id.clone(),
                                    prompt,
                                });
                            }
                        }
                        self.runs.insert(mid.clone(), run);
                    }
                    SwarmStage::Verifying => {
                        if run.verifier_agent_id.as_deref() != Some(agent_id.as_str()) {
                            self.runs.insert(mid.clone(), run);
                            return dispatches;
                        }
                        run.gate_output = Some(message.clone());
                        run.gate_report = None;
                        push_system_message_to_mission(
                            state,
                            &run.mission_id,
                            format!("VERIFY result: ERROR ({message})"),
                        );

                        run.stage = SwarmStage::Synthesizing;
                        update_mission_phase(state, &run.mission_id, MissionPhase::Report);
                        update_mission_status(state, &run, Some(tasks_terminal_count(&run.tasks)));
                        let prompt = build_synthesis_prompt(&run);
                        dispatches.push(SwarmDispatch {
                            agent_id: run.planner_agent_id.clone(),
                            mission_id: run.mission_id.clone(),
                            prompt,
                        });
                        self.runs.insert(mid.clone(), run);
                    }
                    SwarmStage::Synthesizing if agent_id == &run.planner_agent_id => {
                        update_mission_final(state, &run.mission_id, "ERROR");
                    }
                    _ => {
                        self.runs.insert(mid.clone(), run);
                    }
                }
            }
            _ => {}
        }

        dispatches
    }
}

#[derive(serde::Deserialize)]
struct SwarmPlanV2 {
    #[serde(default)]
    version: Option<u32>,
    #[serde(default)]
    template: Option<String>,
    #[serde(default)]
    integrator_agent_id: Option<String>,
    tasks: Vec<SwarmPlanTaskV2>,
    #[serde(default)]
    synthesis_prompt: Option<String>,
}

#[derive(serde::Deserialize)]
struct SwarmPlanTaskV2 {
    #[serde(default)]
    id: Option<String>,
    agent_id: String,
    #[serde(default)]
    role: Option<String>,
    title: String,
    prompt: String,
    #[serde(default)]
    deps: Vec<String>,
    #[serde(default)]
    writes: bool,
    #[serde(default)]
    artifacts: Vec<String>,
    #[serde(default)]
    done_when: Option<String>,
}

#[derive(serde::Deserialize)]
struct SwarmPlanV1 {
    tasks: Vec<SwarmPlanTaskV1>,
    #[serde(default)]
    synthesis_prompt: Option<String>,
}

#[derive(serde::Deserialize)]
struct SwarmPlanTaskV1 {
    agent_id: String,
    title: String,
    prompt: String,
}

struct ParsedSwarmPlan {
    tasks: Vec<SwarmTask>,
    synthesis_prompt: Option<String>,
    integrator_agent_id: Option<String>,
    warnings: Vec<String>,
}

fn parse_plan_from_planner(
    planner_message: &str,
    template: SwarmTemplate,
    root_prompt: &str,
    available_agents: &[String],
    integrator_hint: Option<&str>,
) -> ParsedSwarmPlan {
    let Some(json) = extract_json_code_block(planner_message) else {
        return fallback_tasks(template, root_prompt, available_agents, None);
    };

    if let Ok(plan) = serde_json::from_str::<SwarmPlanV2>(&json) {
        if let Some(parsed) = parse_v2_plan(plan, template, available_agents, integrator_hint) {
            return parsed;
        }
    }

    let Ok(plan) = serde_json::from_str::<SwarmPlanV1>(&json) else {
        return fallback_tasks(template, root_prompt, available_agents, None);
    };
    let mut tasks = Vec::new();
    let mut idx = 0usize;
    let mut seen_agents = HashSet::new();
    for task in plan.tasks.into_iter() {
        let agent_id = task.agent_id.trim().to_string();
        if agent_id.is_empty() {
            continue;
        }
        if available_agents.iter().all(|id| id != &agent_id) {
            continue;
        }
        // Keep v1 deterministic: at most one task per agent id.
        if !seen_agents.insert(agent_id.clone()) {
            continue;
        }
        let title = task.title.trim().to_string();
        let prompt = task.prompt.trim().to_string();
        if title.is_empty() || prompt.is_empty() {
            continue;
        }
        idx = idx.saturating_add(1);
        tasks.push(SwarmTask {
            id: format!("task-{idx:02}"),
            agent_id,
            role: None,
            title,
            task_prompt: prompt,
            deps: Vec::new(),
            writes: false,
            artifacts: Vec::new(),
            done_when: None,
            state: SwarmTaskState::Pending,
            output: None,
            failed: false,
        });
    }

    if tasks.is_empty() {
        return fallback_tasks(template, root_prompt, available_agents, None);
    }
    tasks.truncate(available_agents.len());

    ParsedSwarmPlan {
        tasks,
        synthesis_prompt: plan.synthesis_prompt,
        integrator_agent_id: None,
        warnings: Vec::new(),
    }
}

fn parse_v2_plan(
    plan: SwarmPlanV2,
    template: SwarmTemplate,
    available_agents: &[String],
    integrator_hint: Option<&str>,
) -> Option<ParsedSwarmPlan> {
    if plan.tasks.is_empty() {
        return None;
    }
    if let Some(version) = plan.version {
        if version != 2 {
            return None;
        }
    }

    let integrator_plan = plan.integrator_agent_id.as_deref().map(str::trim);
    let integrator = integrator_plan
        .filter(|id| !id.is_empty())
        .or(integrator_hint)
        .and_then(|id| {
            available_agents
                .iter()
                .find(|candidate| candidate.as_str() == id)
                .map(|id| id.to_string())
        });

    let mut warnings = Vec::new();
    if let Some(plan_template) = plan
        .template
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        let plan_template = parse_swarm_template(Some(plan_template));
        if plan_template != template {
            warnings.push(format!(
                "Planner returned template '{}' but swarm is running template '{}'; continuing with the swarm template.",
                plan_template.label(),
                template.label()
            ));
        }
    }
    let mut tasks = Vec::new();
    let mut seen_ids = HashSet::new();
    for (idx, task) in plan.tasks.into_iter().enumerate() {
        let agent_id = task.agent_id.trim().to_string();
        if agent_id.is_empty() {
            continue;
        }
        if available_agents.iter().all(|id| id != &agent_id) {
            continue;
        }

        let title = task.title.trim().to_string();
        let prompt = task.prompt.trim().to_string();
        if title.is_empty() || prompt.is_empty() {
            continue;
        }

        let id = task
            .id
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(ToString::to_string)
            .unwrap_or_else(|| format!("task-{:02}", idx + 1));
        if !seen_ids.insert(id.clone()) {
            warnings.push(format!(
                "Duplicate task id '{id}' in planner output; skipping."
            ));
            continue;
        }

        let mut writes = task.writes;
        if writes {
            let allowed = integrator
                .as_deref()
                .is_some_and(|integrator| integrator == agent_id.as_str());
            if !allowed {
                writes = false;
                warnings.push(format!(
                    "Planner marked task '{id}' as writes=true but agent '{agent_id}' is not the integrator; forcing read-only."
                ));
            }
        }

        let deps = task
            .deps
            .into_iter()
            .map(|dep| dep.trim().to_string())
            .filter(|dep| !dep.is_empty() && dep != &id)
            .collect::<Vec<_>>();
        let artifacts = task
            .artifacts
            .into_iter()
            .map(|a| a.trim().to_string())
            .filter(|a| !a.is_empty())
            .collect::<Vec<_>>();

        tasks.push(SwarmTask {
            id,
            agent_id,
            role: task
                .role
                .map(|r| r.trim().to_string())
                .filter(|r| !r.is_empty()),
            title,
            task_prompt: prompt,
            deps,
            writes,
            artifacts,
            done_when: task
                .done_when
                .map(|d| d.trim().to_string())
                .filter(|d| !d.is_empty()),
            state: SwarmTaskState::Pending,
            output: None,
            failed: false,
        });
    }

    if tasks.is_empty() {
        return None;
    }

    Some(ParsedSwarmPlan {
        tasks,
        synthesis_prompt: plan.synthesis_prompt,
        integrator_agent_id: integrator,
        warnings,
    })
}

fn fallback_tasks(
    template: SwarmTemplate,
    _root_prompt: &str,
    available_agents: &[String],
    plan_error: Option<&str>,
) -> ParsedSwarmPlan {
    if matches!(template, SwarmTemplate::Lab) {
        let integrator = available_agents.first().cloned();
        let recon_agent = available_agents
            .get(1)
            .or_else(|| available_agents.first())
            .cloned();
        let design_agent = available_agents
            .get(2)
            .or_else(|| available_agents.get(1))
            .or_else(|| available_agents.first())
            .cloned();
        let review_agent = available_agents
            .get(3)
            .or_else(|| available_agents.get(1))
            .or_else(|| available_agents.first())
            .cloned();

        let mut tasks = Vec::new();
        if let Some(agent_id) = recon_agent {
            tasks.push(SwarmTask {
                id: "recon".into(),
                agent_id,
                role: Some("research".into()),
                title: "Codebase recon".into(),
                task_prompt: "Scan the repository for the most relevant files/modules and summarize where changes should happen. Provide concrete file paths and key functions/symbols. Avoid proposing large diffs; focus on mapping the terrain and risks.".into(),
                deps: Vec::new(),
                writes: false,
                artifacts: vec!["files".into(), "risks".into()],
                done_when: Some("We know exactly where changes should happen and the main risks.".into()),
                state: SwarmTaskState::Pending,
                output: None,
                failed: false,
            });
        }
        if let Some(agent_id) = design_agent {
            tasks.push(SwarmTask {
                id: "design".into(),
                agent_id,
                role: Some("research".into()),
                title: "Design options".into(),
                task_prompt: "Propose 2-3 plausible implementation approaches (with tradeoffs) and call out which files/modules each approach touches. Keep it specific and repo-grounded.".into(),
                deps: Vec::new(),
                writes: false,
                artifacts: vec!["options".into(), "files".into()],
                done_when: Some("We have 1-2 clear, repo-grounded approaches with tradeoffs.".into()),
                state: SwarmTaskState::Pending,
                output: None,
                failed: false,
            });
        }
        if let Some(agent_id) = integrator.clone() {
            tasks.push(SwarmTask {
                id: "implement".into(),
                agent_id,
                role: Some("integrate".into()),
                title: "Integrate + implement".into(),
                task_prompt: "Implement the best approach using the dependency outputs. You are the only agent allowed to make workspace edits in this swarm. Prefer small, safe diffs and keep tests green.".into(),
                deps: vec!["recon".into(), "design".into()],
                writes: true,
                artifacts: vec!["diffs".into(), "commands".into()],
                done_when: Some("Changes are implemented cleanly with validations to run.".into()),
                state: SwarmTaskState::Pending,
                output: None,
                failed: false,
            });
        }
        if let Some(agent_id) = review_agent {
            tasks.push(SwarmTask {
                id: "review".into(),
                agent_id,
                role: Some("review".into()),
                title: "Review & verification".into(),
                task_prompt: "Review the implemented approach for correctness, UX, and maintainability. Suggest verification steps (exact commands) and edge cases. If you propose edits, do so as text/diff; do not apply changes.".into(),
                deps: vec!["implement".into()],
                writes: false,
                artifacts: vec!["risks".into(), "commands".into()],
                done_when: Some("We have confidence in correctness and a clear test plan.".into()),
                state: SwarmTaskState::Pending,
                output: None,
                failed: false,
            });
        }

        let synth = plan_error.map(|err| {
            format!("Note: planning failed; fallback prompts were used. Planner error was: {err}")
        });

        return ParsedSwarmPlan {
            tasks,
            synthesis_prompt: synth,
            integrator_agent_id: integrator,
            warnings: Vec::new(),
        };
    }

    let mut tasks = Vec::new();
    let mut idx = 0usize;
    for (agent_idx, agent_id) in available_agents.iter().enumerate() {
        idx = idx.saturating_add(1);
        let (role, title, prompt, deps, writes) = match (template, agent_idx) {
            (SwarmTemplate::Lab, 0) => (
                Some("research".to_string()),
                "Codebase recon",
                "Scan the repository for the most relevant files/modules and summarize where changes should happen. Provide concrete file paths and key functions/symbols. Avoid proposing large diffs; focus on mapping the terrain and risks.",
                Vec::new(),
                false,
            ),
            (SwarmTemplate::Lab, 1) => (
                Some("research".to_string()),
                "Design options",
                "Propose 2-3 plausible implementation approaches (with tradeoffs) and call out which files/modules each approach touches. Keep it specific and repo-grounded.",
                Vec::new(),
                false,
            ),
            (SwarmTemplate::Lab, 2) => (
                Some("integrate".to_string()),
                "Integrate + implement",
                "Implement the best approach using the dependency outputs. You are the only agent allowed to make workspace edits in this swarm. Prefer small, safe diffs and keep tests green.",
                vec!["task-01".into(), "task-02".into()],
                true,
            ),
            (SwarmTemplate::Lab, _) => (
                Some("review".to_string()),
                "Review & verification",
                "Review the proposed approach for correctness, UX, and maintainability. Suggest verification steps (exact commands) and edge cases. If you propose edits, do so as text/diff; do not apply changes.",
                vec!["task-03".into()],
                false,
            ),
            (_, 0) => (
                Some("recon".to_string()),
                "Codebase recon",
                "Scan the repository for the most relevant files/modules and summarize where changes should happen. Provide concrete file paths and key functions/symbols. Avoid proposing large diffs; focus on mapping the terrain and risks.",
                Vec::new(),
                false,
            ),
            (_, 1) => (
                Some("plan".to_string()),
                "Implementation plan",
                "Propose an implementation approach and the specific code changes needed. If appropriate, provide a concise unified diff for the most important edits. Call out any concurrency/file-conflict risks with multiple agents.",
                Vec::new(),
                false,
            ),
            (_, 2) => (
                Some("test".to_string()),
                "Tests & verification",
                "Propose how to verify the change (tests, manual checks, edge cases). If tests likely exist, suggest exact commands and where to add/update test coverage.",
                Vec::new(),
                false,
            ),
            (_, _) => (
                Some("review".to_string()),
                "Review & pitfalls",
                "Review the planned approach for correctness, UX, and maintainability. Point out edge cases, failure modes, and simpler alternatives.",
                Vec::new(),
                false,
            ),
        };

        let task_id = format!("task-{idx:02}");
        tasks.push(SwarmTask {
            id: task_id,
            agent_id: agent_id.clone(),
            role,
            title: title.into(),
            task_prompt: prompt.into(),
            deps,
            writes,
            artifacts: Vec::new(),
            done_when: None,
            state: SwarmTaskState::Pending,
            output: None,
            failed: false,
        });
    }

    let synth = plan_error.map(|err| {
        format!("Note: planning failed; fallback prompts were used. Planner error was: {err}")
    });

    ParsedSwarmPlan {
        tasks,
        synthesis_prompt: synth,
        integrator_agent_id: None,
        warnings: Vec::new(),
    }
}

fn initialize_task_graph(run: &mut SwarmRun) {
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

fn tasks_terminal_count(tasks: &[SwarmTask]) -> usize {
    tasks.iter().filter(|task| task.state.is_terminal()).count()
}

fn mark_task_running(run: &mut SwarmRun, agent_id: &str) {
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

fn mark_task_finished(run: &mut SwarmRun, agent_id: &str, message: String, failed: bool) -> bool {
    let pos_running = run.tasks.iter().position(|task| {
        task.agent_id == agent_id && matches!(task.state, SwarmTaskState::Running)
    });
    let pos = pos_running.or_else(|| {
        run.tasks.iter().position(|task| {
            task.agent_id == agent_id && matches!(task.state, SwarmTaskState::Dispatched)
        })
    });
    let Some(pos) = pos else {
        return false;
    };
    let task = &mut run.tasks[pos];
    task.output = Some(message);
    task.failed = failed;
    task.state = if failed {
        SwarmTaskState::Failed
    } else {
        SwarmTaskState::Done
    };
    true
}

fn refresh_task_readiness(run: &mut SwarmRun) {
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

fn maybe_resolve_deadlock(run: &mut SwarmRun) -> Vec<String> {
    let has_active_or_ready = run.tasks.iter().any(|task| {
        matches!(
            task.state,
            SwarmTaskState::Ready | SwarmTaskState::Dispatched | SwarmTaskState::Running
        )
    });
    if has_active_or_ready {
        return Vec::new();
    }

    let pending = run
        .tasks
        .iter()
        .filter(|task| matches!(task.state, SwarmTaskState::Pending))
        .map(|task| task.id.clone())
        .collect::<Vec<_>>();
    if pending.is_empty() {
        return Vec::new();
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
    pending
}

fn dispatch_ready_tasks(run: &mut SwarmRun) -> Vec<SwarmDispatch> {
    let indices = select_dispatchable_ready_task_indices(run);
    let mut dispatches = Vec::new();
    for idx in indices {
        let task = &run.tasks[idx];
        let deps_payload = collect_dependency_payload(run, task);
        let prompt = if deps_payload.is_empty() {
            wrap_task_prompt(&run.root_prompt, task, None)
        } else {
            wrap_task_prompt(&run.root_prompt, task, Some(deps_payload.as_slice()))
        };
        let agent_id = task.agent_id.clone();
        run.tasks[idx].state = SwarmTaskState::Dispatched;
        dispatches.push(SwarmDispatch {
            agent_id,
            mission_id: run.mission_id.clone(),
            prompt,
        });
    }
    dispatches
}

fn select_dispatchable_ready_task_indices(run: &SwarmRun) -> Vec<usize> {
    let mut writer_taken = run.tasks.iter().any(|task| {
        task.writes
            && matches!(
                task.state,
                SwarmTaskState::Dispatched | SwarmTaskState::Running
            )
    });
    let mut indices = Vec::new();
    for (idx, task) in run.tasks.iter().enumerate() {
        if !matches!(task.state, SwarmTaskState::Ready) {
            continue;
        }
        if task.writes {
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
        let text = dep.output.as_deref().unwrap_or("(no output)");
        out.push((label, truncate_chars(text, SWARM_DEP_OUTPUT_MAX_CHARS)));
    }
    out
}

fn build_planner_prompt(
    root_prompt: &str,
    template: SwarmTemplate,
    planner_agent_id: &str,
    agent_ids: &[String],
    integrator_agent_id: Option<&str>,
) -> String {
    let available = agent_ids
        .iter()
        .filter(|id| id.as_str() != planner_agent_id)
        .cloned()
        .collect::<Vec<_>>();
    let mut out = String::new();
    out.push_str(
        "You are the SWARM PLANNER inside nit. Create an execution plan for a multi-agent workflow.\n\n",
    );
    out.push_str(&format!("Template: `{}`\n\n", template.label()));
    if matches!(template, SwarmTemplate::Lab) {
        if let Some(integrator_agent_id) = integrator_agent_id {
            out.push_str(&format!(
                "Single-writer integrator: `{integrator_agent_id}` (only this agent may do workspace writes).\n\n"
            ));
        } else {
            out.push_str("Single-writer integrator: (none)\n\n");
        }
    }

    out.push_str("Constraints:\n");
    out.push_str("- Only assign tasks to these agent ids:\n");
    for id in available.iter() {
        out.push_str(&format!("  - {id}\n"));
    }
    match template {
        SwarmTemplate::Parallel => {
            out.push_str(
                "- Prefer ONE task per agent id (max parallelism, deterministic tracking).\n",
            );
            out.push_str(
                "- Prefer tasks that can run in parallel (deps should usually be empty).\n",
            );
            out.push_str(
                "- If code changes are needed, avoid having multiple agents edit the same files.\n",
            );
        }
        SwarmTemplate::Lab => {
            out.push_str(
                "- You MAY assign multiple tasks to the same agent id (they run sequentially).\n",
            );
            out.push_str("- Use deps to express ordering (DAG). Avoid cycles.\n");
            out.push_str("- Only the integrator agent may have `writes=true` tasks.\n");
            out.push_str("- Use read-only researcher/reviewer tasks to feed the integrator.\n");
        }
    }
    out.push_str("\nOutput format:\n");
    out.push_str("1) 3-6 bullets summarizing the plan.\n");
    out.push_str("2) A JSON plan in a ```json code block with this schema (v2):\n");
    out.push_str("{\n");
    out.push_str("  \"version\": 2,\n");
    out.push_str(&format!("  \"template\": \"{}\",\n", template.label()));
    out.push_str("  \"integrator_agent_id\": \"(optional)\",\n");
    out.push_str("  \"tasks\": [\n");
    out.push_str("    {\n");
    out.push_str("      \"id\": \"task-id\",\n");
    out.push_str("      \"agent_id\": \"one-of-the-listed-agent-ids\",\n");
    out.push_str("      \"role\": \"(optional: research|integrate|review|test)\",\n");
    out.push_str("      \"title\": \"short title\",\n");
    out.push_str("      \"prompt\": \"task instructions\",\n");
    out.push_str("      \"deps\": [\"task-id\"],\n");
    out.push_str("      \"writes\": false,\n");
    out.push_str(
        "      \"artifacts\": [\"(optional keys: files, diffs, commands, risks, notes)\"],\n",
    );
    out.push_str("      \"done_when\": \"(optional completion contract)\"\n");
    out.push_str("    }\n");
    out.push_str("  ],\n");
    out.push_str(
        "  \"synthesis_prompt\": \"(optional extra guidance for the final synthesis step)\"\n",
    );
    out.push_str("}\n");
    out.push_str("\nOperator request:\n");
    out.push_str(root_prompt.trim());
    out.push('\n');
    out
}

fn wrap_task_prompt(
    root_prompt: &str,
    task: &SwarmTask,
    deps: Option<&[(String, String)]>,
) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "SWARM TASK: {} ({})\n",
        task.title.trim(),
        task.id
    ));
    if let Some(role) = task.role.as_deref() {
        if !role.trim().is_empty() {
            out.push_str(&format!("ROLE: {}\n", role.trim()));
        }
    }
    if task.writes {
        out.push_str("MODE: single-writer integrator (workspace writes allowed)\n");
    } else {
        out.push_str("MODE: read-only (do not edit the workspace)\n");
    }
    if let Some(done_when) = task.done_when.as_deref() {
        if !done_when.trim().is_empty() {
            out.push_str(&format!("DONE WHEN: {}\n", done_when.trim()));
        }
    }
    if !task.artifacts.is_empty() {
        out.push_str("ARTIFACTS:\n");
        for item in task.artifacts.iter() {
            let item = item.trim();
            if item.is_empty() {
                continue;
            }
            out.push_str(&format!("- {item}\n"));
        }
    }

    out.push_str("\nOperator request:\n");
    out.push_str(root_prompt.trim());
    out.push_str("\n\nYour task:\n");
    out.push_str(task.task_prompt.trim());
    out.push('\n');

    if let Some(deps) = deps {
        if !deps.is_empty() {
            out.push_str("\nDependency outputs:\n");
            for (label, output) in deps.iter() {
                out.push_str(&format!("\n---\nDEP: {label}\n"));
                out.push_str(output.trim());
                out.push('\n');
            }
        }
    }

    out.push_str("\nRespond with:\n- Findings / recommendations\n- Concrete file paths / commands where relevant\n");
    out
}

fn build_synthesis_prompt(run: &SwarmRun) -> String {
    let mut out = String::new();
    out.push_str(
        "You are the SWARM SYNTHESIZER. Produce a single cohesive response for the operator by combining the parallel agent outputs below.\n\n",
    );
    out.push_str("Operator request:\n");
    out.push_str(run.root_prompt.trim());
    out.push_str("\n\nAgent outputs:\n");
    for task in run.tasks.iter() {
        out.push_str(&format!(
            "\n---\nAGENT: {}\nTASK: {} ({})\n",
            task.agent_id,
            task.title.trim(),
            task.id
        ));
        if let Some(role) = task.role.as_deref() {
            if !role.trim().is_empty() {
                out.push_str(&format!("ROLE: {}\n", role.trim()));
            }
        }
        if !task.deps.is_empty() {
            out.push_str(&format!("DEPS: {}\n", task.deps.join(", ")));
        }
        let status = match task.state {
            SwarmTaskState::Done => "DONE",
            SwarmTaskState::Failed => "FAILED",
            SwarmTaskState::Skipped => "SKIPPED",
            SwarmTaskState::Pending => "PENDING",
            SwarmTaskState::Ready => "READY",
            SwarmTaskState::Dispatched => "QUEUED",
            SwarmTaskState::Running => "RUNNING",
        };
        out.push_str(&format!("STATUS: {status}\n"));
        if let Some(output) = task.output.as_deref() {
            out.push_str(output.trim());
            out.push('\n');
        } else {
            out.push_str("(no output)\n");
        }
    }
    if let Some(bundle) = run.gate_bundle.as_ref() {
        out.push_str("\n\nVerification gates:\n");
        out.push_str(&format!("Bundle: {}\n", bundle.label()));
        if let Some(report) = run.gate_report.as_ref() {
            out.push_str("Structured report:\n```json\n");
            if let Ok(json) = serde_json::to_string_pretty(report) {
                out.push_str(&json);
            } else {
                out.push_str("{\"overall_ok\":false}");
            }
            out.push_str("\n```\n");
        } else {
            out.push_str("Structured report: (missing)\n");
        }
        if let Some(output) = run.gate_output.as_deref() {
            out.push_str("\nVerifier raw output (truncated):\n");
            out.push_str(&truncate_chars(output, SWARM_VERIFY_MAX_CHARS));
            out.push('\n');
        }
    }
    if let Some(extra) = run.synthesis_prompt.as_deref() {
        out.push_str("\n\nSynthesis notes:\n");
        out.push_str(extra.trim());
        out.push('\n');
    }
    out.push_str(
        "\nResponse requirements:\n- Be decisive: choose a best approach.\n- Include specific next steps.\n- If code changes are needed, outline exact edits and validation steps.\n",
    );
    out
}

fn extract_json_code_block(text: &str) -> Option<String> {
    let mut lines = text.lines();
    while let Some(line) = lines.next() {
        let trimmed = line.trim_start();
        let is_json_fence = trimmed.starts_with("```json") || trimmed.starts_with("```JSON");
        if !is_json_fence {
            continue;
        }
        let mut buf = String::new();
        for inner in &mut lines {
            if inner.trim() == "```" {
                break;
            }
            buf.push_str(inner);
            buf.push('\n');
        }
        let candidate = buf.trim().to_string();
        if !candidate.is_empty() {
            return Some(candidate);
        }
    }

    // Fallback: attempt to parse the first JSON object we can find.
    let trimmed = text.trim();
    let start = trimmed.find('{')?;
    let end = trimmed.rfind('}')?;
    if start >= end {
        return None;
    }
    let candidate = trimmed[start..=end].trim().to_string();
    (!candidate.is_empty()).then_some(candidate)
}

fn build_verify_prompt(run: &SwarmRun, bundle: &GateBundle) -> String {
    let mut out = String::new();
    out.push_str(
        "You are the SWARM VERIFIER. Run the verification gate bundle below against the current workspace.\n\n",
    );
    out.push_str("Rules:\n");
    out.push_str("- Run commands in order.\n");
    out.push_str(
        "- If a gate fails, keep going when feasible (collect as much signal as possible).\n",
    );
    out.push_str("- Keep logs concise: include only the key error snippets needed to debug.\n");
    out.push_str("- At the end, output a single JSON report in a ```json code block.\n");
    out.push_str("\nOperator request (context):\n");
    out.push_str(run.root_prompt.trim());
    out.push_str("\n\nGate bundle:\n");
    out.push_str(&format!("Bundle: {}\n", bundle.label()));
    for gate in bundle.gates() {
        out.push_str(&format!("- {}: `{}`\n", gate.name, gate.command));
    }

    out.push_str("\nReport schema:\n");
    out.push_str("{\"overall_ok\":true,\"gates\":[{\"name\":\"fmt\",\"command\":\"...\",\"ok\":true,\"notes\":\"(optional)\"}]}\n");
    out.push_str(
        "\nImportant: The JSON must reflect the actual command outcomes (ok=true only when the command succeeded).\n",
    );
    out
}

fn parse_gate_report(message: &str) -> Option<GateReport> {
    let json = extract_json_code_block(message)?;
    serde_json::from_str::<GateReport>(&json).ok()
}

fn truncate_chars(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let clipped: String = text.chars().take(max_chars).collect();
    format!("{clipped}\n... (truncated)")
}

fn next_mission_id(state: &AppState) -> String {
    format!("mis-{:03}", state.agents.missions.len() + 1)
}

fn swarm_mission_title(root_prompt: &str, mission_id: &str) -> String {
    let first = root_prompt.lines().next().unwrap_or("Swarm mission").trim();
    if first.is_empty() {
        return format!("{mission_id} swarm mission");
    }
    let mut title = String::new();
    for ch in first.chars().take(48) {
        title.push(ch);
    }
    format!("Swarm: {title}")
}

fn timestamp_label(state: &AppState) -> String {
    format!("t+{}", state.metrics.frame_count)
}

fn update_mission_phase(state: &mut AppState, mission_id: &str, phase: MissionPhase) {
    let at = timestamp_label(state);
    if let Some(mission) = state
        .agents
        .missions
        .iter_mut()
        .find(|m| m.id == mission_id)
    {
        mission.phase = phase;
        mission.updated_at = at;
    }
}

fn update_mission_final(state: &mut AppState, mission_id: &str, status: &str) {
    let at = timestamp_label(state);
    if let Some(mission) = state
        .agents
        .missions
        .iter_mut()
        .find(|m| m.id == mission_id)
    {
        mission.status = status.into();
        mission.phase = MissionPhase::Report;
        mission.updated_at = at;
    }
}

fn update_mission_status(state: &mut AppState, run: &SwarmRun, done_override: Option<usize>) {
    let at = timestamp_label(state);
    let Some(mission) = state
        .agents
        .missions
        .iter_mut()
        .find(|mission| mission.id == run.mission_id)
    else {
        return;
    };

    let done = done_override.unwrap_or_else(|| tasks_terminal_count(&run.tasks));
    let total = run.tasks.len().max(1);
    let status = match run.stage {
        SwarmStage::Planning => "PLAN".into(),
        SwarmStage::Executing => format!("EXEC {done}/{total}"),
        SwarmStage::Verifying => "VERIFY".into(),
        SwarmStage::Synthesizing => "SYNTH".into(),
    };
    mission.status = status;
    mission.updated_at = at;
}

pub fn select_swarm_agents(state: &AppState, planner: &str, size: SwarmSize) -> Vec<String> {
    let mut codex = state
        .agents
        .agents
        .iter()
        .filter(|lane| lane.is_codex())
        .map(|lane| lane.id.clone())
        .collect::<Vec<_>>();
    if codex.is_empty() {
        return Vec::new();
    }
    codex.retain(|id| id != planner);
    let mut agents = vec![planner.to_string()];
    let target = match size {
        SwarmSize::Default => DEFAULT_SWARM_SIZE,
        SwarmSize::All => usize::MAX,
        SwarmSize::Count(n) => n,
    }
    .clamp(1, MAX_SWARM_SIZE);
    let take = target.saturating_sub(1);
    agents.extend(codex.into_iter().take(take));
    agents
}

pub fn is_agent_busy(state: &AppState, agent_id: &str) -> bool {
    state.agents.active_turns.contains_key(agent_id)
        || state
            .agents
            .queued_codex_turns
            .iter()
            .any(|turn| turn.agent_id == agent_id)
        || state
            .agents
            .agents
            .iter()
            .find(|lane| lane.id.as_str() == agent_id)
            .is_some_and(|lane| matches!(lane.status, AgentStatus::Running))
}

pub fn push_system_message_to_mission(state: &mut AppState, mission_id: &str, text: String) {
    state.agents.messages.push(AgentMessage {
        at: timestamp_label(state),
        channel: nit_core::AgentChannel::Broadcast,
        agent_id: Some("swarm".into()),
        mission_id: Some(mission_id.to_string()),
        text,
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_swarm_requires_whitespace_after_prefix() {
        assert!(parse_swarm_command("@swarmies hello").is_none());
        assert!(parse_swarm_command("@swarm").is_none());
        assert!(parse_swarm_command("@swarm   ").is_none());
    }

    #[test]
    fn parse_swarm_default_size() {
        let cmd = parse_swarm_command("@swarm build x").expect("cmd");
        assert_eq!(cmd.size, SwarmSize::Default);
        assert_eq!(cmd.template, None);
        assert_eq!(cmd.prompt, "build x");
    }

    #[test]
    fn parse_swarm_all() {
        let cmd = parse_swarm_command("@swarm all do thing").expect("cmd");
        assert_eq!(cmd.size, SwarmSize::All);
        assert_eq!(cmd.template, None);
        assert_eq!(cmd.prompt, "do thing");
    }

    #[test]
    fn parse_swarm_count() {
        let cmd = parse_swarm_command("@swarm 6 do thing").expect("cmd");
        assert_eq!(cmd.size, SwarmSize::Count(6));
        assert_eq!(cmd.template, None);
        assert_eq!(cmd.prompt, "do thing");
    }

    #[test]
    fn parse_swarm_template() {
        let cmd = parse_swarm_command("@swarm template=lab do thing").expect("cmd");
        assert_eq!(cmd.size, SwarmSize::Default);
        assert_eq!(cmd.template.as_deref(), Some("lab"));
        assert_eq!(cmd.prompt, "do thing");

        let cmd = parse_swarm_command("@swarm 5 t=parallel do thing").expect("cmd");
        assert_eq!(cmd.size, SwarmSize::Count(5));
        assert_eq!(cmd.template.as_deref(), Some("parallel"));
        assert_eq!(cmd.prompt, "do thing");
    }

    #[test]
    fn plan_v2_enforces_single_writer_integrator() {
        let planner_message = r#"
Plan:
- do stuff

```json
{
  "version": 2,
  "template": "lab",
  "integrator_agent_id": "a1",
  "tasks": [
    { "id": "t1", "agent_id": "a2", "title": "Bad writer", "prompt": "x", "writes": true, "deps": [] },
    { "id": "t2", "agent_id": "a1", "title": "Good writer", "prompt": "y", "writes": true, "deps": [] }
  ]
}
```
"#;
        let available = vec!["a1".to_string(), "a2".to_string()];
        let parsed = parse_plan_from_planner(
            planner_message,
            SwarmTemplate::Lab,
            "root",
            &available,
            Some("a1"),
        );
        assert_eq!(parsed.integrator_agent_id.as_deref(), Some("a1"));
        assert!(parsed
            .warnings
            .iter()
            .any(|w| w.contains("forcing read-only")));

        let t1 = parsed.tasks.iter().find(|t| t.id == "t1").expect("t1");
        let t2 = parsed.tasks.iter().find(|t| t.id == "t2").expect("t2");
        assert!(!t1.writes);
        assert!(t2.writes);
    }

    #[test]
    fn dag_scheduler_dispatches_after_deps() {
        let mut run = SwarmRun {
            mission_id: "mis-001".into(),
            root_prompt: "root".into(),
            template: SwarmTemplate::Lab,
            planner_agent_id: "planner".into(),
            integrator_agent_id: Some("a1".into()),
            verifier_agent_id: None,
            gate_bundle: None,
            agent_ids: vec![
                "planner".into(),
                "a1".into(),
                "a2".into(),
                "a3".into(),
                "a4".into(),
            ],
            stage: SwarmStage::Executing,
            tasks: vec![
                SwarmTask {
                    id: "recon".into(),
                    agent_id: "a2".into(),
                    role: Some("research".into()),
                    title: "Recon".into(),
                    task_prompt: "recon".into(),
                    deps: Vec::new(),
                    writes: false,
                    artifacts: Vec::new(),
                    done_when: None,
                    state: SwarmTaskState::Pending,
                    output: None,
                    failed: false,
                },
                SwarmTask {
                    id: "design".into(),
                    agent_id: "a3".into(),
                    role: Some("research".into()),
                    title: "Design".into(),
                    task_prompt: "design".into(),
                    deps: Vec::new(),
                    writes: false,
                    artifacts: Vec::new(),
                    done_when: None,
                    state: SwarmTaskState::Pending,
                    output: None,
                    failed: false,
                },
                SwarmTask {
                    id: "implement".into(),
                    agent_id: "a1".into(),
                    role: Some("integrate".into()),
                    title: "Implement".into(),
                    task_prompt: "impl".into(),
                    deps: vec!["recon".into(), "design".into()],
                    writes: true,
                    artifacts: Vec::new(),
                    done_when: None,
                    state: SwarmTaskState::Pending,
                    output: None,
                    failed: false,
                },
                SwarmTask {
                    id: "review".into(),
                    agent_id: "a4".into(),
                    role: Some("review".into()),
                    title: "Review".into(),
                    task_prompt: "review".into(),
                    deps: vec!["implement".into()],
                    writes: false,
                    artifacts: Vec::new(),
                    done_when: None,
                    state: SwarmTaskState::Pending,
                    output: None,
                    failed: false,
                },
            ],
            synthesis_prompt: None,
            gate_output: None,
            gate_report: None,
        };

        initialize_task_graph(&mut run);
        refresh_task_readiness(&mut run);

        let first = dispatch_ready_tasks(&mut run);
        assert_eq!(first.len(), 2);
        assert!(first.iter().any(|d| d.agent_id == "a2"));
        assert!(first.iter().any(|d| d.agent_id == "a3"));

        assert!(mark_task_finished(
            &mut run,
            "a2",
            "recon out".into(),
            false
        ));
        assert!(mark_task_finished(
            &mut run,
            "a3",
            "design out".into(),
            false
        ));
        refresh_task_readiness(&mut run);

        let second = dispatch_ready_tasks(&mut run);
        assert_eq!(second.len(), 1);
        assert_eq!(second[0].agent_id, "a1");

        assert!(mark_task_finished(&mut run, "a1", "impl out".into(), false));
        refresh_task_readiness(&mut run);
        let third = dispatch_ready_tasks(&mut run);
        assert_eq!(third.len(), 1);
        assert_eq!(third[0].agent_id, "a4");
    }

    #[test]
    fn single_writer_limits_concurrent_write_tasks() {
        let mut run = SwarmRun {
            mission_id: "mis-001".into(),
            root_prompt: "root".into(),
            template: SwarmTemplate::Lab,
            planner_agent_id: "planner".into(),
            integrator_agent_id: Some("a1".into()),
            verifier_agent_id: None,
            gate_bundle: None,
            agent_ids: vec!["planner".into(), "a1".into(), "a2".into()],
            stage: SwarmStage::Executing,
            tasks: vec![
                SwarmTask {
                    id: "w1".into(),
                    agent_id: "a1".into(),
                    role: Some("integrate".into()),
                    title: "Write 1".into(),
                    task_prompt: "w1".into(),
                    deps: Vec::new(),
                    writes: true,
                    artifacts: Vec::new(),
                    done_when: None,
                    state: SwarmTaskState::Pending,
                    output: None,
                    failed: false,
                },
                SwarmTask {
                    id: "w2".into(),
                    agent_id: "a1".into(),
                    role: Some("integrate".into()),
                    title: "Write 2".into(),
                    task_prompt: "w2".into(),
                    deps: Vec::new(),
                    writes: true,
                    artifacts: Vec::new(),
                    done_when: None,
                    state: SwarmTaskState::Pending,
                    output: None,
                    failed: false,
                },
                SwarmTask {
                    id: "r1".into(),
                    agent_id: "a2".into(),
                    role: Some("research".into()),
                    title: "Read".into(),
                    task_prompt: "r1".into(),
                    deps: Vec::new(),
                    writes: false,
                    artifacts: Vec::new(),
                    done_when: None,
                    state: SwarmTaskState::Pending,
                    output: None,
                    failed: false,
                },
            ],
            synthesis_prompt: None,
            gate_output: None,
            gate_report: None,
        };

        initialize_task_graph(&mut run);
        refresh_task_readiness(&mut run);

        let first = dispatch_ready_tasks(&mut run);
        // Should dispatch w1 and r1, but not w2 (single-writer lock).
        assert_eq!(first.len(), 2);
        assert!(first.iter().any(|d| d.prompt.contains("Write 1 (w1)")));
        assert!(first.iter().any(|d| d.prompt.contains("Read (r1)")));
        assert!(!first.iter().any(|d| d.prompt.contains("Write 2 (w2)")));

        assert!(mark_task_finished(&mut run, "a1", "w1 out".into(), false));
        refresh_task_readiness(&mut run);
        let second = dispatch_ready_tasks(&mut run);
        assert_eq!(second.len(), 1);
        assert!(second[0].prompt.contains("Write 2 (w2)"));
    }

    #[test]
    fn deadlock_detection_skips_pending_tasks() {
        let mut run = SwarmRun {
            mission_id: "mis-001".into(),
            root_prompt: "root".into(),
            template: SwarmTemplate::Lab,
            planner_agent_id: "planner".into(),
            integrator_agent_id: Some("a1".into()),
            verifier_agent_id: None,
            gate_bundle: None,
            agent_ids: vec!["planner".into(), "a1".into()],
            stage: SwarmStage::Executing,
            tasks: vec![
                SwarmTask {
                    id: "t1".into(),
                    agent_id: "a1".into(),
                    role: None,
                    title: "T1".into(),
                    task_prompt: "t1".into(),
                    deps: vec!["t2".into()],
                    writes: false,
                    artifacts: Vec::new(),
                    done_when: None,
                    state: SwarmTaskState::Pending,
                    output: None,
                    failed: false,
                },
                SwarmTask {
                    id: "t2".into(),
                    agent_id: "a1".into(),
                    role: None,
                    title: "T2".into(),
                    task_prompt: "t2".into(),
                    deps: vec!["t1".into()],
                    writes: false,
                    artifacts: Vec::new(),
                    done_when: None,
                    state: SwarmTaskState::Pending,
                    output: None,
                    failed: false,
                },
            ],
            synthesis_prompt: None,
            gate_output: None,
            gate_report: None,
        };
        initialize_task_graph(&mut run);
        refresh_task_readiness(&mut run);
        assert!(dispatch_ready_tasks(&mut run).is_empty());

        let skipped = maybe_resolve_deadlock(&mut run);
        assert_eq!(skipped.len(), 2);
        assert!(run
            .tasks
            .iter()
            .all(|t| matches!(t.state, SwarmTaskState::Skipped)));
    }

    #[test]
    fn extracts_json_code_block() {
        let text = "hello\n```json\n{\"tasks\":[]}\n```\nbye";
        let json = extract_json_code_block(text).expect("json");
        assert_eq!(json.trim(), "{\"tasks\":[]}");
    }

    #[test]
    fn parse_gate_report_requires_json_block() {
        assert!(parse_gate_report("no json here").is_none());
    }

    #[test]
    fn parse_gate_report_parses_schema() {
        let text = "ok\n```json\n{\"overall_ok\":false,\"gates\":[{\"name\":\"fmt\",\"command\":\"cargo fmt\",\"ok\":false,\"notes\":\"bad\"}]}\n```\n";
        let report = parse_gate_report(text).expect("report");
        assert!(!report.overall_ok);
        assert_eq!(report.gates.len(), 1);
        assert_eq!(report.gates[0].name, "fmt");
        assert_eq!(report.gates[0].command, "cargo fmt");
        assert!(!report.gates[0].ok);
        assert_eq!(report.gates[0].notes.as_deref(), Some("bad"));
    }
}
