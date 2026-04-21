use std::collections::HashMap;

use super::{
    SwarmMissionKind, SwarmRun, SwarmRuntime, SwarmStage, SwarmTask, SwarmTaskState, SwarmTemplate,
};

/// Test helper: build a `SwarmRuntime` fixture with one Running task per
/// `(agent_id, role)` pair *and* one Dispatched (dashboard label "Queued")
/// task per entry in `queued`. Used by the breather regression test that
/// reproduces a queued task poisoning the role-uniformity check.
pub(crate) fn test_runtime_with_running_and_queued_tasks(
    mission_id: &str,
    running: &[(&str, &str)],
    queued: &[(&str, &str)],
) -> SwarmRuntime {
    let mut runtime = test_runtime_with_running_tasks(mission_id, running);
    if let Some(run) = runtime.runs.get_mut(mission_id) {
        let base = run.tasks.len();
        for (idx, (agent_id, role)) in queued.iter().enumerate() {
            run.tasks.push(SwarmTask {
                id: format!("queued-{:02}", base + idx),
                agent_id: agent_id.to_string(),
                role: Some(role.to_string()),
                title: format!("{role} task (queued)"),
                task_prompt: String::new(),
                deps: Vec::new(),
                writes: false,
                artifacts: Vec::new(),
                done_when: None,
                state: SwarmTaskState::Dispatched,
                output: None,
                parsed_artifacts: None,
                expected_artifacts_missing: false,
                failed: false,
                retries: 0,
            });
            if !run.agent_ids.iter().any(|id| id == agent_id) {
                run.agent_ids.push(agent_id.to_string());
            }
        }
    }
    runtime
}

/// Test helper: build a `SwarmRuntime` fixture with one running task per
/// `(agent_id, role)` pair. Used by `agent_console_view` breather tests to
/// exercise role-aware labels without going through the full planner flow.
pub(crate) fn test_runtime_with_running_tasks(
    mission_id: &str,
    tasks: &[(&str, &str)],
) -> SwarmRuntime {
    let template = SwarmTemplate::Parallel;
    let mut runtime = SwarmRuntime::default();
    let agent_ids: Vec<String> = tasks.iter().map(|(id, _)| id.to_string()).collect();
    let swarm_tasks: Vec<SwarmTask> = tasks
        .iter()
        .enumerate()
        .map(|(idx, (agent_id, role))| SwarmTask {
            id: format!("task-{idx:02}"),
            agent_id: agent_id.to_string(),
            role: Some(role.to_string()),
            title: format!("{role} task"),
            task_prompt: String::new(),
            deps: Vec::new(),
            writes: false,
            artifacts: Vec::new(),
            done_when: None,
            state: SwarmTaskState::Running,
            output: None,
            parsed_artifacts: None,
            expected_artifacts_missing: false,
            failed: false,
            retries: 0,
        })
        .collect();
    let run = SwarmRun {
        mission_id: mission_id.to_string(),
        root_prompt: String::new(),
        template,
        mission_kind: SwarmMissionKind::General,
        planner_agent_id: agent_ids
            .first()
            .cloned()
            .unwrap_or_else(|| "planner".into()),
        integrator_agent_id: None,
        integrator_locked: false,
        verifier_agent_id: None,
        gate_bundle: None,
        gate_custom: None,
        gate_selection: "auto:none".into(),
        agent_ids,
        stage: SwarmStage::Executing,
        tasks: swarm_tasks,
        synthesis_prompt: None,
        gate_output: None,
        gate_report: None,
        genome_gate_results: None,
        genome_gate_pending: None,
        genome_review_pending: None,
        report_status: None,
        report_output: None,
        scope_files: Vec::new(),
        initial_genome_baselines: HashMap::new(),
        gate_retry_count: 0,
    };
    runtime.runs.insert(mission_id.to_string(), run);
    runtime
}
