use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::mpsc,
};

use nit_core::GenomeReport;

use super::{normalize_role_label, read_workspace_gate_default, COMPUTATIONAL_RESEARCH_ROLE};

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum SwarmSize {
    Default,
    All,
    Count(usize),
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum SwarmTemplate {
    /// v1-style: keep tasks independent and preferably one per agent.
    Parallel,
    /// Read-only analysis/proposal/review feeding a single-writer integrator.
    Lab,
    /// Propose many candidate solutions in parallel, then converge via a
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
    /// Full command run when no cargo-package scope is known. Typically
    /// hits the whole workspace (e.g. `cargo test --workspace`).
    pub(super) command: String,
    /// Renders with `{cargo_packages}` → `-p pkg1 -p pkg2 ...` when the
    /// swarm knows which packages it touched. `None` = always full command.
    pub(super) scoped_command: Option<String>,
}

impl Gate {
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

/// Checks `Cargo.toml` directly at the spawn cwd — no walk-up — so a child
/// project nested under an unrelated Rust workspace does not inherit Rust
/// framing. Suppresses cargo-specific text on non-Rust workspaces.
pub(crate) fn is_cargo_workspace(cwd: &Path) -> bool {
    cwd.join("Cargo.toml").is_file()
}

// Bounds `GateBundle::detect`'s ancestor walk so a stray ancestor manifest
// cannot leak gates into an unrelated child. `.git` may be a file (worktree)
// or a dir (plain repo).
fn git_repo_root(cwd: &Path) -> Option<PathBuf> {
    let mut cursor = Some(cwd);
    while let Some(path) = cursor {
        if path.join(".git").exists() {
            return Some(path.to_path_buf());
        }
        cursor = path.parent();
    }
    None
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

    /// Walk is bounded at `cwd` or the surrounding git root (whichever is
    /// shallower) so a stray ancestor `Cargo.toml` cannot impose Rust gates
    /// on an unrelated child project.
    pub(super) fn detect(cwd: &Path) -> GateBundleSelection {
        let config_default = read_workspace_gate_default(cwd);
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

        let walk_root = git_repo_root(cwd).unwrap_or_else(|| cwd.to_path_buf());
        let mut detected = None;
        let mut cursor = Some(cwd);
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
            if path == walk_root.as_path() {
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

    /// Rust gates include `scoped_command` templates so the verifier can run
    /// `-p <pkg>` when the swarm's scope maps cleanly onto cargo packages.
    /// Other bundles only expose full-workspace commands — users who want
    /// scoped runs can provide custom gates via `.nit/config.toml`.
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
    /// Reject plans with cycles or unknown deps; do not auto-repair.
    Strict,
    /// Drop unknown deps and break cycles to make the graph runnable.
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
    pub(super) retries: u8,
    /// Files declared in proposer/judge artifacts that the previous turn
    /// failed to modify. Populated by the structural-compliance retry path
    /// so the continuation preamble can list them back to the integrator.
    /// Empty on first dispatch and after a turn that closed the gap.
    pub(super) compliance_missing_files: Vec<String>,
    /// `(shard_index_1based, shard_total)` when the runtime sharded a
    /// large-scope integrate task into N sequential pieces. None for normal
    /// non-sharded tasks. Used by dispatch to inject the shard's file slice
    /// and by the structural-compliance check to scope coverage per shard.
    pub(super) shard_index: Option<(u8, u8)>,
    /// Snapshot of (existed, line_count) for each declared file at the
    /// moment this write-role task is first dispatched. Empty for read-only
    /// tasks. Populated by `dispatch_ready_tasks`. Compared post-turn to
    /// detect stub-only file creations and incomplete splits — the runtime
    /// expects new declared files to be substantive (≥20 lines) and
    /// declared "huge" source files (>1500 lines) to shrink meaningfully
    /// when same-stem-dir siblings get created.
    pub(super) pre_dispatch_file_state: HashMap<String, FilePreState>,
}

/// Snapshot of a file's state at integrate-task dispatch time. Used by the
/// structural-compliance check to detect stub creations and incomplete
/// splits after the agent's turn finishes.
#[derive(Clone, Debug)]
pub(super) struct FilePreState {
    pub(super) existed: bool,
    pub(super) line_count: usize,
}

// `label` is the display string ("rust-ci", "custom", ...) used in system
// messages; the actual gate list is read from the `SwarmRun`.
pub(super) struct GenomeGatePending {
    pub(super) rx: mpsc::Receiver<String>,
    pub(super) label: String,
    pub(super) verifier: String,
}

// Empty prompt = worker had nothing to evaluate; reviewer is silently
// skipped in that case.
pub(super) struct GenomeReviewPending {
    pub(super) rx: mpsc::Receiver<String>,
    pub(super) reviewer_id: String,
}

pub(super) struct SwarmRun {
    pub(super) mission_id: String,
    pub(super) root_prompt: String,
    pub(super) template: SwarmTemplate,
    pub(super) mission_kind: SwarmMissionKind,
    /// Single-pane: `state.workspace_root`; multipane: the dispatching
    /// pane's cwd. Prompt builders consult this (not `state.workspace_root`)
    /// for cargo / language gating so a non-Rust pane never sees Rust
    /// framing even when the harness was launched from a Rust repo.
    pub(super) spawn_cwd: PathBuf,
    pub(super) planner_agent_id: String,
    pub(super) integrator_agent_id: Option<String>,
    pub(super) integrator_locked: bool,
    pub(super) verifier_agent_id: Option<String>,
    pub(super) gate_bundle: Option<GateBundle>,
    /// `.nit/config.toml` overrides — when `Some`, fully replace the
    /// auto-detected `gate_bundle` for verify/dashboard iteration. Kept
    /// separate from `gate_bundle` so the UI source label can still show
    /// which language was detected and whether the user overrode it.
    pub(super) gate_custom: Option<Vec<Gate>>,
    pub(super) gate_selection: String,
    pub(super) agent_ids: Vec<String>,
    pub(super) stage: SwarmStage,
    pub(super) tasks: Vec<SwarmTask>,
    pub(super) synthesis_prompt: Option<String>,
    pub(super) gate_output: Option<String>,
    pub(super) gate_report: Option<GateReport>,
    pub(super) genome_gate_results: Option<String>,
    pub(super) genome_gate_pending: Option<GenomeGatePending>,
    pub(super) genome_review_pending: Option<GenomeReviewPending>,
    pub(super) report_status: Option<String>,
    pub(super) report_output: Option<String>,
    /// Files in the scope referenced by the operator prompt. Populated at
    /// run creation; injected into integrate task prompts so agents cannot
    /// skip files.
    pub(super) scope_files: Vec<String>,
    /// Genome reports frozen at swarm start; the "before" side of the final
    /// review. Per-turn `state.genome_baselines` is unsuitable here — it
    /// gets cleared between agent turns and re-captured from post-edit
    /// state on the next `TurnStarted`, making every review show `+0.00`.
    pub(super) initial_genome_baselines: HashMap<PathBuf, GenomeReport>,
    /// Bumped per gate-FAIL retry; capped by `settings.swarm.gate_retry_limit`
    /// (default 3). Each increment dispatches a fix task to the integrator
    /// and re-enters `Verifying`.
    pub(super) gate_retry_count: u8,
}
