use std::collections::HashMap;

use nit_core::AppState;
#[cfg(test)]
use nit_core::{AgentBusEvent, AgentStatus, MissionPhase};

#[derive(Default)]
pub struct SwarmRuntime {
    runs: HashMap<String, SwarmRun>,
    completed_runs: HashMap<String, SwarmRun>,
}

/// Configuration from a previous swarm run, used to re-launch follow-up prompts
/// with the same template, size, and planner.
pub struct SwarmSessionConfig {
    pub template: String,
    pub size: usize,
    pub planner_agent_id: String,
}

/// Re-create swarm clones for a follow-up dispatch within an existing mission.
/// Returns the full list of agent IDs (planner + clones) ready for dispatch.
pub fn ensure_swarm_agents_for_followup(
    state: &mut AppState,
    mission_id: &str,
    config: &SwarmSessionConfig,
) -> Vec<String> {
    let template = parse_swarm_template(Some(config.template.as_str()));
    let size = SwarmSize::Count(config.size);
    let mut agents = vec![config.planner_agent_id.clone()];
    ensure_size_clones(
        state,
        mission_id,
        template,
        size,
        &config.planner_agent_id,
        &mut agents,
    );
    // Re-apply parallel-template clone role coverage so follow-up dispatches
    // see the same role assignments as the original mission. No-op when the
    // planner is `all`/unset or coverage is already satisfied (most common
    // case for follow-ups since the original setup already assigned hints).
    let _ = assign_clone_roles_for_parallel_coverage(
        state,
        template,
        &config.planner_agent_id,
        None,
        &agents,
    );
    // Update the mission's assigned_agents so broadcast_target_agents can find them.
    if let Some(mission) = state
        .agents
        .missions
        .iter_mut()
        .find(|m| m.id == mission_id)
    {
        mission.assigned_agents = agents.clone();
    }
    agents
}

mod artifacts;
mod bulk_plan;
mod clones;
mod command;
mod config;
mod constants;
mod dag;
mod dashboard;
mod fallback;
mod gate_retry;
mod graph_exec;
mod json;
mod mission;
mod plan_parser;
mod prompts;
mod runtime;
mod runtime_events;
mod scope;
mod signals;
mod types;
mod workers;

use artifacts::{
    dependency_payload_text, dependency_payload_text_full, merge_task_artifacts,
    parse_task_artifacts, task_artifacts_summary_for_prompt,
};
use bulk_plan::{
    apply_lab_lenses, ensure_agent_coverage, ensure_integrate_task,
    ensure_judge_task_for_multi_proposer, ensure_proposer_task, normalize_bulk_plan,
    normalize_lab_plan, validate_bulk_plan, ParsedSwarmPlan, SwarmPlanTaskV2, SwarmPlanV1,
    SwarmPlanV2,
};
pub(crate) use clones::drain_queued_turns_for_agent as drain_queued_turns_for_agent_pub;
pub use clones::{
    chat_clone_base_id, cleanup_idle_chat_clone, compact_agent_display_id, create_chat_clone,
    is_any_clone_agent_id, is_chat_clone_agent_id, SWARM_CLONE_INFIX,
};
use clones::{
    cleanup_swarm_clones_for_mission, drain_queued_turns_for_agent, ensure_size_clones,
    is_swarm_clone_agent_id, swarm_clone_base_id,
};
pub(crate) use clones::{
    copy_claude_runtime_metadata, copy_codex_runtime_metadata, insert_swarm_clone_lane,
};
pub use command::{parse_swarm_command, SwarmCommand};
use config::{
    read_workspace_custom_gates, read_workspace_dag_validation_mode, read_workspace_gate_default,
};
use constants::{
    COMPUTATIONAL_RESEARCH_ROLE, COMPUTATIONAL_RESEARCH_ROLE_LEGACY, MAX_SWARM_SIZE,
    SWARM_DEP_OUTPUT_MAX_CHARS, SWARM_DEP_OUTPUT_MAX_CHARS_FULL,
    SWARM_DEP_OUTPUT_TOTAL_MAX_CHARS_FULL, SWARM_VERIFY_MAX_CHARS,
};
pub(crate) use constants::{
    DEFAULT_SWARM_SIZE, LARGE_SWARM_WARN_THRESHOLD, NO_PADDING_CLAUSE, NO_REVERT_CLAUSE,
    SWARM_DEP_OUTPUT_MAX_CHARS_FULL as DEP_BUDGET_PER_DEP_CEILING, TEST_DISCIPLINE_CLAUSE,
};

mod limits;
use dag::{analyze_swarm_dag, ensure_deps_resolve, find_swarm_cycle_path, repair_swarm_dag};
use dashboard::{
    blocked_on, dashboard_gate_rows, derive_cargo_packages, gate_bundle_label, run_effective_gates,
    run_gates_label, stage_label, task_state_dashboard_label,
};
use fallback::fallback_tasks;
use gate_retry::{build_verify_prompt, parse_gate_report, truncate_chars, try_dispatch_gate_retry};
use graph_exec::{
    dispatch_ready_tasks, initialize_task_graph, mark_task_finished, mark_task_running,
    maybe_resolve_deadlock, refresh_task_readiness, structural_compliance_missing_files,
    tasks_terminal_count,
};
pub(crate) use graph_exec::{per_dep_budget, task_uses_full_output_budget};
use json::{extract_json_code_block, extract_json_code_blocks};
pub(crate) use limits::{
    current_fd_soft_limit, effective_max_swarm_size, is_light_planner, large_swarm_warn_threshold,
    BULK_PRACTICAL_MAX, LIGHT_PLANNER_SWARM_THRESHOLD,
};
use plan_parser::{
    apply_role_dependency_ordering, assign_clone_roles_for_parallel_coverage,
    classify_swarm_mission_kind, deduplicate_inherited_role_hints, direct_role_hint_for_agent,
    parse_plan_from_planner, planner_role_hint_for_agent,
};
pub(crate) use plan_parser::{detect_swarm_mission_kind_from_prompt, normalize_role_label};
pub(crate) use prompts::role_contract_lines;
use prompts::{
    build_planner_prompt, build_synthesis_prompt, detect_incomplete_signoff,
    is_provider_rate_limit_failure, wrap_task_prompt,
};
pub(crate) use scope::enumerate_scope_files;
use scope::sanitize_for_filename;
#[cfg(test)]
use signals::collect_unresolved_deps;
use signals::{emit_parallel_deps_auto_repair_signals, emit_unresolved_dep_signals};
pub(crate) use types::{
    explicit_swarm_mission_kind_from_prompt, parse_swarm_mission_kind, SwarmArtifactFocus,
    SwarmEventOutcome,
};
use types::{
    parse_swarm_template, Gate, GateBundle, GenomeGatePending, GenomeReviewPending,
    SwarmDagValidationMode, SwarmRun, SwarmStage, SwarmTask, SwarmTaskState, SwarmTemplate,
    DEFAULT_DAG_VALIDATION_MODE,
};
pub use types::{
    GateReport, GateReportGate, SwarmArtifactCommand, SwarmArtifactDiff, SwarmArtifactFile,
    SwarmArtifactRisk, SwarmDashboardView, SwarmDispatch, SwarmGateDashboardRow, SwarmMissionKind,
    SwarmPersistenceView, SwarmSize, SwarmTaskArtifacts, SwarmTaskDashboardRow,
    SwarmTaskPersistenceView,
};
use workers::{maybe_spawn_genome_review, spawn_genome_gate_eval};

use mission::{
    abort_swarm_plan_preflight, is_priority_agent, next_mission_id, swarm_mission_title,
    tag_last_agent_message_kind, timestamp_label, update_mission_final, update_mission_phase,
    update_mission_status,
};
pub use mission::{
    is_agent_busy, is_agent_family_busy, push_system_alert_to_mission,
    push_system_message_to_mission, resolve_base_agent_id, select_swarm_agents,
    swarm_intended_size, SYSTEM_ALERT_KIND,
};

#[cfg(test)]
mod test_fixtures;

#[cfg(test)]
pub(crate) use test_fixtures::{
    merge_single_mission_runtime, test_runtime_with_running_and_queued_tasks,
    test_runtime_with_running_tasks, test_runtime_with_running_tasks_and_template,
};
#[cfg(test)]
pub(crate) use types::SwarmTemplate as SwarmTemplateForTests;

#[cfg(test)]
#[path = "../tests/swarm.rs"]
mod tests;
