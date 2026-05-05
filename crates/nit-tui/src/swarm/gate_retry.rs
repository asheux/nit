use nit_core::{AppState, MissionPhase};

use super::mission::{update_mission_phase, update_mission_status};
use super::{
    derive_cargo_packages, extract_json_code_block, extract_json_code_blocks,
    push_system_message_to_mission, run_effective_gates, tasks_terminal_count, GateBundle,
    GateReport, GateReportGate, SwarmDispatch, SwarmRun, SwarmStage, SwarmTask, SwarmTaskState,
    NO_REVERT_CLAUSE,
};

pub(super) fn build_verify_prompt(run: &SwarmRun) -> String {
    let effective = run_effective_gates(run);
    let cargo_packages = derive_cargo_packages(&run.scope_files, run.spawn_cwd.as_path());
    let bundle_label = run
        .gate_custom
        .as_ref()
        .map(|_| "custom".to_string())
        .or_else(|| run.gate_bundle.as_ref().map(|b| b.label().to_string()))
        .unwrap_or_else(|| "(none)".to_string());

    let mut out = String::new();
    push_verify_preamble(&mut out, run, &bundle_label);
    push_verify_scope(&mut out, run, &cargo_packages);
    for gate in effective.iter() {
        out.push_str(&format!("- {}: `{}`\n", gate.name, gate.command));
    }
    push_verify_genome_section(&mut out, run);
    push_verify_schema(&mut out);
    out
}

fn push_verify_preamble(out: &mut String, run: &SwarmRun, bundle_label: &str) {
    out.push_str(
        "You are the SWARM VERIFIER. Run the verification gate bundle below against the current workspace.\n\n",
    );
    out.push_str("Rules:\n");
    out.push_str("- Run EXACTLY the commands listed below, in order. Do not substitute or broaden them (e.g. do not replace a scoped `-p <pkg>` command with `--workspace`).\n");
    out.push_str(
        "- If a gate fails, keep going when feasible (collect as much signal as possible).\n",
    );
    out.push_str("- Keep logs concise: include only the key error snippets needed to debug.\n");
    out.push_str("- Do NOT edit the workspace to fix issues you find — report them in the JSON `notes` field and let the operator / next integrator fix them.\n");
    out.push_str("- At the end, output a single JSON report in a ```json code block.\n");
    out.push_str("\nOperator request (context):\n");
    out.push_str(run.root_prompt.trim());
    out.push_str("\n\nGate bundle:\n");
    out.push_str(&format!("Bundle: {bundle_label}\n"));
}

// Cargo-specific scope wording is only meaningful for the Rust bundle —
// Node / Python / Go (or no bundle) lean on the rendered gate commands for
// their scope signal, so emitting "did not map to cargo packages" against
// them would just leak Rust framing into unrelated workspaces.
fn push_verify_scope(out: &mut String, run: &SwarmRun, cargo_packages: &[String]) {
    if !matches!(run.gate_bundle, Some(GateBundle::Rust)) {
        return;
    }
    if !cargo_packages.is_empty() {
        out.push_str(&format!(
            "Scope: cargo packages {} (derived from scope_files — only these packages were touched; do not widen to --workspace)\n",
            cargo_packages.join(", ")
        ));
    } else if !run.scope_files.is_empty() {
        out.push_str(
            "Scope: scope_files did not map to cargo packages — running full-workspace commands.\n",
        );
    } else {
        out.push_str("Scope: (no scope_files declared — running full-workspace commands)\n");
    }
}

fn push_verify_genome_section(out: &mut String, run: &SwarmRun) {
    let Some(genome_results) = run.genome_gate_results.as_deref() else {
        return;
    };
    out.push_str("\nGenome gate (pre-evaluated by nit):\n");
    out.push_str(genome_results);
    out.push_str(
        "\nInclude a gate entry for \"genome-quality\" with ok=true/false based on the results above.\n",
    );
}

fn push_verify_schema(out: &mut String) {
    out.push_str("\nReport schema:\n");
    out.push_str("{\"overall_ok\":true,\"gates\":[{\"name\":\"fmt\",\"command\":\"...\",\"ok\":true,\"status\":\"pass|fail|skip\",\"notes\":\"(optional)\"}]}\n");
    out.push_str(
        "\nImportant: The JSON must reflect the actual command outcomes (ok=true only when the command succeeded).\n",
    );
}

pub(super) fn parse_gate_report(message: &str) -> Option<GateReport> {
    for json in extract_json_code_blocks(message) {
        if let Ok(report) = serde_json::from_str::<GateReport>(&json) {
            return Some(report);
        }
    }
    let json = extract_json_code_block(message)?;
    serde_json::from_str::<GateReport>(&json).ok()
}

// Returns `None` to advance to `Synthesizing` (PASS, no integrator, retries
// exhausted, only-genome failures, or unparseable report). Otherwise builds
// the retry fix task, appends it, rolls stage back to `Executing`, and
// returns the integrator dispatch.
pub(super) fn try_dispatch_gate_retry(
    run: &mut SwarmRun,
    state: &mut AppState,
) -> Option<SwarmDispatch> {
    let limit = state.settings.swarm.gate_retry_limit;
    if limit == 0 {
        return None;
    }
    let report = run.gate_report.as_ref()?;
    if report.overall_ok {
        return None;
    }
    let integrator = run.integrator_agent_id.clone()?;

    if only_genome_failing(report) {
        push_system_message_to_mission(
            state,
            &run.mission_id,
            "Swarm verify: only genome-quality is failing (advisory). Not retrying — accepting the code as-is and proceeding.".into(),
        );
        return None;
    }

    if run.gate_retry_count >= limit {
        push_system_message_to_mission(
            state,
            &run.mission_id,
            format!(
                "Swarm verify FAILED after {} retry attempt(s); giving up and writing the report.",
                run.gate_retry_count,
            ),
        );
        return None;
    }

    let attempt = run.gate_retry_count + 1;
    let prompt = build_gate_retry_prompt(run, report, attempt, limit);
    let task_id = format!("gate-retry-{attempt}");
    enqueue_retry_task(
        run,
        integrator.clone(),
        task_id.clone(),
        prompt.clone(),
        attempt,
        limit,
    );

    push_system_message_to_mission(
        state,
        &run.mission_id,
        format!(
            "Swarm verify FAIL: dispatching fix task '{task_id}' to {integrator} (retry {attempt}/{limit})",
        ),
    );
    update_mission_phase(state, &run.mission_id, MissionPhase::Execute);
    update_mission_status(state, run, Some(tasks_terminal_count(&run.tasks)));

    Some(SwarmDispatch {
        agent_id: integrator,
        mission_id: run.mission_id.clone(),
        prompt,
        task_role: Some("integrate".into()),
    })
}

// Genome is a structural-quality signal, not a correctness gate. Repeated
// "try harder" dispatches waste budget and risk reverting real work to pass
// vacuously, so accept the degraded score and move on.
fn only_genome_failing(report: &GateReport) -> bool {
    let failing: Vec<&GateReportGate> = report
        .gates
        .iter()
        .filter(|gate| gate.ui_status() == "FAIL")
        .collect();
    !failing.is_empty()
        && failing.iter().all(|gate| {
            gate.name.eq_ignore_ascii_case("genome-quality")
                || gate.name.eq_ignore_ascii_case("genome")
        })
}

fn enqueue_retry_task(
    run: &mut SwarmRun,
    integrator: String,
    task_id: String,
    prompt: String,
    attempt: u8,
    limit: u8,
) {
    run.tasks.push(SwarmTask {
        id: task_id,
        agent_id: integrator,
        role: Some("integrate".into()),
        title: format!("Fix gate FAIL (retry {attempt}/{limit})"),
        task_prompt: prompt,
        deps: Vec::new(),
        writes: true,
        artifacts: Vec::new(),
        done_when: Some("Failing gates addressed; ready for verify re-run.".into()),
        state: SwarmTaskState::Dispatched,
        output: None,
        parsed_artifacts: None,
        expected_artifacts_missing: false,
        failed: false,
        retries: 0,
        compliance_missing_files: Vec::new(),
        shard_index: None,
    });
    run.gate_retry_count = attempt;
    run.gate_output = None;
    run.gate_report = None;
    // The integrator is about to change files, so any cached genome result
    // would describe a stale workspace — clear it.
    run.genome_gate_results = None;
    run.stage = SwarmStage::Executing;
}

pub(super) fn build_gate_retry_prompt(
    run: &SwarmRun,
    report: &GateReport,
    attempt: u8,
    limit: u8,
) -> String {
    let failing: Vec<&GateReportGate> = report
        .gates
        .iter()
        .filter(|gate| gate.ui_status() == "FAIL")
        .collect();

    let mut out = String::new();
    push_retry_header(&mut out, run, attempt, limit);
    push_failing_gates_section(&mut out, &failing);
    push_verifier_output_section(&mut out, run);
    out.push_str(
        "\nWhen done, reply briefly describing the edits you made — do not include a JSON report.\n",
    );
    out
}

fn push_retry_header(out: &mut String, run: &SwarmRun, attempt: u8, limit: u8) {
    out.push_str(&format!(
        "The swarm verify gate returned FAIL on attempt {attempt} of {limit}. Fix the failing gates below, then stop — the verifier will re-run automatically.\n\n",
    ));
    out.push_str("Rules:\n");
    out.push_str(
        "- You are the integrator. Apply the smallest workspace edits needed to make every failing gate pass.\n",
    );
    out.push_str(
        "- Do NOT broaden scope or refactor unrelated code. Only fix what the gates report.\n",
    );
    out.push_str(
        "- Do NOT run the verify commands yourself — the verifier agent will re-run them.\n",
    );
    out.push_str("- ");
    out.push_str(NO_REVERT_CLAUSE);
    out.push('\n');
    out.push_str(
        "- ADVISORY GATES (genome-quality): treat as best-effort. If you've made reasonable improvements and hit diminishing returns, STOP and report \"no further improvements possible\". Do NOT contort the code to chase a metric; the score is a signal, not a requirement.\n",
    );
    out.push_str(
        "- If a gate's failure cannot be fixed in code (e.g. missing tool, env issue), say so explicitly in your reply so the verifier can mark it SKIP.\n",
    );
    out.push_str("\nOperator request (context):\n");
    out.push_str(run.root_prompt.trim());
}

fn push_failing_gates_section(out: &mut String, failing: &[&GateReportGate]) {
    out.push_str("\n\nFailing gates:\n");
    if failing.is_empty() {
        out.push_str(
            "(report says overall_ok=false but no individual gate is FAIL — treat the verifier's notes as the failure signal.)\n",
        );
        return;
    }
    for gate in failing.iter() {
        out.push_str(&format!("- {} (`{}`)\n", gate.name, gate.command));
        if let Some(notes) = gate.notes.as_deref() {
            let trimmed = notes.trim();
            if !trimmed.is_empty() {
                out.push_str("  notes: ");
                out.push_str(&truncate_chars(trimmed, 1200));
                out.push('\n');
            }
        }
    }
}

fn push_verifier_output_section(out: &mut String, run: &SwarmRun) {
    let Some(raw) = run.gate_output.as_deref() else {
        return;
    };
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return;
    }
    out.push_str("\nVerifier raw output (truncated):\n");
    out.push_str(&truncate_chars(trimmed, 4000));
    out.push('\n');
}

pub(super) fn truncate_chars(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let clipped: String = text.chars().take(max_chars).collect();
    format!("{clipped}\n... (truncated)")
}
