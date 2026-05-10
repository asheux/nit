use std::path::Path;

use nit_core::AppState;

use super::{
    is_cargo_workspace, Gate, GateBundle, SwarmGateDashboardRow, SwarmRun, SwarmStage, SwarmTask,
    SwarmTaskState,
};

/// Source of truth for "which files does this mission care about, *now*".
///
/// `run.scope_files` is a spawn-time prediction (from prompt path tokens or a
/// `git diff` against the merge-base). It's frozen the moment the swarm
/// starts, so it can't see anything the agents actually modify. For prompts
/// like "fix the regression" against a clean tree it's empty, which then
/// collapses gate commands to `cargo test --workspace`.
///
/// Once tasks start writing, `state.genome_mission_modified[mission_id]`
/// holds the real per-mission write set (populated in
/// `claude_runner` / `codex_runner` via the agent bus). Prefer that as soon
/// as it has content; fall back to the prediction only when nothing has been
/// written yet (e.g., during `Planning` or before any task finalises).
pub(super) fn effective_scope_files(state: &AppState, run: &SwarmRun) -> Vec<String> {
    let modified = state.genome_mission_modified.get(&run.mission_id);
    let Some(modified) = modified.filter(|set| !set.is_empty()) else {
        return run.scope_files.clone();
    };
    let spawn_cwd = run.spawn_cwd.as_path();
    let mut rel: Vec<String> = modified
        .iter()
        .map(|abs| {
            abs.strip_prefix(spawn_cwd)
                .unwrap_or(abs.as_path())
                .display()
                .to_string()
        })
        .collect();
    rel.sort();
    rel.dedup();
    rel
}

pub(super) fn derive_cargo_packages(scope_files: &[String], spawn_cwd: &Path) -> Vec<String> {
    if scope_files.is_empty() || !is_cargo_workspace(spawn_cwd) {
        return Vec::new();
    }
    // Filter (don't bail) on non-`crates/` paths. The earlier "any miss → bail
    // to --workspace" rule was too pessimistic: a swarm touching `crates/X/...`
    // alongside a root-level file like `Cargo.lock` or `CLAUDE.md` would lose
    // *all* scoping, even though the cargo packages it touched are perfectly
    // identifiable. Drop paths we can't map and keep the ones we can; only
    // bail when nothing maps.
    let mut packages: Vec<String> = Vec::new();
    for path in scope_files {
        let normalized = path.replace('\\', "/");
        let Some(rest) = normalized.strip_prefix("crates/") else {
            continue;
        };
        let Some(pkg) = rest.split('/').next() else {
            continue;
        };
        if pkg.is_empty() {
            continue;
        }
        let pkg = pkg.to_string();
        if !packages.contains(&pkg) {
            packages.push(pkg);
        }
    }
    packages
}

pub(super) fn blocked_on(run: &SwarmRun, task: &SwarmTask) -> Vec<String> {
    task.deps
        .iter()
        .filter_map(|dep_id| {
            let dep = run.tasks.iter().find(|candidate| candidate.id == *dep_id)?;
            (!dep.state.is_terminal()).then(|| dep.id.clone())
        })
        .collect()
}

pub(super) fn task_state_dashboard_label(state: SwarmTaskState) -> &'static str {
    match state {
        SwarmTaskState::Pending => "Pending",
        SwarmTaskState::Ready | SwarmTaskState::Dispatched => "Queued",
        SwarmTaskState::Running => "Running",
        SwarmTaskState::Done => "Done",
        SwarmTaskState::Failed => "Failed",
        SwarmTaskState::Skipped => "Skipped",
    }
}

pub(super) fn stage_label(stage: SwarmStage) -> &'static str {
    match stage {
        SwarmStage::Planning => "PLAN",
        SwarmStage::Executing => "EXEC",
        SwarmStage::Verifying => "VERIFY",
        SwarmStage::Synthesizing => "SYNTH",
    }
}

// `"custom"` wins when explicit gates are configured; otherwise fall back to
// the detected language bundle. `None` means no gates are active.
pub(super) fn run_gates_label(run: &SwarmRun) -> Option<String> {
    if run.gate_custom.is_some() {
        Some("custom".to_string())
    } else {
        run.gate_bundle.as_ref().map(|b| b.label().to_string())
    }
}

// Custom gates from `.nit/config.toml` win over the auto-detected language
// bundle's default gates. The result is rendered against the run's cargo
// packages so each command is correctly scoped (or full-workspace when the
// scope can't be derived cleanly).
//
// Cargo-specific scoping kicks in only on Rust workspaces (`is_cargo_workspace`
// is the gate inside `derive_cargo_packages`). Node / Python / Go bundles
// have `scoped_command = None` on their `Gate` entries, so `rendered_command`
// returns the unscoped form regardless of what `effective_scope_files` finds —
// language-agnostic by construction.
pub(super) fn run_effective_gates(state: &AppState, run: &SwarmRun) -> Vec<Gate> {
    let scope = effective_scope_files(state, run);
    let cargo_packages = derive_cargo_packages(&scope, run.spawn_cwd.as_path());
    let base_gates = if let Some(custom) = run.gate_custom.as_ref() {
        custom.clone()
    } else if let Some(bundle) = run.gate_bundle.as_ref() {
        bundle.gates()
    } else {
        return Vec::new();
    };
    base_gates
        .into_iter()
        .map(|gate| Gate {
            command: gate.rendered_command(&cargo_packages),
            name: gate.name,
            scoped_command: None,
        })
        .collect()
}

pub(super) fn dashboard_gate_rows(state: &AppState, run: &SwarmRun) -> Vec<SwarmGateDashboardRow> {
    let mut rows: Vec<SwarmGateDashboardRow> = run_effective_gates(state, run)
        .into_iter()
        .map(|gate| SwarmGateDashboardRow {
            name: gate.name,
            command: gate.command,
            status: "PENDING".into(),
            notes: None,
        })
        .collect();
    let Some(report) = run.gate_report.as_ref() else {
        return rows;
    };
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
    rows
}

pub(super) fn gate_bundle_label(bundle: Option<&GateBundle>, source: &str) -> String {
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
