use super::{
    Gate, GateBundle, SwarmGateDashboardRow, SwarmRun, SwarmStage, SwarmTask, SwarmTaskState,
};

pub(super) fn derive_cargo_packages(scope_files: &[String]) -> Vec<String> {
    if scope_files.is_empty() {
        return Vec::new();
    }
    let mut packages: Vec<String> = Vec::new();
    for path in scope_files {
        // Normalize separators for cross-platform path handling.
        let normalized = path.replace('\\', "/");
        let Some(rest) = normalized.strip_prefix("crates/") else {
            // File sits outside `crates/` — scope is mixed or unknown.
            return Vec::new();
        };
        let Some(pkg) = rest.split('/').next() else {
            return Vec::new();
        };
        if pkg.is_empty() {
            return Vec::new();
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

// Prefers the `"custom"` label when custom gates are configured; falls back to
// the detected language bundle (`"rust-ci"` / `"node-ci"` / …). `None` means
// no gates are active.
pub(super) fn run_gates_label(run: &SwarmRun) -> Option<String> {
    if run.gate_custom.is_some() {
        Some("custom".to_string())
    } else {
        run.gate_bundle.as_ref().map(|b| b.label().to_string())
    }
}

/// Resolve the effective gate list for a swarm run. Prefers project-defined
/// custom gates from `.nit/config.toml` (if any), otherwise falls back to the
/// auto-detected language bundle's default gates. Returns the gates as
/// already-rendered commands scoped to the run's cargo packages (when the
/// scope can be derived cleanly) or as full-workspace commands otherwise.
pub(super) fn run_effective_gates(run: &SwarmRun) -> Vec<Gate> {
    let cargo_packages = derive_cargo_packages(&run.scope_files);
    let base_gates = if let Some(custom) = run.gate_custom.as_ref() {
        custom.clone()
    } else if let Some(bundle) = run.gate_bundle.as_ref() {
        bundle.gates()
    } else {
        return Vec::new();
    };
    base_gates
        .into_iter()
        .map(|gate| {
            let rendered = gate.rendered_command(&cargo_packages);
            Gate {
                name: gate.name,
                command: rendered,
                scoped_command: None,
            }
        })
        .collect()
}

pub(super) fn dashboard_gate_rows(run: &SwarmRun) -> Vec<SwarmGateDashboardRow> {
    let mut rows = Vec::new();
    for gate in run_effective_gates(run) {
        rows.push(SwarmGateDashboardRow {
            name: gate.name,
            command: gate.command,
            status: "PENDING".into(),
            notes: None,
        });
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
