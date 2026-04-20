use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::mpsc,
};

use nit_core::{AppState, GenomeReport};

use super::{normalize_role_label, read_workspace_gate_default, COMPUTATIONAL_RESEARCH_ROLE};

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum SwarmSize {
    Default,
    All,
    Count(usize),
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) enum SwarmTemplate {
    /// Parallel task splitting (v1-style): keep tasks independent and preferably one per agent.
    Parallel,
    /// "Lab" workflow: read-only analysis/proposal/review feeding a single-writer integrator.
    Lab,
    /// "Bulk orchestration": propose many candidate solutions in parallel, then converge via a
    /// judge step feeding a single-writer integrator.
    Bulk,
}

pub(super) fn parse_swarm_template(value: Option<&str>) -> SwarmTemplate {
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
    pub(super) fn label(&self) -> &'static str {
        match self {
            SwarmTemplate::Parallel => "parallel",
            SwarmTemplate::Lab => "lab",
            SwarmTemplate::Bulk => "bulk",
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum SwarmMissionKind {
    General,
    Research,
    ComputationalResearch,
}

impl SwarmMissionKind {
    pub(super) fn label(&self) -> &'static str {
        match self {
            SwarmMissionKind::General => "general",
            SwarmMissionKind::Research => "research",
            SwarmMissionKind::ComputationalResearch => COMPUTATIONAL_RESEARCH_ROLE,
        }
    }

    pub(super) fn allows_research_roles(&self) -> bool {
        !matches!(self, SwarmMissionKind::General)
    }

    pub(super) fn allows_role(&self, role: &str) -> bool {
        match normalize_role_label(role).as_deref() {
            Some("research") => matches!(
                self,
                SwarmMissionKind::Research | SwarmMissionKind::ComputationalResearch
            ),
            Some(COMPUTATIONAL_RESEARCH_ROLE) => {
                matches!(self, SwarmMissionKind::ComputationalResearch)
            }
            _ => true,
        }
    }
}

pub(crate) fn parse_swarm_mission_kind(value: Option<&str>) -> Option<SwarmMissionKind> {
    let value = value?.trim();
    if value.is_empty() {
        return None;
    }
    if value.eq_ignore_ascii_case("general")
        || value.eq_ignore_ascii_case("default")
        || value.eq_ignore_ascii_case("code")
        || value.eq_ignore_ascii_case("coding")
    {
        return Some(SwarmMissionKind::General);
    }
    if value.eq_ignore_ascii_case("research") {
        return Some(SwarmMissionKind::Research);
    }
    if value.eq_ignore_ascii_case("computational")
        || value.eq_ignore_ascii_case("computational-research")
        || value.eq_ignore_ascii_case("computational research")
        || value.eq_ignore_ascii_case("comp-research")
        || value.eq_ignore_ascii_case("comp_research")
    {
        return Some(SwarmMissionKind::ComputationalResearch);
    }
    None
}

pub(crate) fn explicit_swarm_mission_kind_from_prompt(
    root_prompt: &str,
) -> Option<SwarmMissionKind> {
    for line in root_prompt.lines() {
        let trimmed = line.trim().trim_start_matches(['-', '*', '•']).trim_start();
        if trimmed.is_empty() {
            continue;
        }
        let lower = trimmed.to_ascii_lowercase();
        let Some(rest) = lower.strip_prefix("mission:") else {
            continue;
        };
        let value = rest.trim();
        if value.is_empty() {
            continue;
        }
        let value = value
            .trim_matches(|ch: char| matches!(ch, '`' | '"' | '\''))
            .trim();
        let token = value
            .split_whitespace()
            .next()
            .unwrap_or_default()
            .trim_matches(|ch: char| matches!(ch, ',' | '.' | ';' | ')'));
        if let Some(kind) = parse_swarm_mission_kind(Some(token)) {
            return Some(kind);
        }
    }
    None
}

#[derive(Clone, Debug)]
pub struct SwarmDispatch {
    pub agent_id: String,
    pub mission_id: String,
    pub prompt: String,
    /// Task role (e.g. "review", "code") to apply to the agent lane on dispatch.
    pub task_role: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum SwarmArtifactFocus {
    Task { mission_id: String, task_id: String },
    Report { mission_id: String },
}

#[derive(Default)]
pub(crate) struct SwarmEventOutcome {
    pub dispatches: Vec<SwarmDispatch>,
    pub artifact_focus: Option<SwarmArtifactFocus>,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) enum SwarmStage {
    Planning,
    Executing,
    Verifying,
    Synthesizing,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum GateBundle {
    Rust,
    Node,
    Python,
    Go,
    Genome,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Gate {
    pub(super) name: String,
    /// Full command as a fallback when no scope information is available.
    /// Typically runs against the whole workspace (e.g. `cargo test --workspace`).
    pub(super) command: String,
    /// Optional scoped command template. When the swarm knows which cargo
    /// packages were touched (derived from the operator's scope_files), the
    /// verifier prompt renders this template with `{cargo_packages}` replaced
    /// by `-p pkg1 -p pkg2 ...`. Leave `None` to always run the full command.
    pub(super) scoped_command: Option<String>,
}

impl Gate {
    /// Build the command text to embed in the verifier prompt. When scoped
    /// execution is viable (we have cargo packages AND the gate has a scoped
    /// template), substitute placeholders; otherwise fall back to the full
    /// command.
    pub(super) fn rendered_command(&self, cargo_packages: &[String]) -> String {
        if !cargo_packages.is_empty() {
            if let Some(template) = self.scoped_command.as_deref() {
                let cargo_flags = cargo_packages
                    .iter()
                    .map(|pkg| format!("-p {pkg}"))
                    .collect::<Vec<_>>()
                    .join(" ");
                let packages_list = cargo_packages.join(" ");
                return template
                    .replace("{cargo_packages}", &cargo_flags)
                    .replace("{packages}", &packages_list);
            }
        }
        self.command.clone()
    }
}

impl GateBundle {
    pub(super) fn from_label(value: &str) -> Option<Self> {
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
        if value.eq_ignore_ascii_case("genome") || value.eq_ignore_ascii_case("genome-quality") {
            return Some(Self::Genome);
        }
        None
    }

    pub(super) fn detect(state: &AppState) -> GateBundleSelection {
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

    pub(super) fn label(&self) -> &'static str {
        match self {
            GateBundle::Rust => "rust-ci",
            GateBundle::Node => "node-ci",
            GateBundle::Python => "python-ci",
            GateBundle::Go => "go-ci",
            GateBundle::Genome => "genome",
        }
    }

    /// Default gate steps for this bundle. Rust gates include `scoped_command`
    /// templates so the verifier prompt can run `-p <pkg>` commands when the
    /// swarm's scope maps cleanly onto cargo packages. Other bundles currently
    /// only expose full-workspace commands — users who want scoped Node/Python/
    /// Go runs can provide custom gates via `.nit/config.toml`.
    pub(super) fn gates(&self) -> Vec<Gate> {
        match self {
            GateBundle::Rust => vec![
                Gate {
                    name: "fmt".into(),
                    command: "cargo fmt --all -- --check".into(),
                    scoped_command: Some("cargo fmt {cargo_packages} -- --check".into()),
                },
                Gate {
                    name: "clippy".into(),
                    command: "cargo clippy --all-targets --all-features -- -D warnings".into(),
                    scoped_command: Some(
                        "cargo clippy {cargo_packages} --all-targets --all-features -- -D warnings"
                            .into(),
                    ),
                },
                Gate {
                    name: "test".into(),
                    command: "cargo test --workspace --all-features".into(),
                    scoped_command: Some("cargo test {cargo_packages} --all-features".into()),
                },
            ],
            GateBundle::Node => vec![
                Gate {
                    name: "lint".into(),
                    command: "npm run lint --if-present".into(),
                    scoped_command: None,
                },
                Gate {
                    name: "build".into(),
                    command: "npm run build --if-present".into(),
                    scoped_command: None,
                },
                Gate {
                    name: "test".into(),
                    command: "npm test -- --watch=false --passWithNoTests".into(),
                    scoped_command: None,
                },
            ],
            GateBundle::Python => vec![
                Gate {
                    name: "ruff".into(),
                    command: "python -m ruff check .".into(),
                    scoped_command: None,
                },
                Gate {
                    name: "mypy".into(),
                    command: "python -m mypy .".into(),
                    scoped_command: None,
                },
                Gate {
                    name: "pytest".into(),
                    command: "python -m pytest -q".into(),
                    scoped_command: None,
                },
            ],
            GateBundle::Go => vec![
                Gate {
                    name: "fmt".into(),
                    command: "gofmt -l .".into(),
                    scoped_command: None,
                },
                Gate {
                    name: "vet".into(),
                    command: "go vet ./...".into(),
                    scoped_command: None,
                },
                Gate {
                    name: "test".into(),
                    command: "go test ./...".into(),
                    scoped_command: None,
                },
            ],
            GateBundle::Genome => vec![Gate {
                name: "genome-quality".into(),
                command: "(evaluated locally by nit)".into(),
                scoped_command: None,
            }],
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct GateBundleSelection {
    pub(super) bundle: Option<GateBundle>,
    pub(super) source: String,
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
    pub(super) fn ui_status(&self) -> &'static str {
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
pub(super) enum SwarmTaskState {
    Pending,
    Ready,
    Dispatched,
    Running,
    Done,
    Failed,
    Skipped,
}

impl SwarmTaskState {
    pub(super) fn is_terminal(self) -> bool {
        matches!(
            self,
            SwarmTaskState::Done | SwarmTaskState::Failed | SwarmTaskState::Skipped
        )
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) enum SwarmDagValidationMode {
    /// Reject plans with cycles/unknown deps (do not auto-repair).
    Strict,
    /// Attempt to make the graph runnable (drop unknown deps + break cycles) with warnings.
    Repair,
}

pub(super) const DEFAULT_DAG_VALIDATION_MODE: SwarmDagValidationMode =
    SwarmDagValidationMode::Strict;

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
    pub(super) fn is_empty(&self) -> bool {
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
    pub report_status: Option<String>,
    pub report_agent_id: Option<String>,
    pub report_output: Option<String>,
    pub tasks: Vec<SwarmTaskPersistenceView>,
}

#[derive(Clone, Debug)]
pub(super) struct SwarmTask {
    pub(super) id: String,
    pub(super) agent_id: String,
    pub(super) role: Option<String>,
    pub(super) title: String,
    pub(super) task_prompt: String,
    pub(super) deps: Vec<String>,
    pub(super) writes: bool,
    pub(super) artifacts: Vec<String>,
    pub(super) done_when: Option<String>,
    pub(super) state: SwarmTaskState,
    pub(super) output: Option<String>,
    pub(super) parsed_artifacts: Option<SwarmTaskArtifacts>,
    pub(super) expected_artifacts_missing: bool,
    pub(super) failed: bool,
    /// Number of times this task has been retried after failure.
    pub(super) retries: u8,
}

/// Holds the state for a genome gate evaluation running in a background thread.
/// When the evaluation completes, the result is received via `rx` and the
/// verifier dispatch proceeds — the verifier prompt reads the effective gate
/// list directly from the `SwarmRun`, so we only need the display label
/// ("rust-ci", "custom", etc.) for system-message logging.
pub(super) struct GenomeGatePending {
    pub(super) rx: mpsc::Receiver<String>,
    pub(super) label: String,
    pub(super) verifier: String,
}

/// Holds the state for a genome reviewer prompt being built in a background
/// thread. When the prompt is ready, the reviewer dispatch proceeds. An empty
/// prompt means the worker had nothing to evaluate (no modified files) and
/// the reviewer is silently skipped.
pub(super) struct GenomeReviewPending {
    pub(super) rx: mpsc::Receiver<String>,
    pub(super) reviewer_id: String,
}

pub(super) struct SwarmRun {
    pub(super) mission_id: String,
    pub(super) root_prompt: String,
    pub(super) template: SwarmTemplate,
    pub(super) mission_kind: SwarmMissionKind,
    pub(super) planner_agent_id: String,
    pub(super) integrator_agent_id: Option<String>,
    pub(super) integrator_locked: bool,
    pub(super) verifier_agent_id: Option<String>,
    pub(super) gate_bundle: Option<GateBundle>,
    /// Project-defined custom gates from `.nit/config.toml` (via
    /// `read_workspace_custom_gates`). When `Some`, these fully override the
    /// auto-detected `gate_bundle` — the downstream verify/dashboard code
    /// should iterate this list instead of `bundle.gates()`. Kept separate
    /// from `gate_bundle` so the UI source label can still show which
    /// language was detected and whether the user overrode it.
    pub(super) gate_custom: Option<Vec<Gate>>,
    pub(super) gate_selection: String,
    pub(super) agent_ids: Vec<String>,
    pub(super) stage: SwarmStage,
    pub(super) tasks: Vec<SwarmTask>,
    pub(super) synthesis_prompt: Option<String>,
    pub(super) gate_output: Option<String>,
    pub(super) gate_report: Option<GateReport>,
    pub(super) genome_gate_results: Option<String>,
    /// Background genome gate evaluation — `None` when idle, `Some` while
    /// waiting for the background thread to finish.
    pub(super) genome_gate_pending: Option<GenomeGatePending>,
    /// Background genome review prompt build — `None` when idle, `Some`
    /// while waiting for the worker to finish computing per-file genome
    /// reports for the reviewer agent.
    pub(super) genome_review_pending: Option<GenomeReviewPending>,
    pub(super) report_status: Option<String>,
    pub(super) report_output: Option<String>,
    /// Source files in the scope referenced by the operator prompt (e.g.
    /// `crates/nit-games`).  Populated at run creation; injected into
    /// integrate task prompts so agents cannot skip files.
    pub(super) scope_files: Vec<String>,
    /// Genome reports snapshot taken at swarm start, frozen for the life of
    /// the mission. Used as the "before" side of the final genome review so
    /// the reviewer sees real swarm-wide deltas.  The per-turn
    /// `state.genome_baselines` is unsuitable here because it gets cleared
    /// between agent turns and re-captured from post-edit state on the next
    /// `TurnStarted` — making every review show `+0.00` across all encoders.
    pub(super) initial_genome_baselines: HashMap<PathBuf, GenomeReport>,
    /// Number of swarm-level retries consumed after a gate FAIL. Capped by
    /// `settings.swarm.gate_retry_limit` (default 3). Each increment
    /// dispatches a fix task to the integrator and re-enters `Verifying`.
    pub(super) gate_retry_count: u8,
    /// Proposer genome pre-scan: scope-file paths still being evaluated by
    /// the genome worker. Populated on Executing transition; drained by
    /// `note_genome_prescan_result`. While non-empty, propose-role tasks
    /// are held at Ready without being dispatched — proposers wait until
    /// genome reports exist so their landscape-aware prompt is grounded in
    /// real data. Empty when the pre-scan is complete or was skipped (e.g.
    /// no scope files, genome context disabled).
    pub(super) prescan_pending: HashSet<PathBuf>,
    /// Paths already handed to the genome worker. A path is in
    /// `prescan_dispatched` from the moment we spawn its eval thread until
    /// the result lands (at which point both sets drop it). Without this
    /// guard the dispatcher re-queues the same paths every main-loop tick
    /// and floods the worker with thousands of threads.
    pub(super) prescan_dispatched: HashSet<PathBuf>,
    /// Whether the pre-scan pending set has been seeded from scope_files.
    /// Separate from `prescan_message_pushed` so we don't re-seed after the
    /// scan completes and empties the set.
    pub(super) prescan_seeded: bool,
    /// Whether a "proposer genome pre-scan" status message has been pushed
    /// for this run, so we don't spam the mission transcript.
    pub(super) prescan_message_pushed: bool,
}
