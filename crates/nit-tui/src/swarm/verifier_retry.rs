//! Auto-retry loop driven by verifier (test / review) findings.
//!
//! When a verifier-role task lands with `parsed_artifacts.findings`
//! non-empty AND the run still has retry budget, the runtime synthesises
//! ONE follow-up integrator turn scoped to the cited files with the
//! findings injected as the task description. This closes the gap
//! `gate_retry.rs` doesn't cover — gate retries fire on the post-execute
//! gate-bundle report, not on what a test/review agent itself observed.
//!
//! Bounded by [`super::constants::VERIFIER_RETRY_BUDGET_DEFAULT`] (= 1)
//! so a single retry pass happens automatically; further fixes are the
//! operator's call. The cost guardrail is intentional — runaway loops
//! on flaky tests or pedantic review remarks would burn agent tokens
//! with no human in the loop.

use std::collections::HashMap;

use nit_core::{AppState, MissionPhase};

use super::mission::{update_mission_phase, update_mission_status};
use super::{
    normalize_role_label, push_system_message_to_mission, tasks_terminal_count,
    SwarmArtifactFinding, SwarmDispatch, SwarmRun, SwarmStage, SwarmTask, SwarmTaskState,
};

/// Called from `handle_turn_completed` after a task lands. Returns
/// `Some(dispatch)` when the completed task is a verifier-role task
/// that emitted structured findings AND the run still has budget;
/// otherwise `None` (the normal verify→synthesis flow continues).
pub(super) fn try_dispatch_verifier_findings_retry(
    run: &mut SwarmRun,
    state: &mut AppState,
    completed_task_id: &str,
) -> Option<SwarmDispatch> {
    if run.verifier_retry_budget == 0 {
        return None;
    }
    let integrator = run.integrator_agent_id.clone()?;

    // Pull the just-completed task; only verifier roles trigger this loop.
    let task = run.tasks.iter().find(|t| t.id == completed_task_id)?;
    let role = task.role.as_deref().and_then(normalize_role_label)?;
    if !matches!(role.as_str(), "test" | "review") {
        return None;
    }

    let findings = task
        .parsed_artifacts
        .as_ref()
        .map(|a| a.findings.clone())
        .unwrap_or_default();
    if findings.is_empty() {
        return None;
    }

    // Defensive: ignore findings that lack a file. The parser already
    // filters these (see artifacts.rs::parse_artifact_findings), so this
    // is belt-and-suspenders — keeps the prompt focused on actionable
    // entries even if a custom parser path slipped one through.
    let actionable: Vec<&SwarmArtifactFinding> = findings
        .iter()
        .filter(|f| !f.file.trim().is_empty() && !f.issue.trim().is_empty())
        .collect();
    if actionable.is_empty() {
        return None;
    }

    let attempt = super::constants::VERIFIER_RETRY_BUDGET_DEFAULT - run.verifier_retry_budget + 1;
    let prompt = build_verifier_retry_prompt(run, &actionable, completed_task_id, attempt);
    let task_id = format!("verifier-retry-{completed_task_id}");

    enqueue_verifier_retry_task(run, integrator.clone(), task_id.clone(), prompt.clone());
    run.verifier_retry_budget = run.verifier_retry_budget.saturating_sub(1);

    push_system_message_to_mission(
        state,
        &run.mission_id,
        format!(
            "Swarm verify findings: {} actionable issue(s) reported by '{}'; dispatching fix task '{task_id}' to {integrator} (budget left: {})",
            actionable.len(),
            completed_task_id,
            run.verifier_retry_budget,
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

fn enqueue_verifier_retry_task(
    run: &mut SwarmRun,
    integrator: String,
    task_id: String,
    prompt: String,
) {
    run.tasks.push(SwarmTask {
        id: task_id,
        agent_id: integrator,
        role: Some("integrate".into()),
        title: "Fix verifier findings".into(),
        task_prompt: prompt,
        // No deps — verifier already ran; the integrator gets the
        // findings inline in the prompt so it doesn't need to wait on
        // any other task.
        deps: Vec::new(),
        writes: true,
        artifacts: Vec::new(),
        done_when: Some("Findings addressed; ready for verify re-run.".into()),
        state: SwarmTaskState::Dispatched,
        output: None,
        parsed_artifacts: None,
        expected_artifacts_missing: false,
        failed: false,
        retries: 0,
        compliance_missing_files: Vec::new(),
        shard_index: None,
        pre_dispatch_file_state: HashMap::new(),
    });
    // Roll the run stage back to Executing so the verify gate doesn't
    // fire prematurely against the not-yet-fixed code.
    run.stage = SwarmStage::Executing;
}

fn build_verifier_retry_prompt(
    run: &SwarmRun,
    findings: &[&SwarmArtifactFinding],
    source_task_id: &str,
    attempt: u8,
) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "Verifier '{source_task_id}' reported {} actionable finding(s) on the previous turn(s). This is an automatic fix dispatch (retry {attempt}). Address each finding below, then stop — the verifier will re-run automatically.\n\n",
        findings.len(),
    ));
    out.push_str("Rules:\n");
    out.push_str("- You are the integrator. Apply the smallest workspace edits needed to make every finding go away.\n");
    out.push_str(
        "- Do NOT broaden scope or refactor unrelated code. Only fix what the verifier flagged.\n",
    );
    out.push_str(
        "- Do NOT run the verify commands yourself — the verifier agent will re-run them.\n",
    );
    out.push_str("- If a finding seems wrong (the verifier misread the code), say so explicitly in your response and SKIP just that one finding; do not contort the code to silence a bogus complaint.\n\n");
    out.push_str("## OPERATOR REQUEST (unchanged, for context)\n");
    out.push_str(run.root_prompt.trim());
    out.push_str("\n\n");

    out.push_str("## FINDINGS TO FIX\n");
    for (i, finding) in findings.iter().enumerate() {
        out.push_str(&format!("\n### Finding {} — {}\n", i + 1, finding.file));
        if let Some(line) = finding.line {
            out.push_str(&format!("Line: {line}\n"));
        }
        if let Some(severity) = finding.severity.as_deref() {
            out.push_str(&format!("Severity: {severity}\n"));
        }
        if let Some(category) = finding.category.as_deref() {
            out.push_str(&format!("Category: {category}\n"));
        }
        out.push_str(&format!("Issue: {}\n", finding.issue));
        if let Some(suggestion) = finding.suggestion.as_deref() {
            out.push_str(&format!("Suggested fix: {suggestion}\n"));
        }
    }
    out.push_str(
        "\nWhen done, reply briefly describing the edits you made — do not include a swarm_artifacts JSON block (this is a fix turn, not a verification turn).\n",
    );
    out
}
