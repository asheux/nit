use std::collections::HashMap;

use super::{
    SwarmMissionKind, SwarmRun, SwarmRuntime, SwarmStage, SwarmTask, SwarmTaskState, SwarmTemplate,
};

/// Merge a single-mission fixture into an accumulator. Lets multipane tests
/// build a runtime where multiple missions are simultaneously active without
/// exposing the private `runs` field.
pub(crate) fn merge_single_mission_runtime(accumulator: &mut SwarmRuntime, single: SwarmRuntime) {
    for (k, v) in single.runs {
        accumulator.runs.insert(k, v);
    }
}

/// Build a `SwarmRuntime` fixture with one Running task per `(agent_id, role)`
/// pair AND one Dispatched (dashboard label "Queued") task per entry in
/// `queued`. Used by the breather regression test that reproduces a queued
/// task poisoning the role-uniformity check.
pub(crate) fn test_runtime_with_running_and_queued_tasks(
    mission_id: &str,
    running: &[(&str, &str)],
    queued: &[(&str, &str)],
) -> SwarmRuntime {
    let mut runtime = test_runtime_with_running_tasks(mission_id, running);
    if let Some(run) = runtime.runs.get_mut(mission_id) {
        let base = run.tasks.len();
        for (idx, (agent_id, role)) in queued.iter().enumerate() {
            run.tasks.push(make_task(
                base + idx,
                agent_id,
                role,
                SwarmTaskState::Dispatched,
            ));
            if !run.agent_ids.iter().any(|id| id == agent_id) {
                run.agent_ids.push(agent_id.to_string());
            }
        }
    }
    runtime
}

pub(crate) fn test_runtime_with_running_tasks(
    mission_id: &str,
    tasks: &[(&str, &str)],
) -> SwarmRuntime {
    test_runtime_with_running_tasks_and_template(mission_id, tasks, SwarmTemplate::Parallel)
}

/// Same as `test_runtime_with_running_tasks` but pins the mission kind —
/// used to verify the breather's mission-kind shortcut (research missions
/// label as "Researching ..." even when clones run mixed roles).
pub(crate) fn test_runtime_with_running_tasks_and_kind(
    mission_id: &str,
    tasks: &[(&str, &str)],
    mission_kind: SwarmMissionKind,
) -> SwarmRuntime {
    let mut runtime =
        test_runtime_with_running_tasks_and_template(mission_id, tasks, SwarmTemplate::Parallel);
    if let Some(run) = runtime.runs.get_mut(mission_id) {
        run.mission_kind = mission_kind;
    }
    runtime
}

/// Same as `test_runtime_with_running_tasks` but lets callers pin the
/// template — needed for tests that verify prompt parity across parallel /
/// lab / bulk templates.
pub(crate) fn test_runtime_with_running_tasks_and_template(
    mission_id: &str,
    tasks: &[(&str, &str)],
    template: SwarmTemplate,
) -> SwarmRuntime {
    let mut runtime = SwarmRuntime::default();
    let agent_ids: Vec<String> = tasks.iter().map(|(id, _)| id.to_string()).collect();
    let swarm_tasks: Vec<SwarmTask> = tasks
        .iter()
        .enumerate()
        .map(|(idx, (agent_id, role))| make_task(idx, agent_id, role, SwarmTaskState::Running))
        .collect();
    let run = SwarmRun {
        mission_id: mission_id.to_string(),
        root_prompt: String::new(),
        template,
        mission_kind: SwarmMissionKind::General,
        spawn_cwd: std::path::PathBuf::from("."),
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
        verifier_retry_budget: super::constants::VERIFIER_RETRY_BUDGET_DEFAULT,
        repair_round: 0,
        last_plan_json: None,
        prior_violations: Vec::new(),
        prompt_budget_defaults: runtime.prompt_budgets.clone(),
        prompt_budgets: HashMap::new(),
    };
    runtime.runs.insert(mission_id.to_string(), run);
    runtime
}

fn make_task(idx: usize, agent_id: &str, role: &str, state: SwarmTaskState) -> SwarmTask {
    let (id, title) = match state {
        SwarmTaskState::Dispatched => (format!("queued-{idx:02}"), format!("{role} task (queued)")),
        _ => (format!("task-{idx:02}"), format!("{role} task")),
    };
    SwarmTask {
        id,
        agent_id: agent_id.to_string(),
        role: Some(role.to_string()),
        title,
        task_prompt: String::new(),
        deps: Vec::new(),
        writes: false,
        artifacts: Vec::new(),
        done_when: None,
        state,
        output: None,
        parsed_artifacts: None,
        expected_artifacts_missing: false,
        failed: false,
        retries: 0,
        compliance_missing_files: Vec::new(),
        shard_index: None,
        pre_dispatch_file_state: std::collections::HashMap::new(),
    }
}
