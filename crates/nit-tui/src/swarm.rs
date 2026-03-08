use std::{
    collections::{HashMap, HashSet},
    fs,
    path::Path,
};

use nit_core::{AgentBusEvent, AgentMessage, AgentStatus, AppState, MissionPhase, MissionRecord};

const DEFAULT_SWARM_SIZE: usize = 4;
const MAX_SWARM_SIZE: usize = 16;
const SWARM_VERIFY_MAX_CHARS: usize = 12_000;
const SWARM_DEP_OUTPUT_MAX_CHARS: usize = 8_000;
const COMPUTATIONAL_RESEARCH_ROLE: &str = "computational-research";
const COMPUTATIONAL_RESEARCH_ROLE_LEGACY: &str = "computational research";

fn swarm_clone_base_id(agent_id: &str) -> Option<&str> {
    agent_id.split_once("#swarm-").map(|(base_id, _)| base_id)
}

fn is_swarm_clone_agent_id(agent_id: &str) -> bool {
    swarm_clone_base_id(agent_id).is_some()
}

fn copy_codex_runtime_metadata(state: &mut AppState, base_id: &str, clone_id: &str) {
    if let Some(tokens) = state
        .agents
        .codex_effective_context_window_tokens
        .get(base_id)
        .copied()
    {
        state
            .agents
            .codex_effective_context_window_tokens
            .insert(clone_id.to_string(), tokens);
    }
    if let Some(effort) = state
        .agents
        .codex_default_reasoning_effort
        .get(base_id)
        .cloned()
    {
        state
            .agents
            .codex_default_reasoning_effort
            .insert(clone_id.to_string(), effort);
    }
    if let Some(efforts) = state
        .agents
        .codex_supported_reasoning_efforts
        .get(base_id)
        .cloned()
    {
        state
            .agents
            .codex_supported_reasoning_efforts
            .insert(clone_id.to_string(), efforts);
    }
    if let Some(effort) = state
        .agents
        .codex_selected_reasoning_effort
        .get(base_id)
        .cloned()
    {
        state
            .agents
            .codex_selected_reasoning_effort
            .insert(clone_id.to_string(), effort);
    }
}

fn insert_swarm_clone_lane(state: &mut AppState, base_id: &str, clone_lane: nit_core::AgentLane) {
    if state
        .agents
        .agents
        .iter()
        .any(|existing| existing.id == clone_lane.id)
    {
        return;
    }

    let Some(base_pos) = state
        .agents
        .agents
        .iter()
        .position(|lane| lane.id == base_id)
    else {
        state.agents.agents.push(clone_lane);
        return;
    };

    let mut insert_pos = base_pos.saturating_add(1);
    while insert_pos < state.agents.agents.len() {
        let lane = &state.agents.agents[insert_pos];
        if swarm_clone_base_id(lane.id.as_str()) == Some(base_id) {
            insert_pos = insert_pos.saturating_add(1);
        } else {
            break;
        }
    }
    state.agents.agents.insert(insert_pos, clone_lane);
}

fn ensure_size_clones(
    state: &mut AppState,
    mission_id: &str,
    template: SwarmTemplate,
    size: SwarmSize,
    planner_agent_id: &str,
    agents: &mut Vec<String>,
) {
    if !matches!(template, SwarmTemplate::Parallel | SwarmTemplate::Bulk) {
        return;
    }
    if matches!(size, SwarmSize::All) {
        return;
    }

    let target = match size {
        SwarmSize::Default => DEFAULT_SWARM_SIZE,
        SwarmSize::All => MAX_SWARM_SIZE,
        SwarmSize::Count(n) => n,
    }
    .clamp(1, MAX_SWARM_SIZE);
    if agents.len() >= target {
        return;
    }

    let mut sources: Vec<String> = Vec::new();
    if !state.agents.swarm_priority_agent_ids.is_empty() {
        for lane in state.agents.agents.iter() {
            if !lane.is_codex() {
                continue;
            }
            if lane.id.as_str() == planner_agent_id {
                continue;
            }
            if is_swarm_clone_agent_id(lane.id.as_str()) {
                continue;
            }
            if state.agents.swarm_priority_agent_ids.contains(&lane.id) {
                sources.push(lane.id.clone());
            }
        }
    }
    if sources.is_empty() {
        sources.push(planner_agent_id.to_string());
    }

    let mut source_lanes = Vec::new();
    for source_id in sources.iter() {
        let Some(base_lane) = state
            .agents
            .agents
            .iter()
            .find(|lane| lane.id == *source_id)
            .filter(|lane| lane.is_codex())
            .cloned()
        else {
            continue;
        };
        source_lanes.push((source_id.clone(), base_lane));
    }
    if source_lanes.is_empty() {
        return;
    }

    let mut clone_num: usize = 0;
    while agents.len() < target {
        clone_num = clone_num.saturating_add(1);
        let (source_id, base_lane) = &source_lanes[(clone_num - 1) % source_lanes.len()];
        let clone_id = format!("{source_id}#swarm-{mission_id}-clone-{clone_num:02}");

        if agents.iter().any(|id| id == &clone_id) {
            continue;
        }

        if state.agents.agents.iter().all(|lane| lane.id != clone_id) {
            let mut lane = base_lane.clone();
            lane.id = clone_id.clone();
            let base_role = base_lane.role.trim();
            let display_role = if base_role.is_empty() {
                format!("(clone {clone_num:02})")
            } else {
                format!("{base_role} (clone {clone_num:02})")
            };
            lane.role = display_role;
            lane.status = AgentStatus::Idle;
            lane.heartbeat_age_secs = 0;
            lane.queue_len = 0;
            lane.current_mission = None;
            lane.last_message = String::new();
            insert_swarm_clone_lane(state, source_id.as_str(), lane);
        }

        copy_codex_runtime_metadata(state, source_id.as_str(), clone_id.as_str());
        agents.push(clone_id);
    }
}

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
    /// "Bulk orchestration": propose many candidate solutions in parallel, then converge via a
    /// judge step feeding a single-writer integrator.
    Bulk,
}

fn parse_swarm_template(value: Option<&str>) -> SwarmTemplate {
    let Some(value) = value else {
        return SwarmTemplate::Lab;
    };
    let value = value.trim();
    if value.eq_ignore_ascii_case("parallel") || value.eq_ignore_ascii_case("v1") {
        return SwarmTemplate::Parallel;
    }
    if value.eq_ignore_ascii_case("bulk") || value.eq_ignore_ascii_case("bo") {
        return SwarmTemplate::Bulk;
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
            SwarmTemplate::Bulk => "bulk",
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
    completed_runs: HashMap<String, SwarmRun>,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum SwarmStage {
    Planning,
    Executing,
    Verifying,
    Synthesizing,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum GateBundle {
    Rust,
    Node,
    Python,
    Go,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Gate {
    name: &'static str,
    command: &'static str,
}

impl GateBundle {
    fn from_label(value: &str) -> Option<Self> {
        let value = value.trim();
        if value.eq_ignore_ascii_case("rust-ci") {
            return Some(Self::Rust);
        }
        if value.eq_ignore_ascii_case("node-ci") {
            return Some(Self::Node);
        }
        if value.eq_ignore_ascii_case("python-ci") {
            return Some(Self::Python);
        }
        if value.eq_ignore_ascii_case("go-ci") {
            return Some(Self::Go);
        }
        None
    }

    fn detect(state: &AppState) -> GateBundleSelection {
        let config_default = read_workspace_gate_default(state.workspace_root.as_path());
        if let Ok(Some(default)) = config_default.as_ref() {
            if default.eq_ignore_ascii_case("none") {
                return GateBundleSelection {
                    bundle: None,
                    source: "config:none".into(),
                };
            }
            if default.eq_ignore_ascii_case("auto") {
                // continue with auto-detection below
            } else if let Some(bundle) = Self::from_label(default) {
                return GateBundleSelection {
                    bundle: Some(bundle.clone()),
                    source: format!("config:{}", bundle.label()),
                };
            }
        }

        let mut detected = None;
        let mut cursor = Some(state.workspace_root.as_path());
        while let Some(path) = cursor {
            if path.join("Cargo.toml").exists() {
                detected = Some((Self::Rust, "Cargo.toml"));
                break;
            }
            if path.join("package.json").exists() {
                detected = Some((Self::Node, "package.json"));
                break;
            }
            if path.join("pyproject.toml").exists() {
                detected = Some((Self::Python, "pyproject.toml"));
                break;
            }
            if path.join("requirements.txt").exists() {
                detected = Some((Self::Python, "requirements.txt"));
                break;
            }
            if path.join("setup.cfg").exists() {
                detected = Some((Self::Python, "setup.cfg"));
                break;
            }
            if path.join("setup.py").exists() {
                detected = Some((Self::Python, "setup.py"));
                break;
            }
            if path.join("go.mod").exists() {
                detected = Some((Self::Go, "go.mod"));
                break;
            }
            cursor = path.parent();
        }

        let parse_error = config_default
            .err()
            .map(|err| format!("config-error:{err}"));
        if let Some((bundle, marker)) = detected {
            return GateBundleSelection {
                bundle: Some(bundle.clone()),
                source: parse_error
                    .map(|prefix| format!("{prefix}|auto:{}({marker})", bundle.label()))
                    .unwrap_or_else(|| format!("auto:{}({marker})", bundle.label())),
            };
        }

        GateBundleSelection {
            bundle: None,
            source: parse_error.unwrap_or_else(|| "auto:none".into()),
        }
    }

    fn label(&self) -> &'static str {
        match self {
            GateBundle::Rust => "rust-ci",
            GateBundle::Node => "node-ci",
            GateBundle::Python => "python-ci",
            GateBundle::Go => "go-ci",
        }
    }

    fn gates(&self) -> Vec<Gate> {
        match self {
            GateBundle::Rust => vec![
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
            GateBundle::Node => vec![
                Gate {
                    name: "lint",
                    command: "npm run lint --if-present",
                },
                Gate {
                    name: "build",
                    command: "npm run build --if-present",
                },
                Gate {
                    name: "test",
                    command: "npm test -- --watch=false --passWithNoTests",
                },
            ],
            GateBundle::Python => vec![
                Gate {
                    name: "ruff",
                    command: "python -m ruff check .",
                },
                Gate {
                    name: "mypy",
                    command: "python -m mypy .",
                },
                Gate {
                    name: "pytest",
                    command: "python -m pytest -q",
                },
            ],
            GateBundle::Go => vec![
                Gate {
                    name: "fmt",
                    command: "gofmt -l .",
                },
                Gate {
                    name: "vet",
                    command: "go vet ./...",
                },
                Gate {
                    name: "test",
                    command: "go test ./...",
                },
            ],
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct GateBundleSelection {
    bundle: Option<GateBundle>,
    source: String,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct GateReport {
    pub overall_ok: bool,
    pub gates: Vec<GateReportGate>,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct GateReportGate {
    pub name: String,
    pub command: String,
    pub ok: bool,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub notes: Option<String>,
}

impl GateReportGate {
    fn ui_status(&self) -> &'static str {
        if let Some(status) = self.status.as_deref() {
            if status.eq_ignore_ascii_case("pass")
                || status.eq_ignore_ascii_case("ok")
                || status.eq_ignore_ascii_case("success")
            {
                return "PASS";
            }
            if status.eq_ignore_ascii_case("skip") || status.eq_ignore_ascii_case("skipped") {
                return "SKIP";
            }
            if status.eq_ignore_ascii_case("fail") || status.eq_ignore_ascii_case("failed") {
                return "FAIL";
            }
        }
        if self.ok {
            "PASS"
        } else {
            "FAIL"
        }
    }
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

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum SwarmDagValidationMode {
    /// Reject plans with cycles/unknown deps (do not auto-repair).
    Strict,
    /// Attempt to make the graph runnable (drop unknown deps + break cycles) with warnings.
    Repair,
}

const DEFAULT_DAG_VALIDATION_MODE: SwarmDagValidationMode = SwarmDagValidationMode::Strict;

#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct SwarmTaskArtifacts {
    #[serde(default)]
    pub summary: Option<String>,
    #[serde(default)]
    pub files: Vec<SwarmArtifactFile>,
    #[serde(default)]
    pub diffs: Vec<SwarmArtifactDiff>,
    #[serde(default)]
    pub commands: Vec<SwarmArtifactCommand>,
    #[serde(default)]
    pub risks: Vec<SwarmArtifactRisk>,
    #[serde(default)]
    pub notes: Vec<String>,
}

impl SwarmTaskArtifacts {
    fn is_empty(&self) -> bool {
        self.summary
            .as_deref()
            .is_none_or(|summary| summary.trim().is_empty())
            && self.files.is_empty()
            && self.diffs.is_empty()
            && self.commands.is_empty()
            && self.risks.is_empty()
            && self.notes.is_empty()
    }
}

#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct SwarmArtifactFile {
    pub path: String,
    #[serde(default)]
    pub notes: Option<String>,
}

#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct SwarmArtifactDiff {
    #[serde(default)]
    pub path: Option<String>,
    pub summary: String,
}

#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct SwarmArtifactCommand {
    pub cmd: String,
    #[serde(default)]
    pub purpose: Option<String>,
}

#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct SwarmArtifactRisk {
    #[serde(default)]
    pub level: Option<String>,
    pub item: String,
    #[serde(default)]
    pub mitigation: Option<String>,
}

#[derive(Clone, Debug)]
pub struct SwarmTaskDashboardRow {
    pub id: String,
    pub title: String,
    pub role: Option<String>,
    pub agent_id: String,
    pub state: String,
    pub deps: Vec<String>,
    pub blocked_on: Vec<String>,
    pub writes: bool,
    pub done_when: Option<String>,
    pub output_present: bool,
}

#[derive(Clone, Debug)]
pub struct SwarmGateDashboardRow {
    pub name: String,
    pub command: String,
    pub status: String,
    pub notes: Option<String>,
}

#[derive(Clone, Debug)]
pub struct SwarmDashboardView {
    pub mission_id: String,
    pub template: String,
    pub phase: String,
    pub done: usize,
    pub failed: usize,
    pub skipped: usize,
    pub running: usize,
    pub queued: usize,
    pub pending: usize,
    pub tasks: Vec<SwarmTaskDashboardRow>,
    pub gate_bundle: Option<String>,
    pub gates: Vec<SwarmGateDashboardRow>,
}

#[derive(Clone, Debug, serde::Serialize)]
pub struct SwarmTaskPersistenceView {
    pub id: String,
    pub title: String,
    pub role: Option<String>,
    pub agent_id: String,
    pub state: String,
    pub deps: Vec<String>,
    pub blocked_on: Vec<String>,
    pub writes: bool,
    pub done_when: Option<String>,
    pub expected_artifacts: Vec<String>,
    pub expected_artifacts_missing: bool,
    pub output_present: bool,
    pub output: Option<String>,
    pub artifacts: Option<SwarmTaskArtifacts>,
}

#[derive(Clone, Debug, serde::Serialize)]
pub struct SwarmPersistenceView {
    pub mission_id: String,
    pub template: String,
    pub phase: String,
    pub gate_bundle: Option<String>,
    pub gate_selection: String,
    pub gate_report: Option<GateReport>,
    pub gate_output: Option<String>,
    pub tasks: Vec<SwarmTaskPersistenceView>,
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
    parsed_artifacts: Option<SwarmTaskArtifacts>,
    expected_artifacts_missing: bool,
    failed: bool,
}

#[derive(Clone, Debug)]
struct SwarmRun {
    mission_id: String,
    root_prompt: String,
    template: SwarmTemplate,
    planner_agent_id: String,
    integrator_agent_id: Option<String>,
    integrator_locked: bool,
    verifier_agent_id: Option<String>,
    gate_bundle: Option<GateBundle>,
    gate_selection: String,
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
            gate_bundle: run
                .gate_bundle
                .as_ref()
                .map(|bundle| bundle.label().to_string()),
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
            gate_bundle: run
                .gate_bundle
                .as_ref()
                .map(|bundle| bundle.label().to_string()),
            gate_selection: run.gate_selection.clone(),
            gate_report: run.gate_report.clone(),
            gate_output: run.gate_output.clone(),
            tasks,
        })
    }

    pub fn start(
        &mut self,
        state: &mut AppState,
        planner_agent_id: String,
        agent_ids: Vec<String>,
        size: SwarmSize,
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
        if !agents.iter().any(|id| id == &planner_agent_id) {
            agents.insert(0, planner_agent_id.clone());
        }

        let template_kind = parse_swarm_template(template.as_deref());
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

        let title = swarm_mission_title(&root_prompt, &mission_id, template_kind);
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

        let mut role_hints: Vec<(String, String)> = Vec::new();
        if matches!(template_kind, SwarmTemplate::Parallel | SwarmTemplate::Bulk) {
            for id in agents
                .iter()
                .filter(|id| id.as_str() != planner_agent_id.as_str())
            {
                let role = state
                    .agents
                    .swarm_role_by_agent_id
                    .get(id)
                    .map(|s| s.trim().to_ascii_lowercase())
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| "all".into());
                role_hints.push((id.clone(), role));
            }
        }

        let mut integrator_locked = false;
        let mut integrator_agent_id: Option<String> = None;
        if matches!(template_kind, SwarmTemplate::Lab | SwarmTemplate::Bulk) {
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

        let plan_prompt = build_planner_prompt(
            &root_prompt,
            template_kind,
            &planner_agent_id,
            &agents,
            integrator_agent_id.as_deref(),
            &role_hints,
            &priority_agent_ids,
        );

        let gate_selection = GateBundle::detect(state);
        let gate_bundle = gate_selection.bundle.clone();
        let verifier_agent_id = gate_bundle.as_ref().and_then(|_| {
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
                "Swarm template: {} | integrator: {} | verifier: {} | gates: {}",
                template_kind.label(),
                integrator_agent_id.as_deref().unwrap_or("(none)"),
                verifier_agent_id.as_deref().unwrap_or("(none)"),
                gate_bundle_label(gate_bundle.as_ref(), &gate_selection.source)
            ),
        );

        self.completed_runs.remove(&mission_id);
        self.runs.insert(
            mission_id.clone(),
            SwarmRun {
                mission_id: mission_id.clone(),
                root_prompt,
                template: template_kind,
                planner_agent_id: planner_agent_id.clone(),
                integrator_agent_id: integrator_agent_id.clone(),
                integrator_locked,
                verifier_agent_id,
                gate_bundle,
                gate_selection: gate_selection.source,
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
                        let mut parsed = parse_plan_from_planner(
                            message,
                            run.template,
                            &run.root_prompt,
                            &available,
                            run.integrator_agent_id.as_deref(),
                            run.integrator_locked,
                        );
                        parsed.warnings.extend(apply_role_dependency_ordering(
                            state.workspace_root.as_path(),
                            &state.agents.swarm_role_by_agent_id,
                            parsed.tasks.as_mut_slice(),
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
                            self.completed_runs.insert(mid.clone(), run);
                            return dispatches;
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
                    SwarmStage::Executing => {
                        if let Some(completed) =
                            mark_task_finished(&mut run, agent_id, message.clone(), false)
                        {
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
                        }
                        refresh_task_readiness(&mut run);
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
                        let mut parsed = fallback_tasks(
                            run.template,
                            &run.root_prompt,
                            &available,
                            Some(message),
                            run.integrator_agent_id.as_deref(),
                        );
                        parsed.warnings.extend(apply_role_dependency_ordering(
                            state.workspace_root.as_path(),
                            &state.agents.swarm_role_by_agent_id,
                            parsed.tasks.as_mut_slice(),
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
                            self.completed_runs.insert(mid.clone(), run);
                            return dispatches;
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
                    SwarmStage::Executing => {
                        if let Some(completed) =
                            mark_task_finished(&mut run, agent_id, message.clone(), true)
                        {
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
                        }
                        refresh_task_readiness(&mut run);
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
                        self.completed_runs.insert(mid.clone(), run);
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

    fn run_for_mission(&self, mission_id: &str) -> Option<&SwarmRun> {
        self.runs
            .get(mission_id)
            .or_else(|| self.completed_runs.get(mission_id))
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

fn task_role_is(task: &SwarmTask, role: &str) -> bool {
    task.role
        .as_deref()
        .is_some_and(|r| r.trim().eq_ignore_ascii_case(role))
}

fn bulk_is_proposer(task: &SwarmTask) -> bool {
    task_role_is(task, "propose") || task.id.to_ascii_lowercase().starts_with("propose-")
}

fn bulk_is_judge(task: &SwarmTask) -> bool {
    task_role_is(task, "judge") || task.id.eq_ignore_ascii_case("judge")
}

fn bulk_is_integrate(task: &SwarmTask) -> bool {
    task_role_is(task, "integrate") || task.id.eq_ignore_ascii_case("integrate")
}

fn normalize_bulk_plan(tasks: &mut [SwarmTask], integrator_agent_id: Option<&str>) -> Vec<String> {
    let mut warnings = Vec::new();
    let proposer_ids = tasks
        .iter()
        .filter(|task| bulk_is_proposer(task))
        .map(|task| task.id.clone())
        .collect::<Vec<_>>();
    let judge_idx = tasks.iter().position(bulk_is_judge);
    let integrate_idx = tasks.iter().position(bulk_is_integrate);

    for task in tasks.iter_mut() {
        if bulk_is_proposer(task) && task.writes {
            task.writes = false;
            warnings.push(format!(
                "Bulk plan: proposer task '{}' had writes=true; forcing read-only.",
                task.id
            ));
        }
    }

    if let Some(judge_idx) = judge_idx {
        let judge_id = tasks[judge_idx].id.clone();
        let mut changed = false;
        for proposer in proposer_ids.iter() {
            if proposer == &judge_id {
                continue;
            }
            if tasks[judge_idx].deps.iter().any(|dep| dep == proposer) {
                continue;
            }
            tasks[judge_idx].deps.push(proposer.clone());
            changed = true;
        }
        if changed {
            warnings.push(
                "Bulk plan: added missing deps so the judge depends on all proposer tasks.".into(),
            );
        }
    }

    if let (Some(integrate_idx), Some(judge_idx)) = (integrate_idx, judge_idx) {
        let judge_id = tasks[judge_idx].id.clone();
        if !tasks[integrate_idx].deps.iter().any(|dep| dep == &judge_id) {
            tasks[integrate_idx].deps.push(judge_id);
            warnings.push("Bulk plan: added missing dep so integrate depends on judge.".into());
        }
    }

    if let Some(integrate_idx) = integrate_idx {
        let allowed = integrator_agent_id
            .is_none_or(|integrator| tasks[integrate_idx].agent_id == integrator);
        if allowed && !tasks[integrate_idx].writes {
            tasks[integrate_idx].writes = true;
            warnings
                .push("Bulk plan: forcing integrate task writes=true for the integrator.".into());
        }
    }

    warnings
}

fn validate_bulk_plan(
    tasks: &[SwarmTask],
    available_agents: &[String],
    integrator_agent_id: Option<&str>,
) -> Result<(), String> {
    let mut issues = Vec::new();
    let proposer_tasks = tasks
        .iter()
        .filter(|task| bulk_is_proposer(task))
        .collect::<Vec<_>>();
    let judge_task = tasks.iter().find(|task| bulk_is_judge(task));
    let integrate_task = tasks.iter().find(|task| bulk_is_integrate(task));

    if proposer_tasks.is_empty() {
        issues.push("missing proposer tasks (role=propose or id=propose-XX)".into());
    }
    if judge_task.is_none() {
        issues.push("missing judge task (role=judge or id=judge)".into());
    }
    if integrate_task.is_none() {
        issues.push("missing integrate task (role=integrate or id=integrate)".into());
    }

    if let Some(integrate_task) = integrate_task {
        if !integrate_task.writes {
            issues.push("integrate task must set writes=true (integrator step)".into());
        }
        if let Some(integrator) = integrator_agent_id {
            if integrate_task.agent_id != integrator {
                issues.push(format!(
                    "integrate task must be assigned to integrator agent '{integrator}' (got '{}')",
                    integrate_task.agent_id
                ));
            }
        }
    }

    if let Some(judge_task) = judge_task {
        for proposer in proposer_tasks.iter() {
            if proposer.id == judge_task.id {
                continue;
            }
            if !judge_task.deps.iter().any(|dep| dep == &proposer.id) {
                issues.push(format!(
                    "judge task must depend on proposer task '{}' (missing dep)",
                    proposer.id
                ));
            }
        }
    }

    if let (Some(judge_task), Some(integrate_task)) = (judge_task, integrate_task) {
        if !integrate_task.deps.iter().any(|dep| dep == &judge_task.id) {
            issues.push("integrate task must depend on judge task".into());
        }
    }

    let non_integrator_agents = match integrator_agent_id {
        Some(integrator) => available_agents
            .iter()
            .filter(|id| id.as_str() != integrator)
            .count(),
        None => available_agents.len(),
    };
    let min_proposers = if non_integrator_agents >= 2 { 2 } else { 1 };
    if proposer_tasks.len() < min_proposers {
        issues.push(format!(
            "expected at least {min_proposers} proposer tasks for bulk orchestration"
        ));
    }

    if issues.is_empty() {
        Ok(())
    } else {
        Err(issues.join("; "))
    }
}

#[derive(Copy, Clone, Debug, Default)]
struct RoleDepStats {
    added: usize,
    skipped_cycle: usize,
}

pub(crate) fn normalize_role_label(raw: &str) -> Option<String> {
    let role = raw.trim().to_ascii_lowercase();
    if role.is_empty() {
        return None;
    }
    if role.eq_ignore_ascii_case("all") {
        return None;
    }
    if role.eq_ignore_ascii_case(COMPUTATIONAL_RESEARCH_ROLE_LEGACY) {
        return Some(COMPUTATIONAL_RESEARCH_ROLE.into());
    }
    Some(role)
}

fn infer_role_from_task_id(task_id: &str) -> Option<&'static str> {
    let id = task_id.trim();
    if id.is_empty() {
        return None;
    }
    if id.to_ascii_lowercase().starts_with("propose-") {
        return Some("propose");
    }
    if id.eq_ignore_ascii_case("judge") {
        return Some("judge");
    }
    if id.eq_ignore_ascii_case("integrate") || id.eq_ignore_ascii_case("implement") {
        return Some("integrate");
    }
    if id.eq_ignore_ascii_case("review") {
        return Some("review");
    }
    if id.eq_ignore_ascii_case("test") {
        return Some("test");
    }
    None
}

fn infer_integrator_agent_id_from_v2_tasks(
    tasks: &[SwarmPlanTaskV2],
    available_agents: &[String],
) -> Option<(String, &'static str)> {
    let normalize_agent_id = |raw: &str| {
        let raw = raw.trim();
        available_agents
            .iter()
            .find(|candidate| candidate.as_str() == raw)
            .cloned()
    };

    let mut integrate_agents = Vec::new();
    let mut writer_agents = Vec::new();
    for task in tasks.iter() {
        let Some(agent_id) = normalize_agent_id(task.agent_id.as_str()) else {
            continue;
        };

        let has_integrate_role = task
            .role
            .as_deref()
            .and_then(normalize_role_label)
            .as_deref()
            == Some("integrate")
            || task
                .id
                .as_deref()
                .and_then(infer_role_from_task_id)
                .is_some_and(|role| role == "integrate");
        if has_integrate_role
            && !integrate_agents
                .iter()
                .any(|existing| existing == &agent_id)
        {
            integrate_agents.push(agent_id.clone());
        }
        if task.writes && !writer_agents.iter().any(|existing| existing == &agent_id) {
            writer_agents.push(agent_id);
        }
    }

    if integrate_agents.len() == 1
        && (writer_agents.is_empty() || writer_agents.iter().all(|id| id == &integrate_agents[0]))
    {
        let reason = if writer_agents.is_empty() {
            "integrate task"
        } else {
            "integrate task + writes=true task"
        };
        return Some((integrate_agents.remove(0), reason));
    }

    if writer_agents.len() == 1 && integrate_agents.is_empty() {
        return Some((writer_agents.remove(0), "writes=true task"));
    }

    None
}

fn default_role_deps() -> HashMap<String, Vec<String>> {
    let mut map = HashMap::new();
    map.insert("consumer".into(), vec!["producer".into()]);
    map.insert(
        "judge".into(),
        vec![
            "research".into(),
            COMPUTATIONAL_RESEARCH_ROLE.into(),
            "propose".into(),
        ],
    );
    map.insert(
        "integrate".into(),
        vec![
            "judge".into(),
            "research".into(),
            COMPUTATIONAL_RESEARCH_ROLE.into(),
            "propose".into(),
        ],
    );
    map.insert("review".into(), vec!["integrate".into()]);
    map.insert("test".into(), vec!["integrate".into()]);
    map
}

fn read_workspace_role_deps(
    workspace_root: &Path,
) -> Result<Option<HashMap<String, Vec<String>>>, String> {
    let path = workspace_root.join(".nit").join("config.toml");
    if !path.exists() {
        return Ok(None);
    }
    let contents = fs::read_to_string(&path)
        .map_err(|err| format!("failed reading {}: {err}", path.display()))?;
    let value = toml::from_str::<toml::Value>(&contents)
        .map_err(|err| format!("failed parsing {}: {err}", path.display()))?;
    let table = value
        .get("swarm")
        .and_then(|value| value.get("role_deps"))
        .and_then(|value| value.as_table());
    let Some(table) = table else {
        return Ok(None);
    };

    let mut out: HashMap<String, Vec<String>> = HashMap::new();
    for (consumer, producers) in table.iter() {
        let Some(consumer) = normalize_role_label(consumer) else {
            continue;
        };
        let mut normalized = Vec::new();
        if let Some(producers) = producers.as_array() {
            for producer in producers.iter() {
                let Some(producer) = producer.as_str().and_then(normalize_role_label) else {
                    continue;
                };
                if normalized
                    .iter()
                    .any(|existing: &String| existing == &producer)
                {
                    continue;
                }
                normalized.push(producer);
            }
        } else if let Some(producer) = producers.as_str().and_then(normalize_role_label) {
            normalized.push(producer);
        } else {
            continue;
        }
        if !normalized.is_empty() {
            out.insert(consumer, normalized);
        }
    }

    if out.is_empty() {
        Ok(None)
    } else {
        Ok(Some(out))
    }
}

fn would_create_cycle(
    tasks: &[SwarmTask],
    idx_by_id: &HashMap<String, usize>,
    task_id: &str,
    dep_id: &str,
) -> bool {
    if task_id == dep_id {
        return true;
    }
    let Some(&start) = idx_by_id.get(dep_id) else {
        return false;
    };
    let Some(&target) = idx_by_id.get(task_id) else {
        return false;
    };

    let mut seen: HashSet<usize> = HashSet::new();
    let mut stack = vec![start];
    while let Some(idx) = stack.pop() {
        if idx == target {
            return true;
        }
        if !seen.insert(idx) {
            continue;
        }
        for dep in tasks[idx].deps.iter() {
            if let Some(&next) = idx_by_id.get(dep) {
                stack.push(next);
            }
        }
    }
    false
}

fn apply_role_deps(
    tasks: &mut [SwarmTask],
    role_deps: &HashMap<String, Vec<String>>,
) -> RoleDepStats {
    let mut stats = RoleDepStats::default();
    if tasks.is_empty() || role_deps.is_empty() {
        return stats;
    }

    let idx_by_id = tasks
        .iter()
        .enumerate()
        .map(|(idx, task)| (task.id.clone(), idx))
        .collect::<HashMap<_, _>>();

    let mut tasks_by_role: HashMap<String, Vec<String>> = HashMap::new();
    for task in tasks.iter() {
        if let Some(role) = task.role.as_deref().and_then(normalize_role_label) {
            tasks_by_role.entry(role).or_default().push(task.id.clone());
        }
        if task.writes {
            // Treat writer tasks as integrate-like for role-based ordering.
            let entry = tasks_by_role.entry("integrate".into()).or_default();
            if !entry.iter().any(|id| id == &task.id) {
                entry.push(task.id.clone());
            }
        }
    }

    let mut consumer_roles = role_deps.keys().cloned().collect::<Vec<_>>();
    consumer_roles.sort();
    for consumer_role in consumer_roles.iter() {
        let Some(producer_roles) = role_deps.get(consumer_role) else {
            continue;
        };
        let Some(consumer_task_ids) = tasks_by_role.get(consumer_role) else {
            continue;
        };
        if consumer_task_ids.is_empty() {
            continue;
        }
        for consumer_task_id in consumer_task_ids.iter() {
            let Some(&consumer_idx) = idx_by_id.get(consumer_task_id) else {
                continue;
            };
            for producer_role in producer_roles.iter() {
                let Some(producer_task_ids) = tasks_by_role.get(producer_role) else {
                    continue;
                };
                for producer_task_id in producer_task_ids.iter() {
                    if producer_task_id == consumer_task_id {
                        continue;
                    }
                    if tasks[consumer_idx]
                        .deps
                        .iter()
                        .any(|existing| existing == producer_task_id)
                    {
                        continue;
                    }
                    if would_create_cycle(
                        tasks,
                        &idx_by_id,
                        consumer_task_id.as_str(),
                        producer_task_id.as_str(),
                    ) {
                        stats.skipped_cycle = stats.skipped_cycle.saturating_add(1);
                        continue;
                    }
                    tasks[consumer_idx].deps.push(producer_task_id.clone());
                    stats.added = stats.added.saturating_add(1);
                }
            }
        }
    }

    stats
}

fn apply_role_dependency_ordering(
    workspace_root: &Path,
    role_hints_by_agent_id: &HashMap<String, String>,
    tasks: &mut [SwarmTask],
) -> Vec<String> {
    if tasks.is_empty() {
        return Vec::new();
    }

    let mut warnings = Vec::new();
    let mut inferred_roles = 0usize;
    for task in tasks.iter_mut() {
        if task.role.as_deref().is_some_and(|r| !r.trim().is_empty()) {
            continue;
        }
        if task.writes {
            task.role = Some("integrate".into());
            inferred_roles = inferred_roles.saturating_add(1);
            continue;
        }
        if let Some(inferred) = infer_role_from_task_id(task.id.as_str()) {
            task.role = Some(inferred.to_string());
            inferred_roles = inferred_roles.saturating_add(1);
            continue;
        }
        let hint = role_hints_by_agent_id.get(&task.agent_id).or_else(|| {
            swarm_clone_base_id(task.agent_id.as_str())
                .and_then(|base| role_hints_by_agent_id.get(base))
        });
        let Some(hint) = hint.and_then(|hint| normalize_role_label(hint.as_str())) else {
            continue;
        };
        task.role = Some(hint);
        inferred_roles = inferred_roles.saturating_add(1);
    }

    let (role_deps, source) = match read_workspace_role_deps(workspace_root) {
        Ok(Some(map)) => (map, "config"),
        Ok(None) => (default_role_deps(), "built-in"),
        Err(err) => {
            warnings.push(format!("Role ordering: {err}; using built-in role deps."));
            (default_role_deps(), "built-in")
        }
    };

    let stats = apply_role_deps(tasks, &role_deps);
    if stats.added > 0 || stats.skipped_cycle > 0 {
        let mut parts = Vec::new();
        if inferred_roles > 0 {
            parts.push(format!("inferred {inferred_roles} role(s)"));
        }
        if stats.added > 0 {
            parts.push(format!("added {} dep(s)", stats.added));
        }
        if stats.skipped_cycle > 0 {
            parts.push(format!("skipped {} dep(s) (cycle)", stats.skipped_cycle));
        }
        if parts.is_empty() {
            parts.push("no changes".into());
        }
        warnings.push(format!("Role ordering ({source}): {}.", parts.join(", ")));
    }

    warnings
}

fn parse_plan_from_planner(
    planner_message: &str,
    template: SwarmTemplate,
    root_prompt: &str,
    available_agents: &[String],
    integrator_hint: Option<&str>,
    integrator_locked: bool,
) -> ParsedSwarmPlan {
    let Some(json) = extract_json_code_block(planner_message) else {
        return fallback_tasks(
            template,
            root_prompt,
            available_agents,
            None,
            integrator_hint,
        );
    };

    if let Ok(plan) = serde_json::from_str::<SwarmPlanV2>(&json) {
        if let Some(mut parsed) = parse_v2_plan(
            plan,
            template,
            available_agents,
            integrator_hint,
            integrator_locked,
        ) {
            if matches!(template, SwarmTemplate::Bulk) {
                let integrator = parsed
                    .integrator_agent_id
                    .as_deref()
                    .or(integrator_hint)
                    .or_else(|| available_agents.first().map(String::as_str));
                let mut warnings = normalize_bulk_plan(&mut parsed.tasks, integrator);
                parsed.warnings.append(&mut warnings);
                if let Err(issue) = validate_bulk_plan(&parsed.tasks, available_agents, integrator)
                {
                    let mut fallback = fallback_tasks(
                        template,
                        root_prompt,
                        available_agents,
                        Some(&issue),
                        integrator_hint,
                    );
                    fallback.warnings.push(format!(
                        "Planner did not produce a usable bulk plan; using built-in bulk workflow. Reason: {issue}"
                    ));
                    return fallback;
                }
            }
            return parsed;
        }
    }

    if matches!(template, SwarmTemplate::Bulk) {
        let mut fallback = fallback_tasks(
            template,
            root_prompt,
            available_agents,
            Some("Planner did not return a valid v2 bulk plan."),
            integrator_hint,
        );
        fallback.warnings.push(
            "Bulk template requires the v2 JSON schema (with deps); using built-in bulk workflow."
                .into(),
        );
        return fallback;
    }

    let Ok(plan) = serde_json::from_str::<SwarmPlanV1>(&json) else {
        return fallback_tasks(
            template,
            root_prompt,
            available_agents,
            None,
            integrator_hint,
        );
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
            parsed_artifacts: None,
            expected_artifacts_missing: false,
            failed: false,
        });
    }

    if tasks.is_empty() {
        return fallback_tasks(
            template,
            root_prompt,
            available_agents,
            None,
            integrator_hint,
        );
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
    integrator_locked: bool,
) -> Option<ParsedSwarmPlan> {
    if plan.tasks.is_empty() {
        return None;
    }
    if let Some(version) = plan.version {
        if version != 2 {
            return None;
        }
    }

    let integrator_plan = plan
        .integrator_agent_id
        .as_deref()
        .map(str::trim)
        .filter(|id| !id.is_empty());
    let integrator_hint = integrator_hint.map(str::trim).filter(|id| !id.is_empty());
    let integrator_locked = integrator_locked && integrator_hint.is_some();

    let mut warnings = Vec::new();
    if integrator_locked {
        if let (Some(plan_id), Some(hint_id)) = (integrator_plan, integrator_hint) {
            if !plan_id.eq_ignore_ascii_case(hint_id) {
                warnings.push(format!(
                    "Planner returned integrator_agent_id '{plan_id}' but integrator is locked to '{hint_id}'; ignoring planner override."
                ));
            }
        }
    }
    let inferred_integrator = if integrator_locked || integrator_plan.is_some() {
        None
    } else {
        infer_integrator_agent_id_from_v2_tasks(plan.tasks.as_slice(), available_agents)
    };
    if let Some((agent_id, reason)) = inferred_integrator.as_ref() {
        warnings.push(format!(
            "Planner omitted integrator_agent_id; inferred integrator '{agent_id}' from {reason}."
        ));
    }

    let integrator_candidate = if integrator_locked {
        integrator_hint
    } else {
        integrator_plan
            .or(inferred_integrator
                .as_ref()
                .map(|(agent_id, _)| agent_id.as_str()))
            .or(integrator_hint)
    };
    let integrator = integrator_candidate.and_then(|id| {
        available_agents
            .iter()
            .find(|candidate| candidate.as_str() == id)
            .map(|id| id.to_string())
    });
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
            parsed_artifacts: None,
            expected_artifacts_missing: false,
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
    integrator_hint: Option<&str>,
) -> ParsedSwarmPlan {
    if matches!(template, SwarmTemplate::Bulk) {
        let integrator = integrator_hint
            .map(str::trim)
            .filter(|id| !id.is_empty())
            .and_then(|id| {
                available_agents
                    .iter()
                    .find(|candidate| candidate.as_str() == id)
                    .cloned()
            })
            .or_else(|| available_agents.first().cloned());
        let judge_agent = available_agents
            .iter()
            .find(|id| integrator.as_ref() != Some(*id))
            .cloned()
            .or_else(|| integrator.clone());
        let review_agent = available_agents
            .iter()
            .find(|id| integrator.as_ref() != Some(*id) && judge_agent.as_ref() != Some(*id))
            .cloned()
            .or_else(|| judge_agent.clone())
            .or_else(|| integrator.clone());

        let mut proposer_ids = available_agents
            .iter()
            .filter(|id| integrator.as_ref() != Some(*id))
            .filter(|id| judge_agent.as_ref() != Some(*id))
            .cloned()
            .collect::<Vec<_>>();
        if proposer_ids.is_empty() {
            if let Some(judge) = judge_agent.clone() {
                proposer_ids.push(judge);
            } else if let Some(integrator) = integrator.clone() {
                proposer_ids.push(integrator);
            }
        }
        proposer_ids.truncate(8);

        let proposer_lenses = [
            "minimal diff / safest change",
            "correctness & edge cases",
            "UX/TUI clarity",
            "performance & scalability",
            "testing & verification",
            "docs & maintainability",
            "security & failure modes",
        ];

        let mut tasks = Vec::new();
        let mut proposer_task_ids = Vec::new();
        for (idx, agent_id) in proposer_ids.into_iter().enumerate() {
            let id = format!("propose-{:02}", idx + 1);
            let lens = proposer_lenses
                .get(idx)
                .copied()
                .unwrap_or("alternative approach");
            proposer_task_ids.push(id.clone());
            tasks.push(SwarmTask {
                id,
                agent_id,
                role: Some("propose".into()),
                title: format!("Proposal ({lens})"),
                task_prompt: format!(
                    "Propose an end-to-end solution candidate.\n\nLens: {lens}\n\nConstraints:\n- Do NOT edit the workspace (read-only).\n- Be concrete: file paths, key symbols, and exact commands.\n- If helpful, include a small unified diff (but do not apply it).\n"
                ),
                deps: Vec::new(),
                writes: false,
                artifacts: vec!["options".into(), "files".into(), "commands".into(), "risks".into()],
                done_when: Some(
                    "We have a concrete, repo-grounded candidate solution with tradeoffs."
                        .into(),
                ),
                state: SwarmTaskState::Pending,
                output: None,
                parsed_artifacts: None,
                expected_artifacts_missing: false,
                failed: false,
            });
        }

        if let Some(agent_id) = judge_agent.clone() {
            tasks.push(SwarmTask {
                id: "judge".into(),
                agent_id,
                role: Some("judge".into()),
                title: "Judge + select approach".into(),
                task_prompt: "Compare the proposer outputs and pick the best approach. Provide:\n- Decision (which proposal / why)\n- A step-by-step integration plan for the integrator\n- Acceptance criteria\n- Exact verification commands\n\nConstraints:\n- Do NOT edit the workspace (read-only).\n"
                    .into(),
                deps: proposer_task_ids.clone(),
                writes: false,
                artifacts: vec!["decision".into(), "plan".into(), "commands".into(), "risks".into()],
                done_when: Some("Integrator has a clear, actionable plan to implement.".into()),
                state: SwarmTaskState::Pending,
                output: None,
                parsed_artifacts: None,
                expected_artifacts_missing: false,
                failed: false,
            });
        }

        if let Some(agent_id) = integrator.clone() {
            tasks.push(SwarmTask {
                id: "integrate".into(),
                agent_id,
                role: Some("integrate".into()),
                title: "Integrate selected approach".into(),
                task_prompt: "Implement the selected approach using the judge output.\n\nConstraints:\n- You are the ONLY agent allowed to edit the workspace.\n- Prefer small, safe diffs.\n- Run the suggested verification commands.\n"
                    .into(),
                deps: vec!["judge".into()],
                writes: true,
                artifacts: vec!["diffs".into(), "commands".into()],
                done_when: Some("Changes are implemented cleanly with validations passing.".into()),
                state: SwarmTaskState::Pending,
                output: None,
                parsed_artifacts: None,
                expected_artifacts_missing: false,
                failed: false,
            });
        }

        if let Some(agent_id) = review_agent {
            tasks.push(SwarmTask {
                id: "review".into(),
                agent_id,
                role: Some("review".into()),
                title: "Review final diff".into(),
                task_prompt: "Review the integrated changes for correctness, UX, and maintainability. Suggest follow-ups and edge cases.\n\nConstraints:\n- Do NOT edit the workspace (read-only).\n"
                    .into(),
                deps: vec!["integrate".into()],
                writes: false,
                artifacts: vec!["risks".into(), "commands".into()],
                done_when: Some("We have confidence in correctness and know remaining risks.".into()),
                state: SwarmTaskState::Pending,
                output: None,
                parsed_artifacts: None,
                expected_artifacts_missing: false,
                failed: false,
            });
        }

        let synth = plan_error.map(|err| {
            format!(
                "Note: planner output could not be used; fallback prompts were used. Reason: {err}"
            )
        });

        return ParsedSwarmPlan {
            tasks,
            synthesis_prompt: synth,
            integrator_agent_id: integrator,
            warnings: Vec::new(),
        };
    }
    if matches!(template, SwarmTemplate::Lab) {
        let integrator = integrator_hint
            .map(str::trim)
            .filter(|id| !id.is_empty())
            .and_then(|id| {
                available_agents
                    .iter()
                    .find(|candidate| candidate.as_str() == id)
                    .cloned()
            })
            .or_else(|| available_agents.first().cloned());
        let recon_agent = available_agents
            .iter()
            .find(|id| integrator.as_ref() != Some(*id))
            .cloned()
            .or_else(|| integrator.clone());
        let design_agent = available_agents
            .iter()
            .find(|id| integrator.as_ref() != Some(*id) && recon_agent.as_ref() != Some(*id))
            .cloned()
            .or_else(|| recon_agent.clone())
            .or_else(|| integrator.clone());
        let review_agent = available_agents
            .iter()
            .find(|id| {
                integrator.as_ref() != Some(*id)
                    && recon_agent.as_ref() != Some(*id)
                    && design_agent.as_ref() != Some(*id)
            })
            .cloned()
            .or_else(|| design_agent.clone())
            .or_else(|| recon_agent.clone())
            .or_else(|| integrator.clone());

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
                parsed_artifacts: None,
                expected_artifacts_missing: false,
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
                parsed_artifacts: None,
                expected_artifacts_missing: false,
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
                parsed_artifacts: None,
                expected_artifacts_missing: false,
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
                parsed_artifacts: None,
                expected_artifacts_missing: false,
                failed: false,
            });
        }

        let synth = plan_error.map(|err| {
            format!(
                "Note: planner output could not be used; fallback prompts were used. Reason: {err}"
            )
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
            parsed_artifacts: None,
            expected_artifacts_missing: false,
            failed: false,
        });
    }

    let synth = plan_error.map(|err| {
        format!("Note: planner output could not be used; fallback prompts were used. Reason: {err}")
    });

    ParsedSwarmPlan {
        tasks,
        synthesis_prompt: synth,
        integrator_agent_id: None,
        warnings: Vec::new(),
    }
}

#[derive(Clone, Debug, Default)]
struct SwarmDagIssues {
    unknown_deps: Vec<(String, String)>,
    cycle: Option<Vec<String>>,
}

impl SwarmDagIssues {
    fn is_empty(&self) -> bool {
        self.unknown_deps.is_empty() && self.cycle.is_none()
    }

    fn summary(&self) -> String {
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

fn analyze_swarm_dag(tasks: &[SwarmTask]) -> SwarmDagIssues {
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

fn find_swarm_cycle_path(tasks: &[SwarmTask]) -> Option<Vec<String>> {
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

fn repair_swarm_dag(tasks: &mut [SwarmTask]) -> Vec<String> {
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

struct TaskCompletion {
    task_id: String,
    expected_artifacts_missing: bool,
}

fn mark_task_finished(
    run: &mut SwarmRun,
    agent_id: &str,
    message: String,
    failed: bool,
) -> Option<TaskCompletion> {
    let pos_running = run.tasks.iter().position(|task| {
        task.agent_id == agent_id && matches!(task.state, SwarmTaskState::Running)
    });
    let pos = pos_running.or_else(|| {
        run.tasks.iter().position(|task| {
            task.agent_id == agent_id && matches!(task.state, SwarmTaskState::Dispatched)
        })
    });
    let pos = pos?;

    let parsed_artifacts = parse_task_artifacts(&run.tasks[pos].id, &message);
    let expected_artifacts_missing =
        !run.tasks[pos].artifacts.is_empty() && parsed_artifacts.is_none();

    let task = &mut run.tasks[pos];
    task.output = Some(message);
    task.parsed_artifacts = parsed_artifacts;
    task.expected_artifacts_missing = expected_artifacts_missing;
    task.failed = failed;
    task.state = if failed {
        SwarmTaskState::Failed
    } else {
        SwarmTaskState::Done
    };
    Some(TaskCompletion {
        task_id: task.id.clone(),
        expected_artifacts_missing,
    })
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

#[derive(Clone, Debug, PartialEq, Eq)]
struct SwarmDeadlock {
    skipped: Vec<String>,
    message: String,
}

fn maybe_resolve_deadlock(run: &mut SwarmRun) -> Option<SwarmDeadlock> {
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
        let text = dependency_payload_text(run, dep);
        out.push((label, truncate_chars(&text, SWARM_DEP_OUTPUT_MAX_CHARS)));
    }
    out
}

fn dependency_payload_text(run: &SwarmRun, task: &SwarmTask) -> String {
    if let Some(summary) = task_artifacts_summary_for_prompt(task, &run.mission_id) {
        return summary;
    }
    task.output
        .as_deref()
        .map(ToString::to_string)
        .unwrap_or_else(|| "(no output)".into())
}

fn task_artifacts_summary_for_prompt(task: &SwarmTask, mission_id: &str) -> Option<String> {
    let artifacts = task.parsed_artifacts.as_ref()?;
    if artifacts.is_empty() {
        return None;
    }

    let mut lines = Vec::new();
    if let Some(summary) = artifacts
        .summary
        .as_deref()
        .map(str::trim)
        .filter(|summary| !summary.is_empty())
    {
        lines.push(format!("summary: {summary}"));
    }
    if !artifacts.files.is_empty() {
        let files = artifacts
            .files
            .iter()
            .take(8)
            .map(|entry| match entry.notes.as_deref().map(str::trim) {
                Some(notes) if !notes.is_empty() => format!("{} ({notes})", entry.path),
                _ => entry.path.clone(),
            })
            .collect::<Vec<_>>()
            .join("; ");
        lines.push(format!("files: {files}"));
    }
    if !artifacts.diffs.is_empty() {
        let diffs = artifacts
            .diffs
            .iter()
            .take(8)
            .map(|entry| match entry.path.as_deref().map(str::trim) {
                Some(path) if !path.is_empty() => format!("{path}: {}", entry.summary),
                _ => entry.summary.clone(),
            })
            .collect::<Vec<_>>()
            .join("; ");
        lines.push(format!("diffs: {diffs}"));
    }
    if !artifacts.commands.is_empty() {
        let commands = artifacts
            .commands
            .iter()
            .take(8)
            .map(|entry| match entry.purpose.as_deref().map(str::trim) {
                Some(purpose) if !purpose.is_empty() => format!("{} ({purpose})", entry.cmd),
                _ => entry.cmd.clone(),
            })
            .collect::<Vec<_>>()
            .join("; ");
        lines.push(format!("commands: {commands}"));
    }
    if !artifacts.risks.is_empty() {
        let risks = artifacts
            .risks
            .iter()
            .take(8)
            .map(|entry| {
                let prefix = entry
                    .level
                    .as_deref()
                    .map(str::trim)
                    .filter(|level| !level.is_empty())
                    .map(|level| format!("{level}: "))
                    .unwrap_or_default();
                let mitigation = entry
                    .mitigation
                    .as_deref()
                    .map(str::trim)
                    .filter(|text| !text.is_empty())
                    .map(|text| format!(" (mitigation: {text})"))
                    .unwrap_or_default();
                format!("{prefix}{}{}", entry.item, mitigation)
            })
            .collect::<Vec<_>>()
            .join("; ");
        lines.push(format!("risks: {risks}"));
    }
    if !artifacts.notes.is_empty() {
        lines.push(format!(
            "notes: {}",
            artifacts
                .notes
                .iter()
                .take(8)
                .cloned()
                .collect::<Vec<_>>()
                .join("; ")
        ));
    }
    lines.push(format!(
        "artifact_path: .nit/swarm/{mission_id}/tasks/{}/artifacts.json",
        sanitize_for_filename(&task.id)
    ));
    Some(lines.join("\n"))
}

fn parse_task_artifacts(task_id: &str, message: &str) -> Option<SwarmTaskArtifacts> {
    let mut merged = SwarmTaskArtifacts::default();
    let mut found = false;
    for json in extract_json_code_blocks(message) {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&json) else {
            continue;
        };
        let Some(parsed) = parse_task_artifacts_value(task_id, &value) else {
            continue;
        };
        merge_task_artifacts(&mut merged, parsed);
        found = true;
    }
    if found && !merged.is_empty() {
        Some(merged)
    } else {
        None
    }
}

fn parse_task_artifacts_value(
    task_id: &str,
    value: &serde_json::Value,
) -> Option<SwarmTaskArtifacts> {
    let object = value.as_object()?;
    let typed = object
        .get("type")
        .and_then(|value| value.as_str())
        .is_some_and(|value| value.eq_ignore_ascii_case("swarm_artifacts"));
    let has_artifacts = object.contains_key("artifacts");
    if !typed && !has_artifacts {
        return None;
    }
    if typed
        && object
            .get("version")
            .and_then(|value| value.as_u64())
            .is_some_and(|version| version != 1)
    {
        return None;
    }
    if let Some(owner) = object.get("task_id").and_then(|value| value.as_str()) {
        let owner = owner.trim();
        if !owner.is_empty() && owner != task_id {
            return None;
        }
    }

    let mut parsed = SwarmTaskArtifacts {
        summary: object
            .get("summary")
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|summary| !summary.is_empty())
            .map(ToString::to_string),
        ..SwarmTaskArtifacts::default()
    };

    let source = object.get("artifacts").unwrap_or(value);
    let source_obj = source.as_object()?;

    parsed.files = parse_artifact_files(source_obj.get("files"));
    parsed.diffs = parse_artifact_diffs(source_obj.get("diffs"));
    parsed.commands = parse_artifact_commands(source_obj.get("commands"));
    parsed.risks = parse_artifact_risks(source_obj.get("risks"));
    parsed.notes = parse_artifact_notes(source_obj.get("notes"));

    if parsed.is_empty() {
        None
    } else {
        Some(parsed)
    }
}

fn parse_artifact_files(value: Option<&serde_json::Value>) -> Vec<SwarmArtifactFile> {
    let Some(items) = value.and_then(|value| value.as_array()) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for item in items.iter() {
        if let Some(path) = item.as_str().map(str::trim).filter(|path| !path.is_empty()) {
            out.push(SwarmArtifactFile {
                path: path.to_string(),
                notes: None,
            });
            continue;
        }
        let Some(obj) = item.as_object() else {
            continue;
        };
        let Some(path) = obj
            .get("path")
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|path| !path.is_empty())
        else {
            continue;
        };
        let notes = obj
            .get("notes")
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|notes| !notes.is_empty())
            .map(ToString::to_string);
        out.push(SwarmArtifactFile {
            path: path.to_string(),
            notes,
        });
    }
    out
}

fn parse_artifact_diffs(value: Option<&serde_json::Value>) -> Vec<SwarmArtifactDiff> {
    let Some(items) = value.and_then(|value| value.as_array()) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for item in items.iter() {
        if let Some(summary) = item
            .as_str()
            .map(str::trim)
            .filter(|summary| !summary.is_empty())
        {
            out.push(SwarmArtifactDiff {
                path: None,
                summary: summary.to_string(),
            });
            continue;
        }
        let Some(obj) = item.as_object() else {
            continue;
        };
        let summary = obj
            .get("summary")
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|summary| !summary.is_empty());
        let path = obj
            .get("path")
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|path| !path.is_empty())
            .map(ToString::to_string);
        let summary = summary.map(ToString::to_string).or_else(|| path.clone());
        let Some(summary) = summary else {
            continue;
        };
        out.push(SwarmArtifactDiff { path, summary });
    }
    out
}

fn parse_artifact_commands(value: Option<&serde_json::Value>) -> Vec<SwarmArtifactCommand> {
    let Some(items) = value.and_then(|value| value.as_array()) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for item in items.iter() {
        if let Some(cmd) = item.as_str().map(str::trim).filter(|cmd| !cmd.is_empty()) {
            out.push(SwarmArtifactCommand {
                cmd: cmd.to_string(),
                purpose: None,
            });
            continue;
        }
        let Some(obj) = item.as_object() else {
            continue;
        };
        let Some(cmd) = obj
            .get("cmd")
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|cmd| !cmd.is_empty())
        else {
            continue;
        };
        let purpose = obj
            .get("purpose")
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|purpose| !purpose.is_empty())
            .map(ToString::to_string);
        out.push(SwarmArtifactCommand {
            cmd: cmd.to_string(),
            purpose,
        });
    }
    out
}

fn parse_artifact_risks(value: Option<&serde_json::Value>) -> Vec<SwarmArtifactRisk> {
    let Some(items) = value.and_then(|value| value.as_array()) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for item in items.iter() {
        if let Some(item_text) = item
            .as_str()
            .map(str::trim)
            .filter(|item_text| !item_text.is_empty())
        {
            out.push(SwarmArtifactRisk {
                level: None,
                item: item_text.to_string(),
                mitigation: None,
            });
            continue;
        }
        let Some(obj) = item.as_object() else {
            continue;
        };
        let Some(item_text) = obj
            .get("item")
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|item_text| !item_text.is_empty())
        else {
            continue;
        };
        let level = obj
            .get("level")
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|level| !level.is_empty())
            .map(ToString::to_string);
        let mitigation = obj
            .get("mitigation")
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|mitigation| !mitigation.is_empty())
            .map(ToString::to_string);
        out.push(SwarmArtifactRisk {
            level,
            item: item_text.to_string(),
            mitigation,
        });
    }
    out
}

fn parse_artifact_notes(value: Option<&serde_json::Value>) -> Vec<String> {
    let Some(items) = value.and_then(|value| value.as_array()) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for item in items.iter() {
        if let Some(note) = item.as_str().map(str::trim).filter(|note| !note.is_empty()) {
            out.push(note.to_string());
            continue;
        }
        let Some(obj) = item.as_object() else {
            continue;
        };
        if let Some(note) = obj
            .get("note")
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|note| !note.is_empty())
        {
            out.push(note.to_string());
        }
    }
    out
}

fn merge_task_artifacts(dst: &mut SwarmTaskArtifacts, src: SwarmTaskArtifacts) {
    if let Some(summary) = src
        .summary
        .as_deref()
        .map(str::trim)
        .filter(|summary| !summary.is_empty())
    {
        dst.summary = Some(summary.to_string());
    }

    let mut seen_files = dst
        .files
        .iter()
        .map(|entry| entry.path.to_ascii_lowercase())
        .collect::<HashSet<_>>();
    for entry in src.files {
        let key = entry.path.to_ascii_lowercase();
        if key.is_empty() || !seen_files.insert(key) {
            continue;
        }
        dst.files.push(entry);
    }

    let mut seen_diffs = dst
        .diffs
        .iter()
        .map(|entry| {
            format!(
                "{}|{}",
                entry
                    .path
                    .as_deref()
                    .unwrap_or_default()
                    .to_ascii_lowercase(),
                entry.summary.to_ascii_lowercase()
            )
        })
        .collect::<HashSet<_>>();
    for entry in src.diffs {
        let key = format!(
            "{}|{}",
            entry
                .path
                .as_deref()
                .unwrap_or_default()
                .to_ascii_lowercase(),
            entry.summary.to_ascii_lowercase()
        );
        if key == "|" || !seen_diffs.insert(key) {
            continue;
        }
        dst.diffs.push(entry);
    }

    let mut seen_commands = dst
        .commands
        .iter()
        .map(|entry| entry.cmd.to_ascii_lowercase())
        .collect::<HashSet<_>>();
    for entry in src.commands {
        let key = entry.cmd.to_ascii_lowercase();
        if key.is_empty() || !seen_commands.insert(key) {
            continue;
        }
        dst.commands.push(entry);
    }

    let mut seen_risks = dst
        .risks
        .iter()
        .map(|entry| entry.item.to_ascii_lowercase())
        .collect::<HashSet<_>>();
    for entry in src.risks {
        let key = entry.item.to_ascii_lowercase();
        if key.is_empty() || !seen_risks.insert(key) {
            continue;
        }
        dst.risks.push(entry);
    }

    let mut seen_notes = dst
        .notes
        .iter()
        .map(|note| note.to_ascii_lowercase())
        .collect::<HashSet<_>>();
    for note in src.notes {
        let key = note.to_ascii_lowercase();
        if key.is_empty() || !seen_notes.insert(key) {
            continue;
        }
        dst.notes.push(note);
    }
}

fn blocked_on(run: &SwarmRun, task: &SwarmTask) -> Vec<String> {
    task.deps
        .iter()
        .filter_map(|dep_id| {
            let dep = run.tasks.iter().find(|candidate| candidate.id == *dep_id)?;
            (!dep.state.is_terminal()).then(|| dep.id.clone())
        })
        .collect()
}

fn task_state_dashboard_label(state: SwarmTaskState) -> &'static str {
    match state {
        SwarmTaskState::Pending => "Pending",
        SwarmTaskState::Ready | SwarmTaskState::Dispatched => "Queued",
        SwarmTaskState::Running => "Running",
        SwarmTaskState::Done => "Done",
        SwarmTaskState::Failed => "Failed",
        SwarmTaskState::Skipped => "Skipped",
    }
}

fn stage_label(stage: SwarmStage) -> &'static str {
    match stage {
        SwarmStage::Planning => "PLAN",
        SwarmStage::Executing => "EXEC",
        SwarmStage::Verifying => "VERIFY",
        SwarmStage::Synthesizing => "SYNTH",
    }
}

fn dashboard_gate_rows(run: &SwarmRun) -> Vec<SwarmGateDashboardRow> {
    let mut rows = Vec::new();
    if let Some(bundle) = run.gate_bundle.as_ref() {
        for gate in bundle.gates() {
            rows.push(SwarmGateDashboardRow {
                name: gate.name.to_string(),
                command: gate.command.to_string(),
                status: "PENDING".into(),
                notes: None,
            });
        }
    }
    if let Some(report) = run.gate_report.as_ref() {
        for reported in report.gates.iter() {
            if let Some(existing) = rows.iter_mut().find(|row| row.name == reported.name) {
                existing.status = reported.ui_status().into();
                existing.command = reported.command.clone();
                existing.notes = reported.notes.clone();
            } else {
                rows.push(SwarmGateDashboardRow {
                    name: reported.name.clone(),
                    command: reported.command.clone(),
                    status: reported.ui_status().into(),
                    notes: reported.notes.clone(),
                });
            }
        }
    }
    rows
}

fn gate_bundle_label(bundle: Option<&GateBundle>, source: &str) -> String {
    let source = source.trim();
    if source.is_empty() {
        return bundle
            .map(|bundle| bundle.label().to_string())
            .unwrap_or_else(|| "(none)".into());
    }
    if source.eq_ignore_ascii_case("config:none") {
        return "none (config)".into();
    }
    match bundle {
        Some(bundle) => format!("{} ({source})", bundle.label()),
        None => format!("(none) ({source})"),
    }
}

fn read_workspace_gate_default(workspace_root: &Path) -> Result<Option<String>, String> {
    let path = workspace_root.join(".nit").join("config.toml");
    if !path.exists() {
        return Ok(None);
    }
    let contents = fs::read_to_string(&path)
        .map_err(|err| format!("failed reading {}: {err}", path.display()))?;
    let value = toml::from_str::<toml::Value>(&contents)
        .map_err(|err| format!("failed parsing {}: {err}", path.display()))?;
    Ok(value
        .get("swarm")
        .and_then(|value| value.get("gates"))
        .and_then(|value| value.get("default"))
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.to_ascii_lowercase()))
}

fn read_workspace_dag_validation_mode(
    workspace_root: &Path,
) -> Result<Option<SwarmDagValidationMode>, String> {
    let path = workspace_root.join(".nit").join("config.toml");
    if !path.exists() {
        return Ok(None);
    }
    let contents = fs::read_to_string(&path)
        .map_err(|err| format!("failed reading {}: {err}", path.display()))?;
    let value = toml::from_str::<toml::Value>(&contents)
        .map_err(|err| format!("failed parsing {}: {err}", path.display()))?;
    let Some(mode) = value
        .get("swarm")
        .and_then(|value| value.get("dag_validation"))
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return Ok(None);
    };

    let mode = mode.to_ascii_lowercase();
    if mode == "strict" || mode == "hard-fail" || mode == "hard_fail" || mode == "hardfail" {
        return Ok(Some(SwarmDagValidationMode::Strict));
    }
    if mode == "repair"
        || mode == "best-effort"
        || mode == "best_effort"
        || mode == "auto-repair"
        || mode == "auto_repair"
    {
        return Ok(Some(SwarmDagValidationMode::Repair));
    }

    Err(format!(
        "invalid swarm.dag_validation value '{mode}' (expected 'strict' or 'repair')"
    ))
}

fn sanitize_for_filename(input: &str) -> String {
    input
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_') {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

fn build_planner_prompt(
    root_prompt: &str,
    template: SwarmTemplate,
    planner_agent_id: &str,
    agent_ids: &[String],
    integrator_agent_id: Option<&str>,
    role_hints: &[(String, String)],
    priority_agent_ids: &[String],
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
    if matches!(template, SwarmTemplate::Lab | SwarmTemplate::Bulk) {
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
    if matches!(template, SwarmTemplate::Parallel | SwarmTemplate::Bulk) && !role_hints.is_empty() {
        out.push_str("- Agent role hints (from roster; 'all' means no constraint):\n");
        for (id, role) in role_hints.iter() {
            out.push_str(&format!("  - {id}: {role}\n"));
        }
        out.push_str(
            "- Prefer assigning tasks whose `role` matches each agent's hint (unless role=all).\n",
        );
    }
    if matches!(template, SwarmTemplate::Parallel | SwarmTemplate::Bulk)
        && !priority_agent_ids.is_empty()
    {
        out.push_str("- Priority agents (from roster):\n");
        for id in priority_agent_ids.iter() {
            out.push_str(&format!("  - {id}\n"));
        }
        out.push_str(
            "- When multiple assignments are viable, prefer priority agents for the most critical/high-impact work.\n",
        );
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
                "- If you assign producer/consumer-style roles (e.g. research or computational-research → judge), use deps to express required ordering.\n",
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
        SwarmTemplate::Bulk => {
            out.push_str(
                "- Bulk orchestration: explore multiple solution candidates in parallel, then converge.\n",
            );
            out.push_str(
                "- Prefer ONE proposer task per agent id (except the integrator), each with a distinct lens.\n",
            );
            out.push_str(
                "- Use ids `propose-01`, `propose-02`, ... plus `judge` and `integrate` so the workflow is easy to track.\n",
            );
            out.push_str(
                "- Create a judge task that depends on ALL proposer tasks and selects the best approach.\n",
            );
            out.push_str(
                "- Create an integrator task assigned to the integrator agent with `writes=true`, depending on the judge.\n",
            );
            out.push_str("- Use deps to express ordering (DAG). Avoid cycles.\n");
            out.push_str("- Only the integrator agent may have `writes=true` tasks.\n");
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
    out.push_str("      \"role\": \"(optional: propose|judge|research|computational-research|integrate|review|test)\",\n");
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

    if !task.artifacts.is_empty() {
        out.push_str("\nStructured artifacts:\n");
        out.push_str("Include a ```json block with:\n");
        out.push_str("{\"type\":\"swarm_artifacts\",\"version\":1,\"task_id\":\"");
        out.push_str(task.id.as_str());
        out.push_str("\",\"summary\":\"...\",\"artifacts\":{\"files\":[],\"diffs\":[],\"commands\":[],\"risks\":[],\"notes\":[]}}\n");
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
        if let Some(summary) = task_artifacts_summary_for_prompt(task, &run.mission_id) {
            out.push_str("ARTIFACTS:\n");
            out.push_str(summary.trim());
            out.push('\n');
        } else if task.expected_artifacts_missing {
            out.push_str("ARTIFACTS: expected but missing parseable swarm_artifacts JSON block\n");
        }
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
        for gate in dashboard_gate_rows(run).iter() {
            out.push_str(&format!(
                "- {}: {} ({})\n",
                gate.name, gate.status, gate.command
            ));
        }
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
    if let Some(first) = extract_json_code_blocks(text).into_iter().next() {
        return Some(first);
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

fn extract_json_code_blocks(text: &str) -> Vec<String> {
    let mut blocks = Vec::new();
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
        let candidate = buf.trim().trim_end_matches('`').trim().to_string();
        if !candidate.is_empty() {
            blocks.push(candidate);
        }
    }
    blocks
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
    out.push_str("{\"overall_ok\":true,\"gates\":[{\"name\":\"fmt\",\"command\":\"...\",\"ok\":true,\"status\":\"pass|fail|skip\",\"notes\":\"(optional)\"}]}\n");
    out.push_str(
        "\nImportant: The JSON must reflect the actual command outcomes (ok=true only when the command succeeded).\n",
    );
    out
}

fn parse_gate_report(message: &str) -> Option<GateReport> {
    for json in extract_json_code_blocks(message) {
        if let Ok(report) = serde_json::from_str::<GateReport>(&json) {
            return Some(report);
        }
    }
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

fn swarm_mission_title(root_prompt: &str, mission_id: &str, template: SwarmTemplate) -> String {
    let first = root_prompt.lines().next().unwrap_or("Swarm mission").trim();
    let label = template.label();
    if first.is_empty() {
        return format!("{mission_id} swarm[{label}]");
    }
    let mut title = String::new();
    for ch in first.chars().take(48) {
        title.push(ch);
    }
    format!("Swarm[{label}]: {title}")
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

fn abort_swarm_plan_preflight(state: &mut AppState, run: &mut SwarmRun, parsed: ParsedSwarmPlan) {
    if parsed.integrator_agent_id.is_some() {
        run.integrator_agent_id = parsed.integrator_agent_id;
    }
    run.tasks = parsed.tasks;
    run.synthesis_prompt = parsed.synthesis_prompt;
    run.stage = SwarmStage::Planning;

    let at = timestamp_label(state);
    if let Some(mission) = state
        .agents
        .missions
        .iter_mut()
        .find(|mission| mission.id == run.mission_id)
    {
        mission.status = "FAILED".into();
        mission.phase = MissionPhase::Plan;
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

pub fn select_swarm_agents(
    state: &AppState,
    planner: &str,
    size: SwarmSize,
    template: Option<&str>,
) -> Vec<String> {
    let template_kind = parse_swarm_template(template);
    let mut agents = vec![planner.to_string()];

    let roster_index = state
        .agents
        .agents
        .iter()
        .filter(|lane| !is_swarm_clone_agent_id(lane.id.as_str()))
        .enumerate()
        .map(|(idx, lane)| (lane.id.clone(), idx))
        .collect::<HashMap<_, _>>();

    let role_hint = |state: &AppState, agent_id: &str| -> String {
        state
            .agents
            .swarm_role_by_agent_id
            .get(agent_id)
            .map(|s| s.trim().to_ascii_lowercase())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "all".into())
    };

    let codex_pool = state
        .agents
        .agents
        .iter()
        .filter(|lane| lane.is_codex())
        .filter(|lane| lane.id.as_str() != planner)
        .filter(|lane| !is_swarm_clone_agent_id(lane.id.as_str()))
        .map(|lane| lane.id.clone())
        .collect::<Vec<_>>();
    if codex_pool.is_empty() {
        return agents;
    }

    let target = match size {
        SwarmSize::Default => DEFAULT_SWARM_SIZE,
        SwarmSize::All => usize::MAX,
        SwarmSize::Count(n) => n,
    }
    .clamp(1, MAX_SWARM_SIZE);
    let take = target.saturating_sub(1);
    if take == 0 {
        return agents;
    }

    #[derive(Clone)]
    struct Candidate {
        id: String,
        role_hint: String,
        priority: bool,
        busy: bool,
        roster_idx: usize,
    }

    let mut pool: Vec<Candidate> = codex_pool
        .into_iter()
        .map(|id| Candidate {
            roster_idx: *roster_index.get(&id).unwrap_or(&usize::MAX),
            busy: is_agent_busy(state, id.as_str()),
            priority: is_priority_agent(state, id.as_str()),
            role_hint: role_hint(state, id.as_str()),
            id,
        })
        .collect();

    let (mut priority_pool, mut fallback_pool): (Vec<Candidate>, Vec<Candidate>) =
        pool.drain(..).partition(|candidate| candidate.priority);

    let take_best =
        |pool: &mut Vec<Candidate>, target_role: &str, critical: bool| -> Option<Candidate> {
            if pool.is_empty() {
                return None;
            }
            pool.sort_by(|a, b| {
                let role_match = |candidate: &Candidate| -> u8 {
                    if candidate.role_hint.eq_ignore_ascii_case(target_role) {
                        0
                    } else if candidate.role_hint.eq_ignore_ascii_case("all") {
                        1
                    } else {
                        2
                    }
                };

                let priority_score = |candidate: &Candidate| -> u8 {
                    if candidate.priority {
                        0
                    } else if critical {
                        1
                    } else {
                        2
                    }
                };

                (
                    role_match(a),
                    priority_score(a),
                    a.busy as u8,
                    a.roster_idx,
                    &a.id,
                )
                    .cmp(&(
                        role_match(b),
                        priority_score(b),
                        b.busy as u8,
                        b.roster_idx,
                        &b.id,
                    ))
            });
            pool.first().cloned().map(|pick| {
                let pos = pool.iter().position(|cand| cand.id == pick.id).unwrap();
                pool.remove(pos)
            })
        };

    let mut selected: Vec<String> = Vec::new();

    if matches!(template_kind, SwarmTemplate::Parallel | SwarmTemplate::Bulk) {
        let has_priority_selection = !priority_pool.is_empty();
        if !has_priority_selection && !matches!(size, SwarmSize::All) {
            return agents;
        }
        let integrator = if !priority_pool.is_empty() {
            take_best(&mut priority_pool, "integrate", true)
        } else {
            take_best(&mut fallback_pool, "integrate", true)
        };
        if let Some(integrator) = integrator {
            selected.push(integrator.id);
        }

        let drain_parallel_pool =
            |pool: &mut Vec<Candidate>, selected: &mut Vec<String>, take: usize| {
                if pool.is_empty() {
                    return;
                }
                pool.sort_by(|a, b| {
                    let role_bucket = |candidate: &Candidate| -> u8 {
                        if candidate.role_hint.eq_ignore_ascii_case("all") {
                            1
                        } else {
                            0
                        }
                    };
                    (role_bucket(a), a.busy as u8, a.roster_idx, &a.id).cmp(&(
                        role_bucket(b),
                        b.busy as u8,
                        b.roster_idx,
                        &b.id,
                    ))
                });
                while selected.len() < take {
                    let Some(candidate) = pool.first().cloned() else {
                        break;
                    };
                    pool.remove(0);
                    selected.push(candidate.id);
                }
            };

        if has_priority_selection {
            drain_parallel_pool(&mut priority_pool, &mut selected, take);
        } else {
            drain_parallel_pool(&mut fallback_pool, &mut selected, take);
        }
    } else {
        while selected.len() < take {
            let candidate = if !priority_pool.is_empty() {
                take_best(&mut priority_pool, "all", false)
            } else {
                take_best(&mut fallback_pool, "all", false)
            };
            if let Some(candidate) = candidate {
                selected.push(candidate.id);
            } else {
                break;
            }
        }
    }

    agents.extend(selected);
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

fn is_priority_agent(state: &AppState, agent_id: &str) -> bool {
    if state.agents.swarm_priority_agent_ids.contains(agent_id) {
        return true;
    }
    if let Some(base_id) = swarm_clone_base_id(agent_id) {
        return state.agents.swarm_priority_agent_ids.contains(base_id);
    }
    false
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
    use nit_core::{AgentLane, AgentLaneKind, Buffer};
    use std::path::PathBuf;

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

        let cmd = parse_swarm_command("@swarm 6 template=bulk do thing").expect("cmd");
        assert_eq!(cmd.size, SwarmSize::Count(6));
        assert_eq!(cmd.template.as_deref(), Some("bulk"));
        assert_eq!(cmd.prompt, "do thing");
    }

    fn make_lane(id: &str, role: &str) -> AgentLane {
        AgentLane {
            id: id.into(),
            role: role.into(),
            lane: "Lane".into(),
            kind: AgentLaneKind::Codex,
            status: AgentStatus::Idle,
            heartbeat_age_secs: 0,
            queue_len: 0,
            current_mission: None,
            last_message: String::new(),
        }
    }

    #[test]
    fn swarm_clones_do_not_count_towards_swarm_size() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let editor = Buffer::empty("editor", None);
        let notes = Buffer::empty("notes", None);
        let mut state = AppState::new(root, editor, notes);
        state.agents.messages.clear();
        state.agents.agents.clear();

        state.agents.agents.push(make_lane("planner", "planner"));
        state.agents.agents.push(make_lane("a", "worker"));
        state.agents.agents.push(make_lane("b", "worker"));
        state.agents.agents.push(make_lane("c", "worker"));
        state
            .agents
            .swarm_role_by_agent_id
            .insert("a".into(), "integrate".into());
        state.agents.swarm_priority_agent_ids.insert("a".into());
        state.agents.swarm_priority_agent_ids.insert("b".into());
        state.agents.swarm_priority_agent_ids.insert("c".into());

        // These lanes are mission-scoped swarm clones and should never displace roster picks.
        state
            .agents
            .agents
            .push(make_lane("a#swarm-mis-000-propose-01", "worker"));
        state
            .agents
            .agents
            .push(make_lane("a#swarm-mis-000-judge", "worker"));
        state
            .agents
            .swarm_role_by_agent_id
            .insert("a#swarm-mis-000-propose-01".into(), "propose".into());
        state
            .agents
            .swarm_role_by_agent_id
            .insert("a#swarm-mis-000-judge".into(), "judge".into());

        let agents = select_swarm_agents(&state, "planner", SwarmSize::Count(4), Some("parallel"));

        assert_eq!(agents, vec!["planner", "a", "b", "c"]);
    }

    #[test]
    fn role_all_is_no_constraint_and_does_not_spawn_extra_agents() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let editor = Buffer::empty("editor", None);
        let notes = Buffer::empty("notes", None);
        let mut state = AppState::new(root, editor, notes);
        state.agents.messages.clear();
        state.agents.missions.clear();
        state.agents.agents.clear();

        state.agents.agents.push(make_lane("planner", "planner"));
        state.agents.agents.push(make_lane("a", "worker"));
        state
            .agents
            .swarm_role_by_agent_id
            .insert("a".into(), "all".into());

        let mut swarm = SwarmRuntime::default();
        let (mission_id, _dispatches) = swarm
            .start(
                &mut state,
                "planner".into(),
                vec!["planner".into(), "a".into()],
                SwarmSize::Count(2),
                Some("parallel".into()),
                "root".into(),
            )
            .expect("swarm start");

        assert_eq!(mission_id, "mis-001");
        let mission = state
            .agents
            .missions
            .iter()
            .find(|mission| mission.id == mission_id)
            .expect("mission");
        assert_eq!(mission.assigned_agents, vec!["planner", "a"]);
    }

    #[test]
    fn parallel_without_priorities_returns_planner_only() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let editor = Buffer::empty("editor", None);
        let notes = Buffer::empty("notes", None);
        let mut state = AppState::new(root, editor, notes);
        state.agents.agents.clear();

        state.agents.agents.push(make_lane("planner", "planner"));
        state.agents.agents.push(make_lane("a", "worker"));
        state.agents.agents.push(make_lane("b", "worker"));

        let agents = select_swarm_agents(&state, "planner", SwarmSize::Count(4), Some("parallel"));

        assert_eq!(agents, vec!["planner"]);
    }

    #[test]
    fn parallel_without_priorities_clones_planner_to_swarm_size() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let editor = Buffer::empty("editor", None);
        let notes = Buffer::empty("notes", None);
        let mut state = AppState::new(root, editor, notes);
        state.agents.messages.clear();
        state.agents.missions.clear();
        state.agents.agents.clear();

        state.agents.agents.push(make_lane("planner", "planner"));
        state.agents.agents.push(make_lane("a", "worker"));

        let mut swarm = SwarmRuntime::default();
        let (mission_id, _dispatches) = swarm
            .start(
                &mut state,
                "planner".into(),
                vec!["planner".into()],
                SwarmSize::Count(4),
                Some("parallel".into()),
                "root".into(),
            )
            .expect("swarm start");

        let mission = state
            .agents
            .missions
            .iter()
            .find(|mission| mission.id == mission_id)
            .expect("mission");
        assert_eq!(
            mission.assigned_agents,
            vec![
                "planner",
                "planner#swarm-mis-001-clone-01",
                "planner#swarm-mis-001-clone-02",
                "planner#swarm-mis-001-clone-03",
            ]
        );
    }

    #[test]
    fn parallel_priority_selection_clones_from_selected_models() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let editor = Buffer::empty("editor", None);
        let notes = Buffer::empty("notes", None);
        let mut state = AppState::new(root, editor, notes);
        state.agents.messages.clear();
        state.agents.missions.clear();
        state.agents.agents.clear();

        state.agents.agents.push(make_lane("planner", "planner"));
        state.agents.agents.push(make_lane("a", "worker"));
        state.agents.agents.push(make_lane("b", "worker"));
        state.agents.agents.push(make_lane("c", "worker"));
        state.agents.agents.push(make_lane("d", "worker"));

        state.agents.swarm_priority_agent_ids.insert("b".into());
        state.agents.swarm_priority_agent_ids.insert("d".into());

        let mut swarm = SwarmRuntime::default();
        let (mission_id, _dispatches) = swarm
            .start(
                &mut state,
                "planner".into(),
                vec!["planner".into(), "b".into(), "d".into()],
                SwarmSize::Count(4),
                Some("parallel".into()),
                "root".into(),
            )
            .expect("swarm start");

        let mission = state
            .agents
            .missions
            .iter()
            .find(|mission| mission.id == mission_id)
            .expect("mission");
        assert_eq!(
            mission.assigned_agents,
            vec!["planner", "b", "d", "b#swarm-mis-001-clone-01",]
        );
    }

    #[test]
    fn parallel_priority_agents_ranked_before_non_priority() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let editor = Buffer::empty("editor", None);
        let notes = Buffer::empty("notes", None);
        let mut state = AppState::new(root, editor, notes);
        state.agents.messages.clear();
        state.agents.agents.clear();

        state.agents.agents.push(make_lane("planner", "planner"));
        state.agents.agents.push(make_lane("a", "worker"));
        state.agents.agents.push(make_lane("b", "worker"));
        state.agents.agents.push(make_lane("c", "worker"));
        state.agents.agents.push(make_lane("d", "worker"));

        state.agents.swarm_priority_agent_ids.insert("b".into());
        state.agents.swarm_priority_agent_ids.insert("d".into());

        let agents = select_swarm_agents(&state, "planner", SwarmSize::Count(4), Some("parallel"));

        assert_eq!(agents, vec!["planner", "b", "d"]);
    }

    #[test]
    fn parallel_priority_ties_keep_priority_order() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let editor = Buffer::empty("editor", None);
        let notes = Buffer::empty("notes", None);
        let mut state = AppState::new(root, editor, notes);
        state.agents.agents.clear();

        for id in ["planner", "a", "b", "c"] {
            state.agents.agents.push(make_lane(id, "worker"));
            state
                .agents
                .swarm_role_by_agent_id
                .insert(id.into(), "all".into());
        }
        state.agents.swarm_priority_agent_ids.insert("a".into());
        state.agents.swarm_priority_agent_ids.insert("b".into());

        let agents = select_swarm_agents(&state, "planner", SwarmSize::Count(3), Some("parallel"));

        assert_eq!(agents, vec!["planner", "a", "b"]);
    }

    #[test]
    fn parallel_priority_overrides_role_hints() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let editor = Buffer::empty("editor", None);
        let notes = Buffer::empty("notes", None);
        let mut state = AppState::new(root, editor, notes);
        state.agents.agents.clear();

        state.agents.agents.push(make_lane("planner", "planner"));
        state.agents.agents.push(make_lane("a", "worker"));
        state.agents.agents.push(make_lane("b", "worker"));

        state.agents.swarm_priority_agent_ids.insert("a".into());
        state
            .agents
            .swarm_role_by_agent_id
            .insert("b".into(), "integrate".into());

        let agents = select_swarm_agents(&state, "planner", SwarmSize::Count(2), Some("parallel"));

        assert_eq!(agents, vec!["planner", "a"]);
    }

    #[test]
    fn bulk_integrator_prefers_priority_agents() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let editor = Buffer::empty("editor", None);
        let notes = Buffer::empty("notes", None);
        let mut state = AppState::new(root, editor, notes);
        state.agents.messages.clear();
        state.agents.missions.clear();
        state.agents.agents.clear();

        state.agents.agents.push(make_lane("planner", "planner"));
        state.agents.agents.push(make_lane("a", "worker"));
        state.agents.agents.push(make_lane("b", "worker"));

        state.agents.swarm_priority_agent_ids.insert("a".into());
        state
            .agents
            .swarm_role_by_agent_id
            .insert("b".into(), "integrate".into());

        let mut swarm = SwarmRuntime::default();
        let (mission_id, _dispatches) = swarm
            .start(
                &mut state,
                "planner".into(),
                vec!["planner".into(), "a".into(), "b".into()],
                SwarmSize::Count(3),
                Some("bulk".into()),
                "root".into(),
            )
            .expect("swarm start");

        let run = swarm.runs.get(&mission_id).expect("run");
        assert_eq!(run.integrator_agent_id.as_deref(), Some("a"));
        assert!(!run.integrator_locked);
    }

    #[test]
    fn bulk_priority_respects_role_hints() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let editor = Buffer::empty("editor", None);
        let notes = Buffer::empty("notes", None);
        let mut state = AppState::new(root, editor, notes);
        state.agents.agents.clear();

        state.agents.agents.push(make_lane("planner", "planner"));
        state.agents.agents.push(make_lane("a", "worker"));
        state.agents.agents.push(make_lane("b", "worker"));
        state.agents.agents.push(make_lane("c", "worker"));
        state.agents.agents.push(make_lane("d", "worker"));

        state
            .agents
            .swarm_role_by_agent_id
            .insert("a".into(), "all".into());
        state
            .agents
            .swarm_role_by_agent_id
            .insert("b".into(), "all".into());
        state
            .agents
            .swarm_role_by_agent_id
            .insert("c".into(), "propose".into());
        state
            .agents
            .swarm_role_by_agent_id
            .insert("d".into(), "propose".into());

        state.agents.swarm_priority_agent_ids.insert("b".into());
        state.agents.swarm_priority_agent_ids.insert("c".into());

        let agents = select_swarm_agents(&state, "planner", SwarmSize::Count(4), Some("bulk"));

        assert_eq!(agents, vec!["planner", "b", "c"]);
    }

    #[test]
    fn bulk_priority_agents_ranked_before_non_priority() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let editor = Buffer::empty("editor", None);
        let notes = Buffer::empty("notes", None);
        let mut state = AppState::new(root, editor, notes);
        state.agents.agents.clear();

        state.agents.agents.push(make_lane("planner", "planner"));
        state.agents.agents.push(make_lane("a", "worker"));
        state.agents.agents.push(make_lane("b", "worker"));
        state.agents.agents.push(make_lane("c", "worker"));
        state.agents.agents.push(make_lane("d", "worker"));

        state.agents.swarm_priority_agent_ids.insert("b".into());
        state.agents.swarm_priority_agent_ids.insert("d".into());

        let agents = select_swarm_agents(&state, "planner", SwarmSize::Count(4), Some("bulk"));

        assert_eq!(agents, vec!["planner", "b", "d"]);
    }

    fn make_task(id: &str, agent_id: &str, role: Option<&str>, deps: Vec<&str>) -> SwarmTask {
        SwarmTask {
            id: id.into(),
            agent_id: agent_id.into(),
            role: role.map(str::to_string),
            title: id.into(),
            task_prompt: "prompt".into(),
            deps: deps.into_iter().map(str::to_string).collect(),
            writes: false,
            artifacts: Vec::new(),
            done_when: None,
            state: SwarmTaskState::Pending,
            output: None,
            parsed_artifacts: None,
            expected_artifacts_missing: false,
            failed: false,
        }
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
            false,
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
    fn role_ordering_adds_research_before_judge() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let mut tasks = vec![
            make_task("research-1", "a1", Some("research"), Vec::new()),
            make_task("judge-1", "a2", Some("judge"), Vec::new()),
        ];

        let warnings = apply_role_dependency_ordering(root.as_path(), &HashMap::new(), &mut tasks);

        let judge = tasks.iter().find(|t| t.id == "judge-1").expect("judge");
        assert!(judge.deps.iter().any(|dep| dep == "research-1"));
        assert!(!warnings.is_empty());
    }

    #[test]
    fn role_ordering_uses_roster_hints_when_task_roles_missing() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let mut tasks = vec![
            make_task("t1", "a1", None, Vec::new()),
            make_task("t2", "a2", None, Vec::new()),
        ];

        let mut hints = HashMap::new();
        hints.insert("a1".into(), "research".into());
        hints.insert("a2".into(), "judge".into());

        apply_role_dependency_ordering(root.as_path(), &hints, &mut tasks);

        let t1 = tasks.iter().find(|t| t.id == "t1").expect("t1");
        let t2 = tasks.iter().find(|t| t.id == "t2").expect("t2");
        assert_eq!(t1.role.as_deref(), Some("research"));
        assert_eq!(t2.role.as_deref(), Some("judge"));
        assert!(t2.deps.iter().any(|dep| dep == "t1"));
    }

    #[test]
    fn role_ordering_does_not_introduce_cycles() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let mut tasks = vec![
            make_task("r", "a1", Some("research"), vec!["j"]),
            make_task("j", "a2", Some("judge"), Vec::new()),
        ];

        let warnings = apply_role_dependency_ordering(root.as_path(), &HashMap::new(), &mut tasks);

        let judge = tasks.iter().find(|t| t.id == "j").expect("judge");
        assert!(judge.deps.is_empty());
        assert!(warnings.iter().any(|w| w.contains("skipped")));
    }

    #[test]
    fn bulk_template_falls_back_when_planner_plan_is_not_bulk_shaped() {
        let planner_message = r#"
Plan:
- do stuff

```json
{
  "tasks": [
    { "agent_id": "a1", "title": "T1", "prompt": "x" }
  ]
}
```
"#;
        let available = vec!["a1".to_string(), "a2".to_string(), "a3".to_string()];
        let parsed = parse_plan_from_planner(
            planner_message,
            SwarmTemplate::Bulk,
            "root",
            &available,
            Some("a1"),
            false,
        );

        assert!(parsed
            .warnings
            .iter()
            .any(|w| w.contains("using built-in bulk workflow")));
        assert!(parsed.tasks.iter().any(|t| t.id.starts_with("propose-")));
        assert!(parsed.tasks.iter().any(|t| t.id == "judge"));
        assert!(parsed.tasks.iter().any(|t| t.id == "integrate" && t.writes));
    }

    #[test]
    fn bulk_template_normalizes_missing_deps_and_writes() {
        let planner_message = r#"
Plan:
- bulk

```json
{
  "version": 2,
  "template": "bulk",
  "integrator_agent_id": "a1",
  "tasks": [
    { "id": "propose-01", "agent_id": "a2", "role": "propose", "title": "Proposal", "prompt": "x", "deps": [], "writes": false },
    { "id": "judge", "agent_id": "a2", "role": "judge", "title": "Judge", "prompt": "y", "deps": [], "writes": false },
    { "id": "integrate", "agent_id": "a1", "role": "integrate", "title": "Integrate", "prompt": "z", "deps": [], "writes": false }
  ]
}
```
"#;
        let available = vec!["a1".to_string(), "a2".to_string()];
        let parsed = parse_plan_from_planner(
            planner_message,
            SwarmTemplate::Bulk,
            "root",
            &available,
            Some("a1"),
            false,
        );

        assert_eq!(parsed.tasks.len(), 3);
        let judge = parsed
            .tasks
            .iter()
            .find(|t| t.id == "judge")
            .expect("judge");
        assert!(judge.deps.iter().any(|dep| dep == "propose-01"));

        let integrate = parsed
            .tasks
            .iter()
            .find(|t| t.id == "integrate")
            .expect("integrate");
        assert!(integrate.writes);
        assert!(integrate.deps.iter().any(|dep| dep == "judge"));
    }

    #[test]
    fn bulk_template_infers_integrator_from_integrate_task_when_field_missing() {
        let planner_message = r#"
Plan:
- bulk

```json
{
  "version": 2,
  "template": "bulk",
  "tasks": [
    { "id": "propose-01", "agent_id": "a1", "role": "propose", "title": "Proposal", "prompt": "x", "deps": [], "writes": false },
    { "id": "judge", "agent_id": "a1", "role": "judge", "title": "Judge", "prompt": "y", "deps": ["propose-01"], "writes": false },
    { "id": "integrate", "agent_id": "a2", "role": "integrate", "title": "Integrate", "prompt": "z", "deps": ["judge"], "writes": true }
  ]
}
```
"#;
        let available = vec!["a1".to_string(), "a2".to_string()];
        let parsed = parse_plan_from_planner(
            planner_message,
            SwarmTemplate::Bulk,
            "root",
            &available,
            Some("a1"),
            false,
        );

        assert_eq!(parsed.integrator_agent_id.as_deref(), Some("a2"));
        assert!(parsed
            .warnings
            .iter()
            .any(|warning| warning.contains("inferred integrator 'a2'")));

        let integrate = parsed
            .tasks
            .iter()
            .find(|task| task.id == "integrate")
            .expect("integrate");
        assert!(integrate.writes);
    }

    #[test]
    fn dag_scheduler_dispatches_after_deps() {
        let mut run = SwarmRun {
            mission_id: "mis-001".into(),
            root_prompt: "root".into(),
            template: SwarmTemplate::Lab,
            planner_agent_id: "planner".into(),
            integrator_agent_id: Some("a1".into()),
            integrator_locked: false,
            verifier_agent_id: None,
            gate_bundle: None,
            gate_selection: "auto:none".into(),
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
                    parsed_artifacts: None,
                    expected_artifacts_missing: false,
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
                    parsed_artifacts: None,
                    expected_artifacts_missing: false,
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
                    parsed_artifacts: None,
                    expected_artifacts_missing: false,
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
                    parsed_artifacts: None,
                    expected_artifacts_missing: false,
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

        assert!(mark_task_finished(&mut run, "a2", "recon out".into(), false).is_some());
        assert!(mark_task_finished(&mut run, "a3", "design out".into(), false).is_some());
        refresh_task_readiness(&mut run);

        let second = dispatch_ready_tasks(&mut run);
        assert_eq!(second.len(), 1);
        assert_eq!(second[0].agent_id, "a1");

        assert!(mark_task_finished(&mut run, "a1", "impl out".into(), false).is_some());
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
            integrator_locked: false,
            verifier_agent_id: None,
            gate_bundle: None,
            gate_selection: "auto:none".into(),
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
                    parsed_artifacts: None,
                    expected_artifacts_missing: false,
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
                    parsed_artifacts: None,
                    expected_artifacts_missing: false,
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
                    parsed_artifacts: None,
                    expected_artifacts_missing: false,
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

        assert!(mark_task_finished(&mut run, "a1", "w1 out".into(), false).is_some());
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
            integrator_locked: false,
            verifier_agent_id: None,
            gate_bundle: None,
            gate_selection: "auto:none".into(),
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
                    parsed_artifacts: None,
                    expected_artifacts_missing: false,
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
                    parsed_artifacts: None,
                    expected_artifacts_missing: false,
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

        let deadlock = maybe_resolve_deadlock(&mut run).expect("deadlock");
        assert_eq!(deadlock.skipped.len(), 2);
        assert!(deadlock.message.contains("Swarm deadlock:"));
        assert!(run
            .tasks
            .iter()
            .all(|t| matches!(t.state, SwarmTaskState::Skipped)));
    }

    #[test]
    fn strict_dag_validation_aborts_before_execute() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let editor = Buffer::empty("editor", None);
        let notes = Buffer::empty("notes", None);
        let mut state = AppState::new(root, editor, notes);
        state.agents.messages.clear();
        state.agents.missions.clear();
        state.agents.agents.clear();

        state.agents.agents.push(AgentLane {
            id: "planner".into(),
            role: "Planner".into(),
            lane: "Lane A".into(),
            kind: AgentLaneKind::Codex,
            status: AgentStatus::Idle,
            heartbeat_age_secs: 0,
            queue_len: 0,
            current_mission: None,
            last_message: String::new(),
        });
        state.agents.agents.push(AgentLane {
            id: "a1".into(),
            role: "Integrator".into(),
            lane: "Lane B".into(),
            kind: AgentLaneKind::Codex,
            status: AgentStatus::Idle,
            heartbeat_age_secs: 0,
            queue_len: 0,
            current_mission: None,
            last_message: String::new(),
        });

        let mut swarm = SwarmRuntime::default();
        let (mission_id, _dispatches) = swarm
            .start(
                &mut state,
                "planner".into(),
                vec!["planner".into(), "a1".into()],
                SwarmSize::Count(2),
                Some("lab".into()),
                "root".into(),
            )
            .expect("swarm start");

        let planner_message = r#"
Plan:
- (test) introduce a deadlock cycle

```json
{
  "version": 2,
  "template": "lab",
  "integrator_agent_id": "a1",
  "tasks": [
    { "id": "t1", "agent_id": "a1", "title": "T1", "prompt": "DONE t1", "deps": ["t2"] },
    { "id": "t2", "agent_id": "a1", "title": "T2", "prompt": "DONE t2", "deps": ["t1"] }
  ]
}
```
"#;

        let event = AgentBusEvent::TurnCompleted {
            agent_id: "planner".into(),
            mission_id: Some(mission_id.clone()),
            thread_id: None,
            token_count: None,
            message: planner_message.into(),
        };
        event.apply(&mut state);
        let dispatches = swarm.handle_event(&mut state, &event);

        assert!(state.agents.messages.iter().any(|msg| {
            msg.mission_id.as_deref() == Some(mission_id.as_str())
                && msg.agent_id.as_deref() == Some("swarm")
                && msg.text.contains("PLAN error: invalid task DAG")
                && msg.text.contains("cycle:")
                && msg.text.contains("t1")
                && msg.text.contains("t2")
        }));

        assert!(dispatches.is_empty());
        assert!(!swarm.runs.contains_key(mission_id.as_str()));
        let run = swarm
            .completed_runs
            .get(mission_id.as_str())
            .expect("completed swarm run");
        assert!(matches!(run.stage, SwarmStage::Planning));
        assert!(run
            .tasks
            .iter()
            .all(|task| matches!(task.state, SwarmTaskState::Skipped)));
        let mission = state
            .agents
            .missions
            .iter()
            .find(|mission| mission.id == mission_id)
            .expect("mission");
        assert_eq!(mission.status, "FAILED");
        assert!(matches!(mission.phase, MissionPhase::Plan));
    }

    #[test]
    fn parse_task_artifacts_merges_json_blocks() {
        let message = r#"
notes
```json
{
  "type": "swarm_artifacts",
  "version": 1,
  "task_id": "design",
  "summary": "initial summary",
  "artifacts": {
    "files": [{"path": "crates/nit-tui/src/swarm.rs", "notes": "touches parser"}],
    "commands": [{"cmd": "cargo test --workspace"}]
  }
}
```
```json
{
  "type": "swarm_artifacts",
  "version": 1,
  "task_id": "design",
  "summary": "final summary",
  "artifacts": {
    "files": [{"path": "crates/nit-tui/src/swarm.rs", "notes": "duplicate"}],
    "risks": [{"level": "med", "item": "parser false positive"}],
    "notes": ["remember fallback"]
  }
}
```
"#;

        let artifacts = parse_task_artifacts("design", message).expect("artifacts");
        assert_eq!(artifacts.summary.as_deref(), Some("final summary"));
        assert_eq!(artifacts.files.len(), 1);
        assert_eq!(artifacts.commands.len(), 1);
        assert_eq!(artifacts.risks.len(), 1);
        assert_eq!(artifacts.notes, vec!["remember fallback".to_string()]);
    }

    #[test]
    fn parse_task_artifacts_tolerates_malformed_fence_suffix() {
        let message = r#"
```json
{"type":"swarm_artifacts","version":1,"task_id":"repo-recon","artifacts":{"notes":["ok"]}}``
"#;

        let artifacts = parse_task_artifacts("repo-recon", message).expect("artifacts");
        assert_eq!(artifacts.notes, vec!["ok".to_string()]);
    }

    #[test]
    fn dashboard_distinguishes_pending_queued_and_skipped() {
        let run = SwarmRun {
            mission_id: "mis-001".into(),
            root_prompt: "root".into(),
            template: SwarmTemplate::Lab,
            planner_agent_id: "planner".into(),
            integrator_agent_id: Some("a1".into()),
            integrator_locked: false,
            verifier_agent_id: Some("a2".into()),
            gate_bundle: Some(GateBundle::Rust),
            gate_selection: "auto:rust-ci(Cargo.toml)".into(),
            agent_ids: vec!["planner".into(), "a1".into(), "a2".into(), "a3".into()],
            stage: SwarmStage::Executing,
            tasks: vec![
                SwarmTask {
                    id: "done".into(),
                    agent_id: "a1".into(),
                    role: Some("integrate".into()),
                    title: "done".into(),
                    task_prompt: "done".into(),
                    deps: Vec::new(),
                    writes: true,
                    artifacts: Vec::new(),
                    done_when: None,
                    state: SwarmTaskState::Done,
                    output: Some("done".into()),
                    parsed_artifacts: None,
                    expected_artifacts_missing: false,
                    failed: false,
                },
                SwarmTask {
                    id: "ready".into(),
                    agent_id: "a2".into(),
                    role: Some("review".into()),
                    title: "ready".into(),
                    task_prompt: "ready".into(),
                    deps: Vec::new(),
                    writes: false,
                    artifacts: Vec::new(),
                    done_when: None,
                    state: SwarmTaskState::Ready,
                    output: None,
                    parsed_artifacts: None,
                    expected_artifacts_missing: false,
                    failed: false,
                },
                SwarmTask {
                    id: "blocked".into(),
                    agent_id: "a3".into(),
                    role: Some("review".into()),
                    title: "blocked".into(),
                    task_prompt: "blocked".into(),
                    deps: vec!["ready".into()],
                    writes: false,
                    artifacts: Vec::new(),
                    done_when: None,
                    state: SwarmTaskState::Pending,
                    output: None,
                    parsed_artifacts: None,
                    expected_artifacts_missing: false,
                    failed: false,
                },
                SwarmTask {
                    id: "skip".into(),
                    agent_id: "a3".into(),
                    role: Some("review".into()),
                    title: "skip".into(),
                    task_prompt: "skip".into(),
                    deps: vec!["unknown".into()],
                    writes: false,
                    artifacts: Vec::new(),
                    done_when: None,
                    state: SwarmTaskState::Skipped,
                    output: Some("SKIPPED".into()),
                    parsed_artifacts: None,
                    expected_artifacts_missing: false,
                    failed: true,
                },
            ],
            synthesis_prompt: None,
            gate_output: None,
            gate_report: Some(GateReport {
                overall_ok: false,
                gates: vec![GateReportGate {
                    name: "fmt".into(),
                    command: "cargo fmt --all -- --check".into(),
                    ok: false,
                    status: None,
                    notes: Some("formatting".into()),
                }],
            }),
        };
        let mut runtime = SwarmRuntime::default();
        runtime.runs.insert("mis-001".into(), run);

        let dashboard = runtime.swarm_dashboard("mis-001").expect("dashboard");
        assert_eq!(dashboard.pending, 1);
        assert_eq!(dashboard.queued, 1);
        assert_eq!(dashboard.skipped, 1);
        assert!(dashboard
            .tasks
            .iter()
            .any(|task| task.id == "blocked" && task.blocked_on == vec!["ready"]));
        assert!(dashboard
            .gates
            .iter()
            .any(|gate| gate.name == "fmt" && gate.status == "FAIL"));
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
