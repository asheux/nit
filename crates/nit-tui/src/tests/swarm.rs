use super::*;
use nit_core::{AgentLane, AgentLaneKind, Buffer};
use std::path::PathBuf;

/// Build an empty `AppState` rooted at the crate's manifest dir with the two
/// default buffers the app expects. Tests that only need a fresh state to
/// seed lanes/missions/tasks call this instead of repeating the boilerplate.
fn new_state() -> AppState {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let editor = Buffer::empty("editor", None);
    let notes = Buffer::empty("notes", None);
    AppState::new(root, editor, notes)
}

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

#[test]
fn parse_swarm_mission_focus() {
    let cmd = parse_swarm_command("@swarm mission=research read papers").expect("cmd");
    assert_eq!(cmd.mission_kind, Some(SwarmMissionKind::Research));
    assert_eq!(cmd.prompt, "read papers");

    let cmd =
        parse_swarm_command("@swarm 4 m=computational-research model this topic").expect("cmd");
    assert_eq!(
        cmd.mission_kind,
        Some(SwarmMissionKind::ComputationalResearch)
    );
    assert_eq!(cmd.prompt, "model this topic");
}

#[test]
fn detect_swarm_mission_kind_requires_actual_research_intent() {
    assert_eq!(
        detect_swarm_mission_kind_from_prompt("Fix research role assignment in the TUI"),
        None
    );
    assert_eq!(
        detect_swarm_mission_kind_from_prompt(
            "Read papers, search the web, and rank strategies for this topic"
        ),
        Some(SwarmMissionKind::Research)
    );
    assert_eq!(
        detect_swarm_mission_kind_from_prompt(
            "Run simulations and compare modeling strategies for this research topic"
        ),
        Some(SwarmMissionKind::ComputationalResearch)
    );
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
        shadow: false,
        last_message: String::new(),
    }
}

#[test]
fn swarm_clones_do_not_count_towards_swarm_size() {
    let mut state = new_state();
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
    let mut state = new_state();
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
            None,
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
    let mut state = new_state();
    state.agents.agents.clear();

    state.agents.agents.push(make_lane("planner", "planner"));
    state.agents.agents.push(make_lane("a", "worker"));
    state.agents.agents.push(make_lane("b", "worker"));

    let agents = select_swarm_agents(&state, "planner", SwarmSize::Count(4), Some("parallel"));

    assert_eq!(agents, vec!["planner"]);
}

#[test]
fn parallel_without_priorities_clones_planner_to_swarm_size() {
    let mut state = new_state();
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
            None,
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
fn completed_swarm_cleans_up_mission_clone_lanes_from_roster() {
    let mut state = new_state();
    state.agents.messages.clear();
    state.agents.missions.clear();
    state.agents.agents.clear();
    state
        .agents
        .codex_effective_context_window_tokens
        .insert("planner".into(), 200_000);
    state
        .agents
        .codex_selected_reasoning_effort
        .insert("planner".into(), "medium".into());

    state.agents.agents.push(make_lane("planner", "planner"));

    let mut swarm = SwarmRuntime::default();
    let (mission_id, _dispatches) = swarm
        .start(
            &mut state,
            "planner".into(),
            vec!["planner".into()],
            SwarmSize::Count(2),
            Some("parallel".into()),
            None,
            "root".into(),
        )
        .expect("swarm start");

    let clone_id = format!("planner#swarm-{mission_id}-clone-01");
    assert!(state.agents.agents.iter().any(|lane| lane.id == clone_id));
    assert!(state
        .agents
        .codex_effective_context_window_tokens
        .contains_key(&clone_id));
    assert!(state
        .agents
        .codex_selected_reasoning_effort
        .contains_key(&clone_id));

    state.agents.selected_agent = Some(clone_id.clone());
    state.agents.roster_selected = state
        .agents
        .agents
        .iter()
        .position(|lane| lane.id == clone_id)
        .expect("clone roster index");

    let run = swarm.runs.get_mut(&mission_id).expect("active run");
    run.gate_bundle = None;
    run.verifier_agent_id = None;
    run.gate_selection = "auto:none".into();

    let planner_message = format!(
        r#"
```json
{{
  "version": 2,
  "template": "parallel",
  "tasks": [
{{ "id": "task-1", "agent_id": "{clone_id}", "title": "Task 1", "prompt": "ship it" }}
  ],
  "synthesis_prompt": "summarize"
}}
```
"#
    );
    let planner_event = AgentBusEvent::TurnCompleted {
        agent_id: "planner".into(),
        mission_id: Some(mission_id.clone()),
        thread_id: None,
        token_count: None,
        message: planner_message,
    };
    planner_event.apply(&mut state);
    let dispatches = swarm.handle_event(&mut state, &planner_event);
    assert_eq!(dispatches.len(), 1);
    assert_eq!(dispatches[0].agent_id, clone_id);

    let clone_event = AgentBusEvent::TurnCompleted {
        agent_id: clone_id.clone(),
        mission_id: Some(mission_id.clone()),
        thread_id: Some("thr-clone".into()),
        token_count: None,
        message: "done\n<SWARM_TASK_COMPLETE>".into(),
    };
    clone_event.apply(&mut state);
    let dispatches = swarm.handle_event(&mut state, &clone_event);
    assert_eq!(dispatches.len(), 1);
    assert_eq!(dispatches[0].agent_id, "planner");

    let planner_finish = AgentBusEvent::TurnCompleted {
        agent_id: "planner".into(),
        mission_id: Some(mission_id.clone()),
        thread_id: None,
        token_count: None,
        message: "final report".into(),
    };
    planner_finish.apply(&mut state);
    let dispatches = swarm.handle_event(&mut state, &planner_finish);
    assert!(dispatches.is_empty());

    assert!(!state.agents.agents.iter().any(|lane| lane.id == clone_id));
    assert_eq!(state.agents.selected_agent.as_deref(), Some("planner"));
    assert_eq!(
        state.agents.roster_selected,
        state
            .agents
            .agents
            .iter()
            .position(|lane| lane.id == "planner")
            .expect("planner roster index")
    );
    assert!(!state
        .agents
        .codex_effective_context_window_tokens
        .contains_key(&clone_id));
    assert!(!state
        .agents
        .codex_selected_reasoning_effort
        .contains_key(&clone_id));
    assert!(!state
        .agents
        .codex_mission_thread_ids
        .get(&mission_id)
        .is_some_and(|map| map.contains_key(&clone_id)));

    let mission = state
        .agents
        .missions
        .iter()
        .find(|mission| mission.id == mission_id)
        .expect("mission");
    assert_eq!(mission.status, "DONE");
}

#[test]
fn parallel_priority_selection_clones_from_planner() {
    let mut state = new_state();
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
            None,
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
        vec!["planner", "b", "d", "planner#swarm-mis-001-clone-01",]
    );
}

#[test]
fn parallel_priority_agents_ranked_before_non_priority() {
    let mut state = new_state();
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
    let mut state = new_state();
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
    let mut state = new_state();
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
fn parallel_tracks_single_integrator_hint_without_cloning_it() {
    let mut state = new_state();
    state.agents.messages.clear();
    state.agents.missions.clear();
    state.agents.agents.clear();

    state.agents.agents.push(make_lane("planner", "planner"));
    state.agents.agents.push(make_lane("a", "worker"));
    state.agents.agents.push(make_lane("b", "worker"));
    state
        .agents
        .swarm_role_by_agent_id
        .insert("a".into(), "integrate".into());
    state.agents.swarm_priority_agent_ids.insert("a".into());
    state.agents.swarm_priority_agent_ids.insert("b".into());

    let mut swarm = SwarmRuntime::default();
    let (mission_id, _dispatches) = swarm
        .start(
            &mut state,
            "planner".into(),
            vec!["planner".into(), "a".into(), "b".into()],
            SwarmSize::Count(4),
            Some("parallel".into()),
            None,
            "root".into(),
        )
        .expect("swarm start");

    let run = swarm.runs.get(&mission_id).expect("run");
    assert_eq!(run.integrator_agent_id.as_deref(), Some("a"));
}

#[test]
fn bulk_integrator_prefers_priority_agents() {
    let mut state = new_state();
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
            None,
            "root".into(),
        )
        .expect("swarm start");

    let run = swarm.runs.get(&mission_id).expect("run");
    assert_eq!(run.integrator_agent_id.as_deref(), Some("a"));
    assert!(!run.integrator_locked);
}

#[test]
fn bulk_priority_respects_role_hints() {
    let mut state = new_state();
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
    let mut state = new_state();
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
        retries: 0,
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
        SwarmMissionKind::General,
        "root",
        &available,
        Some("a1"),
        false,
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

    let warnings = apply_role_dependency_ordering(
        root.as_path(),
        &HashMap::new(),
        SwarmMissionKind::Research,
        None,
        &mut tasks,
        false,
    );

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

    apply_role_dependency_ordering(
        root.as_path(),
        &hints,
        SwarmMissionKind::Research,
        None,
        &mut tasks,
        false,
    );

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

    let warnings = apply_role_dependency_ordering(
        root.as_path(),
        &HashMap::new(),
        SwarmMissionKind::Research,
        None,
        &mut tasks,
        false,
    );

    let judge = tasks.iter().find(|t| t.id == "j").expect("judge");
    assert!(judge.deps.is_empty());
    assert!(warnings.iter().any(|w| w.contains("skipped")));
}

#[test]
fn role_ordering_does_not_inherit_singleton_role_hints_to_clones() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let mut tasks = vec![
        make_task("base", "a1", None, Vec::new()),
        make_task("clone", "a1#swarm-mis-001-clone-01", None, Vec::new()),
    ];

    let mut hints = HashMap::new();
    hints.insert("a1".into(), "integrate".into());

    apply_role_dependency_ordering(
        root.as_path(),
        &hints,
        SwarmMissionKind::General,
        Some("a1"),
        &mut tasks,
        false,
    );

    let base = tasks.iter().find(|t| t.id == "base").expect("base");
    let clone = tasks.iter().find(|t| t.id == "clone").expect("clone");
    assert_eq!(base.role.as_deref(), Some("integrate"));
    assert_eq!(clone.role.as_deref(), None);
}

#[test]
fn role_ordering_clears_integrate_role_for_non_integrator() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let mut tasks = vec![
        make_task("good", "a1", Some("integrate"), Vec::new()),
        make_task("bad", "a2", Some("integrate"), Vec::new()),
    ];

    let warnings = apply_role_dependency_ordering(
        root.as_path(),
        &HashMap::new(),
        SwarmMissionKind::General,
        Some("a1"),
        &mut tasks,
        false,
    );

    let good = tasks.iter().find(|t| t.id == "good").expect("good");
    let bad = tasks.iter().find(|t| t.id == "bad").expect("bad");
    assert_eq!(good.role.as_deref(), Some("integrate"));
    assert_eq!(bad.role.as_deref(), None);
    assert!(warnings
        .iter()
        .any(|warning| warning.contains("cleared invalid integrate role")));
}

#[test]
fn role_ordering_clears_research_role_for_non_research_prompts() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let mut tasks = vec![make_task(
        "research-task",
        "a1",
        Some("research"),
        Vec::new(),
    )];

    let warnings = apply_role_dependency_ordering(
        root.as_path(),
        &HashMap::new(),
        SwarmMissionKind::General,
        None,
        &mut tasks,
        false,
    );

    let task = tasks.first().expect("task");
    assert_eq!(task.role, None);
    assert!(warnings
        .iter()
        .any(|warning| warning.contains("does not permit that research role")));
}

#[test]
fn planner_role_hint_downgrades_research_hint_for_non_research_prompts() {
    let mut hints = HashMap::new();
    hints.insert("a1".into(), "research".into());

    let role = planner_role_hint_for_agent(&hints, "a1", None, SwarmMissionKind::General);
    assert_eq!(role, "all");

    let role = planner_role_hint_for_agent(&hints, "a1", None, SwarmMissionKind::Research);
    assert_eq!(role, "research");
}

#[test]
fn planner_role_hint_only_keeps_computational_role_for_computational_missions() {
    let mut hints = HashMap::new();
    hints.insert("a1".into(), COMPUTATIONAL_RESEARCH_ROLE.into());

    let role = planner_role_hint_for_agent(&hints, "a1", None, SwarmMissionKind::Research);
    assert_eq!(role, "all");

    let role =
        planner_role_hint_for_agent(&hints, "a1", None, SwarmMissionKind::ComputationalResearch);
    assert_eq!(role, COMPUTATIONAL_RESEARCH_ROLE);
}

#[test]
fn lab_fallback_reserves_research_roles_for_external_research() {
    let available = vec!["a1".to_string(), "a2".to_string(), "a3".to_string()];
    let parsed = fallback_tasks(
        SwarmTemplate::Lab,
        SwarmMissionKind::General,
        "root",
        &available,
        None,
        Some("a1"),
    );

    let recon = parsed
        .tasks
        .iter()
        .find(|task| task.id == "recon")
        .expect("recon");
    let design = parsed
        .tasks
        .iter()
        .find(|task| task.id == "design")
        .expect("design");
    assert_eq!(recon.role, None);
    assert_eq!(design.role.as_deref(), Some("propose"));
}

#[test]
fn lab_fallback_uses_research_shape_for_research_missions() {
    let available = vec!["a1".to_string(), "a2".to_string(), "a3".to_string()];
    let parsed = fallback_tasks(
        SwarmTemplate::Lab,
        SwarmMissionKind::Research,
        "research this topic",
        &available,
        None,
        Some("a1"),
    );

    let recon = parsed
        .tasks
        .iter()
        .find(|task| task.id == "recon")
        .expect("recon");
    let design = parsed
        .tasks
        .iter()
        .find(|task| task.id == "design")
        .expect("design");
    let implement = parsed
        .tasks
        .iter()
        .find(|task| task.id == "implement")
        .expect("implement");
    assert_eq!(recon.role.as_deref(), Some("research"));
    assert_eq!(design.role.as_deref(), Some("research"));
    assert_eq!(implement.role.as_deref(), Some("integrate"));
    assert!(!implement.writes);
}

#[test]
fn lab_fallback_uses_computational_lane_for_computational_missions() {
    let available = vec!["a1".to_string(), "a2".to_string(), "a3".to_string()];
    let parsed = fallback_tasks(
        SwarmTemplate::Lab,
        SwarmMissionKind::ComputationalResearch,
        "run simulations for this topic",
        &available,
        None,
        Some("a1"),
    );

    let design = parsed
        .tasks
        .iter()
        .find(|task| task.id == "design")
        .expect("design");
    let implement = parsed
        .tasks
        .iter()
        .find(|task| task.id == "implement")
        .expect("implement");
    assert_eq!(design.role.as_deref(), Some(COMPUTATIONAL_RESEARCH_ROLE));
    assert!(!implement.writes);
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
        SwarmMissionKind::General,
        "root",
        &available,
        Some("a1"),
        false,
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
        SwarmMissionKind::General,
        "root",
        &available,
        Some("a1"),
        false,
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
        SwarmMissionKind::General,
        "root",
        &available,
        Some("a1"),
        false,
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
        mission_kind: SwarmMissionKind::General,
        planner_agent_id: "planner".into(),
        integrator_agent_id: Some("a1".into()),
        integrator_locked: false,
        verifier_agent_id: None,
        gate_bundle: None,
        gate_custom: None,
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
                retries: 0,
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
                retries: 0,
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
                retries: 0,
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
                retries: 0,
            },
        ],
        synthesis_prompt: None,
        gate_output: None,
        gate_report: None,
        genome_gate_results: None,
        genome_gate_pending: None,
        genome_review_pending: None,
        report_status: None,
        report_output: None,
        scope_files: Vec::new(),
        initial_genome_baselines: std::collections::HashMap::new(),
        gate_retry_count: 0,
    };

    initialize_task_graph(&mut run);
    refresh_task_readiness(&mut run);

    let first = dispatch_ready_tasks(&mut run);
    assert_eq!(first.len(), 2);
    assert!(first.iter().any(|d| d.agent_id == "a2"));
    assert!(first.iter().any(|d| d.agent_id == "a3"));

    assert!(mark_task_finished(&mut run, "a2", "recon out".into(), false, false).is_some());
    assert!(mark_task_finished(&mut run, "a3", "design out".into(), false, false).is_some());
    refresh_task_readiness(&mut run);

    let second = dispatch_ready_tasks(&mut run);
    assert_eq!(second.len(), 1);
    assert_eq!(second[0].agent_id, "a1");

    assert!(mark_task_finished(&mut run, "a1", "impl out".into(), false, false).is_some());
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
        mission_kind: SwarmMissionKind::General,
        planner_agent_id: "planner".into(),
        integrator_agent_id: Some("a1".into()),
        integrator_locked: false,
        verifier_agent_id: None,
        gate_bundle: None,
        gate_custom: None,
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
                retries: 0,
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
                retries: 0,
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
                retries: 0,
            },
        ],
        synthesis_prompt: None,
        gate_output: None,
        gate_report: None,
        genome_gate_results: None,
        genome_gate_pending: None,
        genome_review_pending: None,
        report_status: None,
        report_output: None,
        scope_files: Vec::new(),
        initial_genome_baselines: std::collections::HashMap::new(),
        gate_retry_count: 0,
    };

    initialize_task_graph(&mut run);
    refresh_task_readiness(&mut run);

    let first = dispatch_ready_tasks(&mut run);
    // Should dispatch w1 and r1, but not w2 (single-writer lock).
    assert_eq!(first.len(), 2);
    assert!(first.iter().any(|d| d.prompt.contains("Write 1 (w1)")));
    assert!(first.iter().any(|d| d.prompt.contains("Read (r1)")));
    assert!(!first.iter().any(|d| d.prompt.contains("Write 2 (w2)")));

    assert!(mark_task_finished(&mut run, "a1", "w1 out".into(), false, false).is_some());
    refresh_task_readiness(&mut run);
    let second = dispatch_ready_tasks(&mut run);
    assert_eq!(second.len(), 1);
    assert!(second[0].prompt.contains("Write 2 (w2)"));
}

#[test]
fn parallel_template_dispatches_multiple_writers_concurrently() {
    // The Parallel template exists to exercise write fan-out: integrate
    // tasks with disjoint work regions (enforced only by the planner
    // prompt, not the dispatcher) should all execute at once. Lab / Bulk
    // enforce global single-writer; Parallel does not.
    let mut run = SwarmRun {
        mission_id: "mis-parallel".into(),
        root_prompt: "root".into(),
        template: SwarmTemplate::Parallel,
        mission_kind: SwarmMissionKind::General,
        planner_agent_id: "planner".into(),
        integrator_agent_id: Some("a1".into()),
        integrator_locked: false,
        verifier_agent_id: None,
        gate_bundle: None,
        gate_custom: None,
        gate_selection: "auto:none".into(),
        agent_ids: vec!["planner".into(), "a1".into(), "a2".into(), "a3".into()],
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
                retries: 0,
            },
            SwarmTask {
                id: "w2".into(),
                agent_id: "a2".into(),
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
                retries: 0,
            },
            SwarmTask {
                id: "w3".into(),
                agent_id: "a3".into(),
                role: Some("integrate".into()),
                title: "Write 3".into(),
                task_prompt: "w3".into(),
                deps: Vec::new(),
                writes: true,
                artifacts: Vec::new(),
                done_when: None,
                state: SwarmTaskState::Pending,
                output: None,
                parsed_artifacts: None,
                expected_artifacts_missing: false,
                failed: false,
                retries: 0,
            },
        ],
        synthesis_prompt: None,
        gate_output: None,
        gate_report: None,
        genome_gate_results: None,
        genome_gate_pending: None,
        genome_review_pending: None,
        report_status: None,
        report_output: None,
        scope_files: Vec::new(),
        initial_genome_baselines: std::collections::HashMap::new(),
        gate_retry_count: 0,
    };

    initialize_task_graph(&mut run);
    refresh_task_readiness(&mut run);

    let dispatches = dispatch_ready_tasks(&mut run);
    assert_eq!(
        dispatches.len(),
        3,
        "parallel template should fan out all three writer tasks at once; got {dispatches:?}"
    );
    assert!(dispatches.iter().any(|d| d.prompt.contains("Write 1 (w1)")));
    assert!(dispatches.iter().any(|d| d.prompt.contains("Write 2 (w2)")));
    assert!(dispatches.iter().any(|d| d.prompt.contains("Write 3 (w3)")));
}

#[test]
fn task_prompt_includes_role_contract_guidance() {
    let task = make_task("judge", "a1", Some("judge"), vec!["propose-01"]);
    let prompt = wrap_task_prompt("root", SwarmMissionKind::General, &task, None, &[]);

    assert!(prompt.contains("ROLE CONTRACT:"));
    assert!(prompt.contains("Act strictly as the assigned role"));
    assert!(prompt.contains("Compare the dependency outputs"));
}

#[test]
fn research_role_contract_mentions_external_sources() {
    let task = make_task("research", "a1", Some("research"), Vec::new());
    let prompt = wrap_task_prompt("root", SwarmMissionKind::Research, &task, None, &[]);

    assert!(prompt.contains("papers, docs, web resources"));
    assert!(prompt.contains("best strategy candidates"));
    assert!(prompt.contains("MISSION FOCUS: research"));
    assert!(prompt.contains("Sources:"));
    assert!(prompt.contains("Methods:"));
    assert!(prompt.contains("Assumptions:"));
    assert!(prompt.contains("Ranked strategies:"));
}

#[test]
fn computational_research_role_contract_mentions_modeling_and_simulation() {
    let task = make_task(
        "comp-research",
        "a1",
        Some(COMPUTATIONAL_RESEARCH_ROLE),
        Vec::new(),
    );
    let prompt = wrap_task_prompt(
        "root",
        SwarmMissionKind::ComputationalResearch,
        &task,
        None,
        &[],
    );

    assert!(prompt.contains("simulations, modeling, numerical methods, optimization"));
    assert!(prompt.contains("reproducible research workflows"));
    assert!(prompt.contains("MISSION FOCUS: computational-research"));
}

#[test]
fn planner_prompt_describes_research_roles_as_topic_research() {
    let prompt = build_planner_prompt(
        "root",
        SwarmTemplate::Parallel,
        SwarmMissionKind::General,
        "planner",
        &["planner".into(), "a1".into()],
        None,
        &[],
        &[],
        std::path::Path::new("."),
        &[],
    );

    assert!(prompt.contains("web/paper/resource exploration"));
    assert!(prompt.contains("not routine codebase recon"));
    assert!(prompt.contains("simulations, modeling, numerical methods, optimization"));
}

#[test]
fn planner_prompt_describes_computational_research_mission_shape() {
    let prompt = build_planner_prompt(
        "root",
        SwarmTemplate::Lab,
        SwarmMissionKind::ComputationalResearch,
        "planner",
        &["planner".into(), "a1".into()],
        None,
        &[],
        &[],
        std::path::Path::new("."),
        &[],
    );

    assert!(prompt.contains("source survey -> modeling / experiments / analysis"));
    assert!(prompt.contains("preferred for quantitative or tool-driven lanes"));
    assert!(prompt.contains("Prefer read-only investigation and synthesis tasks"));
}

// Bulk planner prompt must keep its existing "distinct lens" guidance —
// that's the defining discipline that distinguishes bulk from lab.
#[test]
fn bulk_planner_prompt_keeps_distinct_lens_guidance() {
    let prompt = build_planner_prompt(
        "root",
        SwarmTemplate::Bulk,
        SwarmMissionKind::General,
        "planner",
        &["planner".into(), "a1".into(), "a2".into(), "a3".into()],
        Some("a1"),
        &[],
        &[],
        std::path::Path::new("."),
        &[],
    );

    assert!(
        prompt.contains("distinct lens"),
        "bulk planner must still enforce distinct lenses per proposer"
    );
    assert!(
        prompt.contains("judge task that depends on ALL proposer tasks"),
        "bulk planner must still require judge fan-in"
    );
}

// The parallelism guidance is lab-specific — parallel and bulk each have
// their own proposer orchestration rules (parallel: "reserve at least
// ONE propose lane"; bulk: "distinct lens per proposer + judge fan-in").
// Lab's "PROPOSER PARALLELISM" text must not leak into either — duplicated
// guidance across templates gives the planner contradictory instructions.
#[test]
fn parallel_planner_prompt_does_not_use_lab_parallelism_text() {
    let prompt = build_planner_prompt(
        "root",
        SwarmTemplate::Parallel,
        SwarmMissionKind::General,
        "planner",
        &["planner".into(), "a1".into(), "a2".into(), "a3".into()],
        Some("a1"),
        &[],
        &[],
        std::path::Path::new("."),
        &[],
    );

    assert!(
        !prompt.contains("PROPOSER PARALLELISM (lab)"),
        "parallel planner must not inherit lab-specific parallelism text"
    );
}

// The lab planner guide must tell the LLM that when multiple proposers
// are assigned, they run in parallel (empty deps) — not in a chain.
// Sequential proposers (propose-02 depending on propose-01) waste
// wall-clock time because the judge has to wait for the last one anyway.
#[test]
fn lab_planner_prompt_tells_proposers_to_run_in_parallel() {
    let prompt = build_planner_prompt(
        "root",
        SwarmTemplate::Lab,
        SwarmMissionKind::General,
        "planner",
        &["planner".into(), "a1".into(), "a2".into(), "a3".into()],
        Some("a1"),
        &[],
        &[],
        std::path::Path::new("."),
        &[],
    );

    assert!(
        prompt.contains("PROPOSER PARALLELISM (lab)"),
        "lab planner must surface the proposer-parallelism rule"
    );
    assert!(
        prompt.contains("empty `deps`"),
        "lab planner must tell the LLM to use empty deps on proposers"
    );
    assert!(
        prompt.contains("Do NOT chain them"),
        "lab planner must explicitly forbid chaining proposers"
    );
}

// Lab's planner must teach the LLM how to diverge multi-proposer plans
// on real optimisation axes, not just variant labels. The bullet lists
// five concrete lenses (minimal-diff / architectural / incremental /
// performance / safety) and tells the planner to bake them into each
// proposer's `task_prompt`.
#[test]
fn lab_planner_prompt_teaches_distinct_lenses() {
    let prompt = build_planner_prompt(
        "root",
        SwarmTemplate::Lab,
        SwarmMissionKind::General,
        "planner",
        &["planner".into(), "a1".into(), "a2".into(), "a3".into()],
        Some("a1"),
        &[],
        &[],
        std::path::Path::new("."),
        &[],
    );

    assert!(
        prompt.contains("PROPOSER LENSES (lab)"),
        "lab planner must surface the lens-diversification rule"
    );
    // At least three concrete lenses named — the planner needs a menu,
    // not abstract advice.
    for lens in ["LENS A", "LENS B", "LENS C"] {
        assert!(
            prompt.contains(lens),
            "lab planner must enumerate {lens} as a concrete option"
        );
    }
    // Axes named plainly so the planner can map the request to a lens.
    assert!(prompt.contains("minimal-diff"));
    assert!(prompt.contains("architectural coherence"));
    // Instruction to actually embed the lens in task_prompt, not just
    // mention it in free-form planning text.
    assert!(prompt.contains("task_prompt"));
}

// Defensive repair: when the planner assigns multiple proposers without
// lens markers in any of their task_prompts, `apply_lab_lenses` injects
// default lenses (LENS A / LENS B / ... cycled mod 5) into each so the
// proposers actually diverge. The injection goes at the head of the
// existing task_prompt, preserving whatever the planner wrote.
#[test]
fn apply_lab_lenses_injects_defaults_when_planner_omits() {
    use super::{apply_lab_lenses, SwarmTask};

    let mk = |id: &str, role: &str, prompt: &str| SwarmTask {
        id: id.into(),
        agent_id: "a1".into(),
        role: Some(role.into()),
        title: format!("{role} task"),
        task_prompt: prompt.into(),
        deps: Vec::new(),
        writes: false,
        artifacts: Vec::new(),
        done_when: None,
        state: SwarmTaskState::Pending,
        output: None,
        parsed_artifacts: None,
        expected_artifacts_missing: false,
        failed: false,
        retries: 0,
    };

    let mut tasks = vec![
        mk(
            "propose-01",
            "propose",
            "Survey nit-syntax and recommend fixes.",
        ),
        mk(
            "propose-02",
            "propose",
            "Survey nit-syntax and recommend fixes.",
        ),
        mk(
            "propose-03",
            "propose",
            "Survey nit-syntax and recommend fixes.",
        ),
        mk("judge", "judge", "Pick the strongest proposal."),
    ];

    let warnings = apply_lab_lenses(&mut tasks);

    // Each proposer got a distinct lens injected.
    let p01 = tasks.iter().find(|t| t.id == "propose-01").unwrap();
    let p02 = tasks.iter().find(|t| t.id == "propose-02").unwrap();
    let p03 = tasks.iter().find(|t| t.id == "propose-03").unwrap();

    assert!(p01.task_prompt.contains("LENS A"));
    assert!(p02.task_prompt.contains("LENS B"));
    assert!(p03.task_prompt.contains("LENS C"));

    // Original body preserved after the injected lens.
    for t in [p01, p02, p03] {
        assert!(
            t.task_prompt.contains("Survey nit-syntax"),
            "original prompt body must be preserved after lens injection"
        );
    }

    // Judge untouched.
    let judge = tasks.iter().find(|t| t.id == "judge").unwrap();
    assert_eq!(judge.task_prompt, "Pick the strongest proposal.");

    // Warning per injection with proposer id + lens label.
    assert_eq!(warnings.len(), 3);
    assert!(warnings
        .iter()
        .any(|w| w.contains("propose-01") && w.contains("LENS A")));
    assert!(warnings
        .iter()
        .any(|w| w.contains("propose-02") && w.contains("LENS B")));
    assert!(warnings
        .iter()
        .any(|w| w.contains("propose-03") && w.contains("LENS C")));
}

// Trust the planner when it did bake lenses into any proposer prompt —
// mixing planner-supplied and injected lenses would confuse the agents.
// Partial lens coverage is drift the operator can see via the mission
// log; we don't try to repair it automatically.
#[test]
fn apply_lab_lenses_preserves_planner_supplied_lenses() {
    use super::{apply_lab_lenses, SwarmTask};

    let mk = |id: &str, prompt: &str| SwarmTask {
        id: id.into(),
        agent_id: "a1".into(),
        role: Some("propose".into()),
        title: "propose task".into(),
        task_prompt: prompt.into(),
        deps: Vec::new(),
        writes: false,
        artifacts: Vec::new(),
        done_when: None,
        state: SwarmTaskState::Pending,
        output: None,
        parsed_artifacts: None,
        expected_artifacts_missing: false,
        failed: false,
        retries: 0,
    };

    let p01_original = "LENS A (minimal-diff): smallest fix. Survey the module.";
    let p02_original = "LENS D (performance): target the hot paths. Survey the module.";
    let mut tasks = vec![
        mk("propose-01", p01_original),
        mk("propose-02", p02_original),
    ];

    let warnings = apply_lab_lenses(&mut tasks);

    assert!(
        warnings.is_empty(),
        "no injection when planner supplied lenses"
    );
    assert_eq!(
        tasks
            .iter()
            .find(|t| t.id == "propose-01")
            .unwrap()
            .task_prompt,
        p01_original
    );
    assert_eq!(
        tasks
            .iter()
            .find(|t| t.id == "propose-02")
            .unwrap()
            .task_prompt,
        p02_original
    );
}

// No injection when only one proposer exists — lens divergence is
// meaningless with a single investigator.
#[test]
fn apply_lab_lenses_is_noop_for_single_proposer() {
    use super::{apply_lab_lenses, SwarmTask};

    let mut tasks = vec![SwarmTask {
        id: "propose".into(),
        agent_id: "a1".into(),
        role: Some("propose".into()),
        title: "propose task".into(),
        task_prompt: "Survey the module.".into(),
        deps: Vec::new(),
        writes: false,
        artifacts: Vec::new(),
        done_when: None,
        state: SwarmTaskState::Pending,
        output: None,
        parsed_artifacts: None,
        expected_artifacts_missing: false,
        failed: false,
        retries: 0,
    }];

    let warnings = apply_lab_lenses(&mut tasks);

    assert!(warnings.is_empty());
    assert_eq!(
        tasks[0].task_prompt, "Survey the module.",
        "single-proposer lab plan should pass through untouched"
    );
}

// Plans with >5 proposers cycle the lens pool rather than silently
// dropping divergence on proposers 6+. Degenerate but defensible
// behaviour.
#[test]
fn apply_lab_lenses_cycles_when_proposer_count_exceeds_pool() {
    use super::{apply_lab_lenses, SwarmTask};

    let mk = |id: &str| SwarmTask {
        id: id.into(),
        agent_id: "a1".into(),
        role: Some("propose".into()),
        title: "propose task".into(),
        task_prompt: String::new(),
        deps: Vec::new(),
        writes: false,
        artifacts: Vec::new(),
        done_when: None,
        state: SwarmTaskState::Pending,
        output: None,
        parsed_artifacts: None,
        expected_artifacts_missing: false,
        failed: false,
        retries: 0,
    };

    let mut tasks: Vec<SwarmTask> = (0..7).map(|i| mk(&format!("propose-{i:02}"))).collect();
    let warnings = apply_lab_lenses(&mut tasks);
    assert_eq!(warnings.len(), 7);

    // Positions 0..5 get A/B/C/D/E, then 5 and 6 cycle back to A and B.
    for (nth, expected) in [
        (0, "A"),
        (1, "B"),
        (2, "C"),
        (3, "D"),
        (4, "E"),
        (5, "A"),
        (6, "B"),
    ] {
        let id = format!("propose-{nth:02}");
        let t = tasks.iter().find(|t| t.id == id).unwrap();
        assert!(
            t.task_prompt.contains(&format!("LENS {expected}")),
            "proposer {id} should carry LENS {expected}"
        );
    }
}

// Defensive repair: even if the planner ignores the parallelism guidance
// and emits a chained proposer plan, `normalize_lab_plan` strips
// proposer-to-proposer deps so execution ends up parallel anyway. The
// repair MUST leave proposer→judge and proposer→integrate deps alone.
#[test]
fn normalize_lab_plan_strips_proposer_to_proposer_deps() {
    use super::{normalize_lab_plan, SwarmTask};

    // Build a chained-proposer plan + a judge that waits on all of them.
    let mk = |id: &str, role: &str, deps: &[&str]| SwarmTask {
        id: id.into(),
        agent_id: "a1".into(),
        role: Some(role.into()),
        title: format!("{role} task"),
        task_prompt: String::new(),
        deps: deps.iter().map(|s| s.to_string()).collect(),
        writes: false,
        artifacts: Vec::new(),
        done_when: None,
        state: SwarmTaskState::Pending,
        output: None,
        parsed_artifacts: None,
        expected_artifacts_missing: false,
        failed: false,
        retries: 0,
    };

    let mut tasks = vec![
        mk("propose-01", "propose", &[]),
        mk("propose-02", "propose", &["propose-01"]), // chained — should be stripped
        mk("propose-03", "propose", &["propose-02"]), // chained — should be stripped
        mk(
            "judge",
            "judge",
            &["propose-01", "propose-02", "propose-03"],
        ),
        mk("integrate", "integrate", &["judge"]),
    ];

    let warnings = normalize_lab_plan(&mut tasks);

    // propose-02's dep on propose-01 — stripped.
    assert!(
        tasks
            .iter()
            .find(|t| t.id == "propose-02")
            .unwrap()
            .deps
            .is_empty(),
        "propose-02 must have empty deps after normalization"
    );
    // propose-03's dep on propose-02 — stripped.
    assert!(
        tasks
            .iter()
            .find(|t| t.id == "propose-03")
            .unwrap()
            .deps
            .is_empty(),
        "propose-03 must have empty deps after normalization"
    );
    // Judge's deps on ALL proposers — preserved.
    let judge_deps = tasks.iter().find(|t| t.id == "judge").unwrap().deps.clone();
    assert_eq!(judge_deps.len(), 3);
    for p in ["propose-01", "propose-02", "propose-03"] {
        assert!(
            judge_deps.iter().any(|d| d == p),
            "judge dep on {p} must be preserved"
        );
    }
    // Integrate's dep on judge — preserved.
    assert_eq!(
        tasks.iter().find(|t| t.id == "integrate").unwrap().deps,
        vec!["judge".to_string()]
    );
    // Warnings must enumerate every dep stripped.
    assert!(warnings
        .iter()
        .any(|w| w.contains("propose-02") && w.contains("propose-01")));
    assert!(warnings
        .iter()
        .any(|w| w.contains("propose-03") && w.contains("propose-02")));
}

// Single-proposer lab plans must not trigger the repair — the helper is
// a no-op when fewer than two proposers are present, otherwise it would
// emit spurious warnings on normal lab plans.
#[test]
fn normalize_lab_plan_is_noop_for_single_proposer() {
    use super::{normalize_lab_plan, SwarmTask};

    let mk = |id: &str, role: &str, deps: &[&str]| SwarmTask {
        id: id.into(),
        agent_id: "a1".into(),
        role: Some(role.into()),
        title: format!("{role} task"),
        task_prompt: String::new(),
        deps: deps.iter().map(|s| s.to_string()).collect(),
        writes: false,
        artifacts: Vec::new(),
        done_when: None,
        state: SwarmTaskState::Pending,
        output: None,
        parsed_artifacts: None,
        expected_artifacts_missing: false,
        failed: false,
        retries: 0,
    };

    let mut tasks = vec![
        mk("propose", "propose", &[]),
        mk("review", "review", &["propose"]),
        mk("integrate", "integrate", &["review"]),
    ];
    let warnings = normalize_lab_plan(&mut tasks);

    assert!(warnings.is_empty(), "single-proposer plan needs no repair");
    assert_eq!(
        tasks.iter().find(|t| t.id == "review").unwrap().deps,
        vec!["propose".to_string()],
        "review's dep on propose is preserved"
    );
}

// Lab-specific "PROPOSER PARALLELISM" text must not leak into bulk —
// bulk has its own orchestration rules (distinct lens + judge fan-in).
#[test]
fn bulk_planner_prompt_does_not_use_lab_parallelism_text() {
    let prompt = build_planner_prompt(
        "root",
        SwarmTemplate::Bulk,
        SwarmMissionKind::General,
        "planner",
        &["planner".into(), "a1".into(), "a2".into(), "a3".into()],
        Some("a1"),
        &[],
        &[],
        std::path::Path::new("."),
        &[],
    );

    assert!(
        !prompt.contains("PROPOSER PARALLELISM (lab)"),
        "bulk planner must not inherit lab-specific parallelism text"
    );
}

// Shadow prompts (single-agent pipeline) have no planner and share no
// builder code with the swarm planner. Lab-specific parallelism guidance
// must NOT leak into any shadow role prompt.
#[test]
fn shadow_prompts_do_not_inherit_lab_parallelism_text() {
    use crate::shadow::ShadowRuntime;
    use nit_core::state::AgentTurnState as _AgentTurnState; // force runtime-state linkage
    use nit_core::{AgentLane, AgentLaneKind, AgentStatus, AppState, Buffer};
    let _: &dyn std::any::Any = &std::marker::PhantomData::<_AgentTurnState>;

    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let editor = Buffer::empty("editor", None);
    let notes = Buffer::empty("notes", None);
    let mut state = AppState::new(root, editor, notes);
    state.agents.agents.clear();
    state.agents.agents.push(AgentLane {
        id: "codex-main".into(),
        role: "coder".into(),
        lane: "Codex".into(),
        kind: AgentLaneKind::Codex,
        status: AgentStatus::Idle,
        heartbeat_age_secs: 0,
        queue_len: 0,
        current_mission: None,
        last_message: String::new(),
        shadow: false,
    });

    let mut rt = ShadowRuntime::new();
    let dispatches = rt
        .start(
            &mut state,
            "codex-main".into(),
            "refactor crates/nit-syntax module".into(),
            None,
            Some(0),
        )
        .expect("shadow start");

    for d in &dispatches {
        assert!(
            !d.prompt.contains("PROPOSER PARALLELISM (lab)"),
            "shadow proposer prompt must not inherit lab planner text"
        );
    }
}

#[test]
fn deadlock_detection_skips_pending_tasks() {
    let mut run = SwarmRun {
        mission_id: "mis-001".into(),
        root_prompt: "root".into(),
        template: SwarmTemplate::Lab,
        mission_kind: SwarmMissionKind::General,
        planner_agent_id: "planner".into(),
        integrator_agent_id: Some("a1".into()),
        integrator_locked: false,
        verifier_agent_id: None,
        gate_bundle: None,
        gate_custom: None,
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
                retries: 0,
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
                retries: 0,
            },
        ],
        synthesis_prompt: None,
        gate_output: None,
        gate_report: None,
        genome_gate_results: None,
        genome_gate_pending: None,
        genome_review_pending: None,
        report_status: None,
        report_output: None,
        scope_files: Vec::new(),
        initial_genome_baselines: std::collections::HashMap::new(),
        gate_retry_count: 0,
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
    let mut state = new_state();
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
        shadow: false,
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
        shadow: false,
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
            None,
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
fn strict_dag_abort_cleans_up_mission_clone_lanes_from_roster() {
    let mut state = new_state();
    state.agents.messages.clear();
    state.agents.missions.clear();
    state.agents.agents.clear();

    state.agents.agents.push(make_lane("planner", "planner"));

    let mut swarm = SwarmRuntime::default();
    let (mission_id, _dispatches) = swarm
        .start(
            &mut state,
            "planner".into(),
            vec!["planner".into()],
            SwarmSize::Count(2),
            Some("parallel".into()),
            None,
            "root".into(),
        )
        .expect("swarm start");

    let clone_id = format!("planner#swarm-{mission_id}-clone-01");
    assert!(state.agents.agents.iter().any(|lane| lane.id == clone_id));

    let planner_message = format!(
        r#"
```json
{{
  "version": 2,
  "template": "parallel",
  "tasks": [
{{ "id": "t1", "agent_id": "{clone_id}", "title": "T1", "prompt": "DONE t1", "deps": ["t2"] }},
{{ "id": "t2", "agent_id": "{clone_id}", "title": "T2", "prompt": "DONE t2", "deps": ["t1"] }}
  ]
}}
```
"#
    );

    let event = AgentBusEvent::TurnCompleted {
        agent_id: "planner".into(),
        mission_id: Some(mission_id.clone()),
        thread_id: None,
        token_count: None,
        message: planner_message,
    };
    event.apply(&mut state);
    let dispatches = swarm.handle_event(&mut state, &event);

    assert!(dispatches.is_empty());
    assert!(!swarm.runs.contains_key(mission_id.as_str()));
    assert!(swarm.completed_runs.contains_key(mission_id.as_str()));
    assert!(!state.agents.agents.iter().any(|lane| lane.id == clone_id));

    let mission = state
        .agents
        .missions
        .iter()
        .find(|mission| mission.id == mission_id)
        .expect("mission");
    assert_eq!(mission.status, "FAILED");
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
        mission_kind: SwarmMissionKind::General,
        planner_agent_id: "planner".into(),
        integrator_agent_id: Some("a1".into()),
        integrator_locked: false,
        verifier_agent_id: Some("a2".into()),
        gate_bundle: Some(GateBundle::Rust),
        gate_custom: None,
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
                retries: 0,
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
                retries: 0,
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
                retries: 0,
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
                retries: 0,
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
        genome_gate_results: None,
        genome_gate_pending: None,
        genome_review_pending: None,
        report_status: None,
        report_output: None,
        scope_files: Vec::new(),
        initial_genome_baselines: std::collections::HashMap::new(),
        gate_retry_count: 0,
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

#[test]
fn chat_clone_base_id_parsing() {
    assert_eq!(chat_clone_base_id("agent-a#chat-clone-01"), Some("agent-a"));
    assert_eq!(chat_clone_base_id("agent-a#chat-clone-12"), Some("agent-a"));
    assert_eq!(chat_clone_base_id("agent-a"), None);
    assert_eq!(chat_clone_base_id("agent-a#swarm-mis-01"), None);
}

#[test]
fn is_chat_clone_agent_id_detection() {
    assert!(is_chat_clone_agent_id("agent-a#chat-clone-01"));
    assert!(!is_chat_clone_agent_id("agent-a"));
    assert!(!is_chat_clone_agent_id("agent-a#swarm-mis-01"));
}

#[test]
fn create_chat_clone_basic() {
    let mut state = new_state();
    state.agents.messages.clear();
    state.agents.agents.clear();

    state.agents.agents.push(make_lane("agent-a", "coder"));

    let clone_id = create_chat_clone(&mut state, "agent-a").expect("clone created");
    assert_eq!(clone_id, "agent-a#chat-clone-01");

    let clone_lane = state
        .agents
        .agents
        .iter()
        .find(|l| l.id == clone_id)
        .expect("clone in roster");
    assert_eq!(clone_lane.role, "coder (clone 01)");
    assert!(matches!(clone_lane.status, AgentStatus::Idle));
    assert_eq!(clone_lane.queue_len, 0);

    // Clone should be right after its base
    let base_pos = state
        .agents
        .agents
        .iter()
        .position(|l| l.id == "agent-a")
        .unwrap();
    let clone_pos = state
        .agents
        .agents
        .iter()
        .position(|l| l.id == clone_id)
        .unwrap();
    assert_eq!(clone_pos, base_pos + 1);
}

#[test]
fn create_chat_clone_sequential_numbering() {
    let mut state = new_state();
    state.agents.messages.clear();
    state.agents.agents.clear();

    state.agents.agents.push(make_lane("agent-a", "coder"));

    let first = create_chat_clone(&mut state, "agent-a").expect("first clone");
    assert_eq!(first, "agent-a#chat-clone-01");

    let second = create_chat_clone(&mut state, "agent-a").expect("second clone");
    assert_eq!(second, "agent-a#chat-clone-02");
}

#[test]
fn create_chat_clone_from_clone_resolves_base() {
    let mut state = new_state();
    state.agents.messages.clear();
    state.agents.agents.clear();

    state.agents.agents.push(make_lane("agent-a", "coder"));
    let first = create_chat_clone(&mut state, "agent-a").expect("first clone");

    // Cloning from the clone should still use the root agent
    let second = create_chat_clone(&mut state, &first).expect("second clone");
    assert_eq!(second, "agent-a#chat-clone-02");
}

#[test]
fn chat_clones_excluded_from_select_swarm_agents() {
    let mut state = new_state();
    state.agents.messages.clear();
    state.agents.agents.clear();

    state.agents.agents.push(make_lane("planner", "planner"));
    state.agents.agents.push(make_lane("a", "worker"));
    state.agents.agents.push(make_lane("b", "worker"));
    state.agents.swarm_priority_agent_ids.insert("a".into());
    state.agents.swarm_priority_agent_ids.insert("b".into());

    // Add a chat clone — it should be ignored
    state
        .agents
        .agents
        .push(make_lane("a#chat-clone-01", "worker (clone 01)"));

    let agents = select_swarm_agents(&state, "planner", SwarmSize::Count(3), Some("parallel"));
    assert!(!agents.iter().any(|id| id.contains("#chat-clone-")));
    assert!(agents.contains(&"a".to_string()));
    assert!(agents.contains(&"b".to_string()));
}

#[test]
fn derive_cargo_packages_collects_unique_crate_names() {
    let files = vec![
        "crates/nit-tui/src/swarm.rs".to_string(),
        "crates/nit-tui/src/app/mod.rs".to_string(),
        "crates/nit-core/src/state.rs".to_string(),
        "crates/nit-tui/src/swarm.rs".to_string(), // duplicate
    ];
    let pkgs = derive_cargo_packages(&files);
    assert_eq!(pkgs, vec!["nit-tui".to_string(), "nit-core".to_string()]);
}

#[test]
fn derive_cargo_packages_returns_empty_when_any_file_is_outside_crates_dir() {
    // A file outside `crates/` (e.g., workspace-root file) means the scope is
    // mixed and we cannot safely run scoped cargo commands — fall back to the
    // full workspace.
    let files = vec![
        "crates/nit-tui/src/swarm.rs".to_string(),
        "Cargo.toml".to_string(),
    ];
    assert!(derive_cargo_packages(&files).is_empty());
}

#[test]
fn derive_cargo_packages_empty_scope_returns_empty() {
    assert!(derive_cargo_packages(&[]).is_empty());
}

#[test]
fn gate_rendered_command_substitutes_cargo_packages_when_scoped() {
    let gate = Gate {
        name: "test".into(),
        command: "cargo test --workspace --all-features".into(),
        scoped_command: Some("cargo test {cargo_packages} --all-features".into()),
    };
    let pkgs = vec!["nit-tui".to_string(), "nit-core".to_string()];
    assert_eq!(
        gate.rendered_command(&pkgs),
        "cargo test -p nit-tui -p nit-core --all-features"
    );
}

#[test]
fn gate_rendered_command_falls_back_to_full_when_no_scope() {
    let gate = Gate {
        name: "test".into(),
        command: "cargo test --workspace --all-features".into(),
        scoped_command: Some("cargo test {cargo_packages} --all-features".into()),
    };
    assert_eq!(
        gate.rendered_command(&[]),
        "cargo test --workspace --all-features"
    );
}

#[test]
fn gate_rendered_command_falls_back_when_no_scoped_template() {
    let gate = Gate {
        name: "lint".into(),
        command: "npm run lint --if-present".into(),
        scoped_command: None,
    };
    let pkgs = vec!["some-pkg".to_string()];
    assert_eq!(gate.rendered_command(&pkgs), "npm run lint --if-present");
}

#[test]
fn rust_bundle_gates_render_scoped_when_cargo_packages_provided() {
    let bundle = GateBundle::Rust;
    let pkgs = vec!["nit-tui".to_string()];
    let rendered: Vec<String> = bundle
        .gates()
        .into_iter()
        .map(|g| g.rendered_command(&pkgs))
        .collect();
    assert_eq!(
        rendered,
        vec![
            "cargo fmt -p nit-tui -- --check".to_string(),
            "cargo clippy -p nit-tui --all-targets --all-features -- -D warnings".to_string(),
            "cargo test -p nit-tui --all-features".to_string(),
        ]
    );
}

// ---------------------------------------------------------------------------
// Role contract regression tests — pin the "operator-only workspace-wide
// widening" rule into the test suite so a future prompt edit can't quietly
// soften the language. The user explicitly fixed this twice; we don't want
// it to come back.
// ---------------------------------------------------------------------------

/// Returns the joined role-contract text for a role so we can grep for
/// required phrases. Reaches into the private `role_contract_lines` helper.
fn role_contract_text(role: &str) -> String {
    role_contract_lines(role).join("\n")
}

/// Returns true when `text` semantically forbids running tests / builds /
/// lints / CI commands. Checks case-insensitively for both a "do not run" /
/// "must not run" directive and at least one verification verb. Lets the
/// arms phrase the rule slightly differently without breaking the test.
fn forbids_verification_commands(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    let directive_present = lower.contains("do not run") || lower.contains("must not run");
    let verb_present = lower.contains("test")
        || lower.contains("build")
        || lower.contains("lint")
        || lower.contains(" ci");
    directive_present && verb_present
}

#[test]
fn integrate_role_contract_forbids_unauthorized_workspace_wide_runs() {
    let text = role_contract_text("integrate");
    // Must contain the operator-only widening rule.
    assert!(
        text.contains("ONLY allowed when the OPERATOR explicitly asked"),
        "integrate role must include the operator-only widening rule, got: {text}"
    );
    // Must list the canonical workspace-wide commands as forbidden examples
    // so the agent recognizes them when tempted.
    assert!(
        text.contains("cargo test --all"),
        "integrate role must name `cargo test --all` as forbidden"
    );
    assert!(
        text.contains("--workspace"),
        "integrate role must name `--workspace` as forbidden"
    );
    // Must guide the agent to combine targeted flags rather than widen.
    assert!(
        text.contains("targeted") || text.contains("Targeted"),
        "integrate role must steer toward targeted commands"
    );
    // Must NOT contain the old loophole language we removed.
    assert!(
        !text.contains("as appropriate"),
        "integrate role must not contain the 'as appropriate' loophole that the agent reads as license to widen"
    );
}

#[test]
fn test_role_contract_forbids_unauthorized_workspace_wide_runs() {
    let text = role_contract_text("test");
    assert!(
        text.contains("ONLY allowed when the OPERATOR explicitly asked"),
        "test role must include the operator-only widening rule, got: {text}"
    );
    assert!(
        text.contains("cargo test --all"),
        "test role must name `cargo test --all` as forbidden"
    );
    assert!(
        text.contains("--workspace"),
        "test role must name `--workspace` as forbidden"
    );
    // Must mention multi-module guidance (combine flags, not widen).
    assert!(
        text.contains("multiple targeted flags") || text.contains("MULTI-MODULE"),
        "test role must include multi-module guidance"
    );
    assert!(
        !text.contains("as appropriate"),
        "test role must not contain the 'as appropriate' loophole"
    );
    // Must NOT have the old TEST AUTHORITY line that authorized broad runs.
    assert!(
        !text.contains("TEST AUTHORITY"),
        "test role must not still grant unconditional TEST AUTHORITY"
    );
}

#[test]
fn review_role_contract_forbids_unauthorized_workspace_wide_runs() {
    let text = role_contract_text("review");
    assert!(
        text.contains("ONLY allowed when the OPERATOR explicitly asked"),
        "review role must include the operator-only widening rule, got: {text}"
    );
    assert!(
        text.contains("cargo test --all") || text.contains("cargo clippy --workspace"),
        "review role must name workspace-wide commands as forbidden"
    );
    assert!(
        !text.contains("as appropriate"),
        "review role must not contain the 'as appropriate' loophole"
    );
    assert!(
        !text.contains("TEST AUTHORITY"),
        "review role must not still grant unconditional TEST AUTHORITY"
    );
}

#[test]
fn read_only_roles_forbid_all_verification_commands() {
    // propose / research / computational-research / judge / genome-reviewer
    // are read-only — they should never run any verification command, period.
    for role in [
        "propose",
        "research",
        "computational-research",
        "judge",
        "genome-reviewer",
    ] {
        let text = role_contract_text(role);
        assert!(
            forbids_verification_commands(&text),
            "read-only role '{role}' must explicitly forbid running tests/builds/lints/CI, got: {text}"
        );
    }
}

#[test]
fn default_role_contract_forbids_verification_unless_assigned() {
    let text = role_contract_text("some-unrecognised-future-role");
    assert!(
        forbids_verification_commands(&text),
        "default role contract must forbid verification commands by default, got: {text}"
    );
}

// ---------------------------------------------------------------------------
// ensure_proposer_task safety net — guarantees that a parallel-template
// general-mission plan never ends up with every agent assigned `integrate`.
// Without this, large-scope refactors (>15 files) trip the multi-integrator
// branch in build_planner_prompt and the LLM happily makes everyone a writer.
// ---------------------------------------------------------------------------

/// Build a minimal SwarmTask with the given id, agent, role, and writes flag.
/// Used by the proposer-safety-net tests below. Distinct from `make_task`
/// (defined earlier in this file) which sets `writes=false` unconditionally.
fn make_writer_task(id: &str, agent: &str, role: Option<&str>, writes: bool) -> SwarmTask {
    SwarmTask {
        id: id.into(),
        agent_id: agent.into(),
        role: role.map(str::to_string),
        title: format!("Task {id}"),
        task_prompt: format!("prompt {id}"),
        deps: Vec::new(),
        writes,
        artifacts: Vec::new(),
        done_when: None,
        state: SwarmTaskState::Pending,
        output: None,
        parsed_artifacts: None,
        expected_artifacts_missing: false,
        failed: false,
        retries: 0,
    }
}

/// Structurally-relevant fields of a task, used by `task_fingerprint` to
/// snapshot a plan for no-op assertions. Avoids needing `PartialEq` on
/// `SwarmTask` (which would also force it on every nested type).
type TaskFingerprint = (String, String, Option<String>, bool, Vec<String>);

/// Snapshot of the structurally-relevant fields of a task list, used to assert
/// that a no-op safety-net call leaves the plan untouched.
fn task_fingerprint(tasks: &[SwarmTask]) -> Vec<TaskFingerprint> {
    tasks
        .iter()
        .map(|t| {
            (
                t.id.clone(),
                t.agent_id.clone(),
                t.role.clone(),
                t.writes,
                t.deps.clone(),
            )
        })
        .collect()
}

#[test]
fn ensure_proposer_task_demotes_one_integrate_when_no_proposer_lane() {
    // Reproduces the user's bug: parallel template, general mission, large-scope
    // refactor where the planner emitted 4 integrate tasks (one per clone) and
    // no propose/recon lane. The safety net should demote exactly one task —
    // preferably one not on the designated integrator — and wire it as a dep
    // for the remaining integrate tasks.
    let mut tasks = vec![
        make_writer_task("t1", "integrator-agent", Some("integrate"), true),
        make_writer_task("t2", "clone-01", Some("integrate"), true),
        make_writer_task("t3", "clone-02", Some("integrate"), true),
        make_writer_task("t4", "clone-03", Some("integrate"), true),
    ];

    let warnings = ensure_proposer_task(
        &mut tasks,
        SwarmTemplate::Parallel,
        SwarmMissionKind::General,
        Some("integrator-agent"),
    );

    let propose: Vec<&SwarmTask> = tasks
        .iter()
        .filter(|t| t.role.as_deref() == Some("propose"))
        .collect();
    assert_eq!(
        propose.len(),
        1,
        "exactly one task should have been demoted to propose, tasks: {tasks:?}"
    );
    let demoted = propose[0];
    assert!(!demoted.writes, "demoted task must be read-only");
    assert_ne!(
        demoted.agent_id, "integrator-agent",
        "should prefer demoting a task NOT on the designated integrator agent"
    );
    let demoted_id = demoted.id.clone();

    let integrators: Vec<&SwarmTask> = tasks
        .iter()
        .filter(|t| t.role.as_deref() == Some("integrate"))
        .collect();
    assert_eq!(
        integrators.len(),
        3,
        "three integrate tasks should remain after demotion"
    );
    for task in &integrators {
        assert!(
            task.deps.contains(&demoted_id),
            "integrate task '{}' should depend on the demoted propose task '{demoted_id}', deps: {:?}",
            task.id,
            task.deps
        );
    }
    assert!(
        warnings.iter().any(|w| w.contains("demoted")),
        "safety net should emit a 'demoted' warning, got: {warnings:?}"
    );
}

#[test]
fn ensure_proposer_task_noop_when_propose_already_present() {
    let mut tasks = vec![
        make_writer_task("recon", "a1", Some("propose"), false),
        make_writer_task("impl-1", "a2", Some("integrate"), true),
        make_writer_task("impl-2", "a3", Some("integrate"), true),
    ];
    let before = task_fingerprint(&tasks);
    let warnings = ensure_proposer_task(
        &mut tasks,
        SwarmTemplate::Parallel,
        SwarmMissionKind::General,
        Some("a2"),
    );
    assert!(
        warnings.is_empty(),
        "no-op when propose lane already exists, got: {warnings:?}"
    );
    assert_eq!(
        task_fingerprint(&tasks),
        before,
        "task list should be unchanged when a propose lane already exists"
    );
}

#[test]
fn ensure_proposer_task_noop_when_only_one_integrate_task() {
    // Demoting would leave zero writers — bail out instead.
    let mut tasks = vec![
        make_writer_task("impl", "integrator", Some("integrate"), true),
        make_writer_task("test", "clone-01", Some("test"), false),
    ];
    let before = task_fingerprint(&tasks);
    let warnings = ensure_proposer_task(
        &mut tasks,
        SwarmTemplate::Parallel,
        SwarmMissionKind::General,
        Some("integrator"),
    );
    assert!(warnings.is_empty(), "single integrate must be left alone");
    assert_eq!(task_fingerprint(&tasks), before);
}

#[test]
fn ensure_proposer_task_noop_for_lab_template() {
    let mut tasks = vec![
        make_writer_task("impl-1", "a1", Some("integrate"), true),
        make_writer_task("impl-2", "a2", Some("integrate"), true),
    ];
    let before = task_fingerprint(&tasks);
    let warnings = ensure_proposer_task(
        &mut tasks,
        SwarmTemplate::Lab,
        SwarmMissionKind::General,
        Some("a1"),
    );
    assert!(
        warnings.is_empty(),
        "lab template handles single-writer via prompt; safety net is parallel-only"
    );
    assert_eq!(task_fingerprint(&tasks), before);
}

#[test]
fn ensure_proposer_task_noop_for_research_mission() {
    let mut tasks = vec![
        make_writer_task("impl-1", "a1", Some("integrate"), true),
        make_writer_task("impl-2", "a2", Some("integrate"), true),
    ];
    let before = task_fingerprint(&tasks);
    let warnings = ensure_proposer_task(
        &mut tasks,
        SwarmTemplate::Parallel,
        SwarmMissionKind::Research,
        Some("a1"),
    );
    assert!(
        warnings.is_empty(),
        "research missions already lean read-only; safety net is general-mission-only"
    );
    assert_eq!(task_fingerprint(&tasks), before);
}

#[test]
fn ensure_proposer_task_falls_back_when_all_integrates_on_integrator() {
    // Edge case: every integrate task is on the integrator agent (e.g. the
    // planner only handed work to one agent). The safety net should still fire
    // and demote the first integrate task — better one writer + one proposer
    // than two writers and zero recon.
    let mut tasks = vec![
        make_writer_task("t1", "integrator", Some("integrate"), true),
        make_writer_task("t2", "integrator", Some("integrate"), true),
    ];
    let warnings = ensure_proposer_task(
        &mut tasks,
        SwarmTemplate::Parallel,
        SwarmMissionKind::General,
        Some("integrator"),
    );
    assert_eq!(tasks[0].role.as_deref(), Some("propose"));
    assert!(!tasks[0].writes);
    assert_eq!(tasks[1].role.as_deref(), Some("integrate"));
    assert!(tasks[1].deps.contains(&"t1".to_string()));
    assert!(!warnings.is_empty());
}

// ---------------------------------------------------------------------------
// ensure_agent_coverage safety net — guarantees that a parallel-template
// plan never leaves a provisioned clone without a task. Without this, the
// LLM planner occasionally drops an agent it deems redundant, leaving the
// clone stuck at `swarm_pending` and stalling the swarm.
// ---------------------------------------------------------------------------

#[test]
fn ensure_agent_coverage_injects_review_task_for_uncovered_general_agent() {
    // Planner produced tasks for 2 of 3 available agents; the 3rd should get
    // a synthesized review task so it isn't left idle.
    let mut tasks = vec![
        make_writer_task("t1", "agent-a", Some("propose"), false),
        make_writer_task("t2", "agent-b", Some("integrate"), true),
    ];
    let available = vec![
        "agent-a".to_string(),
        "agent-b".to_string(),
        "agent-c".to_string(),
    ];
    let warnings = ensure_agent_coverage(
        &mut tasks,
        SwarmTemplate::Parallel,
        SwarmMissionKind::General,
        &available,
    );
    assert_eq!(tasks.len(), 3, "uncovered agent must receive a task");
    let injected = tasks.last().expect("at least one injected task");
    assert_eq!(injected.agent_id, "agent-c");
    assert_eq!(injected.role.as_deref(), Some("review"));
    assert!(!injected.writes, "injected task must be read-only");
    assert!(
        injected.deps.is_empty(),
        "injected task should not add deps"
    );
    assert!(
        warnings.iter().any(|w| w.contains("agent-c")),
        "warning should name the uncovered agent, got: {warnings:?}"
    );
}

#[test]
fn ensure_agent_coverage_uses_research_role_for_research_missions() {
    let mut tasks = vec![make_writer_task("t1", "agent-a", Some("research"), false)];
    let available = vec!["agent-a".to_string(), "agent-b".to_string()];
    let _warnings = ensure_agent_coverage(
        &mut tasks,
        SwarmTemplate::Parallel,
        SwarmMissionKind::Research,
        &available,
    );
    let injected = tasks.last().expect("one injected task");
    assert_eq!(injected.agent_id, "agent-b");
    assert_eq!(injected.role.as_deref(), Some("research"));
}

#[test]
fn ensure_agent_coverage_uses_computational_research_for_that_mission() {
    let mut tasks = vec![make_writer_task(
        "t1",
        "agent-a",
        Some(COMPUTATIONAL_RESEARCH_ROLE),
        false,
    )];
    let available = vec!["agent-a".to_string(), "agent-b".to_string()];
    let _warnings = ensure_agent_coverage(
        &mut tasks,
        SwarmTemplate::Parallel,
        SwarmMissionKind::ComputationalResearch,
        &available,
    );
    let injected = tasks.last().expect("one injected task");
    assert_eq!(injected.agent_id, "agent-b");
    assert_eq!(injected.role.as_deref(), Some(COMPUTATIONAL_RESEARCH_ROLE));
}

#[test]
fn ensure_agent_coverage_noop_when_all_agents_covered() {
    let mut tasks = vec![
        make_writer_task("t1", "agent-a", Some("propose"), false),
        make_writer_task("t2", "agent-b", Some("integrate"), true),
    ];
    let available = vec!["agent-a".to_string(), "agent-b".to_string()];
    let before = task_fingerprint(&tasks);
    let warnings = ensure_agent_coverage(
        &mut tasks,
        SwarmTemplate::Parallel,
        SwarmMissionKind::General,
        &available,
    );
    assert!(
        warnings.is_empty(),
        "no-op when every agent already has a task, got: {warnings:?}"
    );
    assert_eq!(task_fingerprint(&tasks), before);
}

#[test]
fn ensure_agent_coverage_noop_for_lab_template() {
    // Lab intentionally allows multiple tasks per agent and silent agents,
    // so the safety net should not fire.
    let mut tasks = vec![make_writer_task("t1", "agent-a", Some("integrate"), true)];
    let available = vec![
        "agent-a".to_string(),
        "agent-b".to_string(),
        "agent-c".to_string(),
    ];
    let before = task_fingerprint(&tasks);
    let warnings = ensure_agent_coverage(
        &mut tasks,
        SwarmTemplate::Lab,
        SwarmMissionKind::General,
        &available,
    );
    assert!(
        warnings.is_empty(),
        "lab template opts out of coverage fill"
    );
    assert_eq!(task_fingerprint(&tasks), before);
}

#[test]
fn ensure_agent_coverage_noop_for_bulk_template() {
    // Bulk has its own validate_bulk_plan check for proposer/judge/integrate
    // roles; ensure_agent_coverage should not pile on.
    let mut tasks = vec![
        make_writer_task("propose-01", "agent-a", Some("propose"), false),
        make_writer_task("judge", "agent-b", Some("judge"), false),
        make_writer_task("integrate", "agent-c", Some("integrate"), true),
    ];
    let available = vec![
        "agent-a".to_string(),
        "agent-b".to_string(),
        "agent-c".to_string(),
        "agent-d".to_string(),
    ];
    let before = task_fingerprint(&tasks);
    let warnings = ensure_agent_coverage(
        &mut tasks,
        SwarmTemplate::Bulk,
        SwarmMissionKind::General,
        &available,
    );
    assert!(
        warnings.is_empty(),
        "bulk template opts out of coverage fill"
    );
    assert_eq!(task_fingerprint(&tasks), before);
}

#[test]
fn ensure_agent_coverage_avoids_id_collision_with_existing_tasks() {
    // If the planner already used `cover-01` as a task id, the injected task
    // must pick a different id instead of silently dropping via duplicate-id
    // filter downstream.
    let mut tasks = vec![
        make_writer_task("cover-01", "agent-a", Some("propose"), false),
        make_writer_task("cover-02", "agent-b", Some("integrate"), true),
    ];
    let available = vec![
        "agent-a".to_string(),
        "agent-b".to_string(),
        "agent-c".to_string(),
    ];
    let _warnings = ensure_agent_coverage(
        &mut tasks,
        SwarmTemplate::Parallel,
        SwarmMissionKind::General,
        &available,
    );
    let injected = tasks.last().expect("one injected task");
    assert_eq!(injected.agent_id, "agent-c");
    assert_ne!(injected.id, "cover-01");
    assert_ne!(injected.id, "cover-02");
}

// ---------------------------------------------------------------------------
// assign_clone_roles_for_parallel_coverage — proactively assigns role hints
// to fresh clones so the parallel-template swarm covers a propose lane and a
// review/test lane, mirroring the lab template's read-only worker structure.
// The user's escape hatch: setting the planner role to `all` (or leaving it
// unset) opts out of this enforcement and lets the LLM decide everything.
// ---------------------------------------------------------------------------

/// Build a fresh AppState with the given lanes and role hints already set up.
/// Used by the clone-coverage tests below.
fn make_coverage_state(lanes: &[(&str, &str)], role_hints: &[(&str, &str)]) -> AppState {
    let mut state = new_state();
    state.agents.messages.clear();
    state.agents.missions.clear();
    state.agents.agents.clear();
    for (id, role) in lanes {
        state.agents.agents.push(make_lane(id, role));
    }
    for (id, role) in role_hints {
        state
            .agents
            .swarm_role_by_agent_id
            .insert((*id).into(), (*role).into());
    }
    state
}

#[test]
fn coverage_assigns_propose_and_review_when_planner_has_role_hint() {
    // Planner has role=integrate (a deliberate non-`all` hint), 3 clones, no
    // priority agents. Expected: clones get propose + review (one each), one
    // clone left unassigned (the integrator candidate).
    let mut state = make_coverage_state(
        &[
            ("planner", "p"),
            ("planner#swarm-mis-001-clone-01", "c1"),
            ("planner#swarm-mis-001-clone-02", "c2"),
            ("planner#swarm-mis-001-clone-03", "c3"),
        ],
        &[("planner", "integrate")],
    );
    let agents = vec![
        "planner".to_string(),
        "planner#swarm-mis-001-clone-01".to_string(),
        "planner#swarm-mis-001-clone-02".to_string(),
        "planner#swarm-mis-001-clone-03".to_string(),
    ];
    let assignments = assign_clone_roles_for_parallel_coverage(
        &mut state,
        SwarmTemplate::Parallel,
        "planner",
        Some("planner#swarm-mis-001-clone-01"),
        &agents,
    );

    let map = &state.agents.swarm_role_by_agent_id;
    let roles: Vec<Option<&str>> = agents
        .iter()
        .map(|id| map.get(id).map(String::as_str))
        .collect();
    // planner stays integrate; clone-01 (integrator) is untouched; clone-02
    // and clone-03 get propose and review (in order).
    assert_eq!(roles[0], Some("integrate"));
    assert_eq!(
        roles[1], None,
        "integrator clone must NOT be assigned a role"
    );
    assert_eq!(roles[2], Some("propose"));
    assert_eq!(roles[3], Some("review"));
    assert_eq!(assignments.len(), 2);
}

#[test]
fn coverage_assigns_when_planner_role_is_all() {
    // Even when the planner is explicitly `all`, the helper still assigns
    // sensible defaults so the swarm has propose + review/test coverage.
    // The planner is always the synthesizer; we want a balanced worker mix.
    let mut state = make_coverage_state(
        &[
            ("planner", "p"),
            ("planner#swarm-mis-001-clone-01", "c1"),
            ("planner#swarm-mis-001-clone-02", "c2"),
            ("planner#swarm-mis-001-clone-03", "c3"),
        ],
        &[("planner", "all")],
    );
    let agents = vec![
        "planner".to_string(),
        "planner#swarm-mis-001-clone-01".to_string(),
        "planner#swarm-mis-001-clone-02".to_string(),
        "planner#swarm-mis-001-clone-03".to_string(),
    ];
    let assignments = assign_clone_roles_for_parallel_coverage(
        &mut state,
        SwarmTemplate::Parallel,
        "planner",
        // clone-01 is the integrator → excluded from coverage assignment.
        Some("planner#swarm-mis-001-clone-01"),
        &agents,
    );
    assert_eq!(assignments.len(), 2);
    assert_eq!(
        state
            .agents
            .swarm_role_by_agent_id
            .get("planner#swarm-mis-001-clone-02")
            .map(String::as_str),
        Some("propose")
    );
    assert_eq!(
        state
            .agents
            .swarm_role_by_agent_id
            .get("planner#swarm-mis-001-clone-03")
            .map(String::as_str),
        Some("review")
    );
}

#[test]
fn coverage_assigns_when_planner_role_unset() {
    // Default state — no role hints anywhere. The helper still ensures the
    // swarm covers propose + review/test by assigning roles to clones.
    let mut state = make_coverage_state(
        &[
            ("planner", "p"),
            ("planner#swarm-mis-001-clone-01", "c1"),
            ("planner#swarm-mis-001-clone-02", "c2"),
            ("planner#swarm-mis-001-clone-03", "c3"),
        ],
        &[],
    );
    let agents = vec![
        "planner".to_string(),
        "planner#swarm-mis-001-clone-01".to_string(),
        "planner#swarm-mis-001-clone-02".to_string(),
        "planner#swarm-mis-001-clone-03".to_string(),
    ];
    let assignments = assign_clone_roles_for_parallel_coverage(
        &mut state,
        SwarmTemplate::Parallel,
        "planner",
        Some("planner#swarm-mis-001-clone-01"),
        &agents,
    );
    assert_eq!(assignments.len(), 2);
    let assigned_roles: Vec<&str> = assignments.iter().map(|(_, r)| *r).collect();
    assert!(assigned_roles.contains(&"propose"));
    assert!(assigned_roles.contains(&"review"));
}

#[test]
fn coverage_noop_when_priority_agents_already_cover_both_slots() {
    let mut state = make_coverage_state(
        &[
            ("planner", "p"),
            ("priority-a", "a"),
            ("priority-b", "b"),
            ("planner#swarm-mis-001-clone-01", "c1"),
        ],
        &[
            ("planner", "integrate"),
            ("priority-a", "propose"),
            ("priority-b", "review"),
        ],
    );
    let agents = vec![
        "planner".to_string(),
        "priority-a".to_string(),
        "priority-b".to_string(),
        "planner#swarm-mis-001-clone-01".to_string(),
    ];
    let assignments = assign_clone_roles_for_parallel_coverage(
        &mut state,
        SwarmTemplate::Parallel,
        "planner",
        Some("planner#swarm-mis-001-clone-01"),
        &agents,
    );
    assert!(assignments.is_empty());
    assert!(!state
        .agents
        .swarm_role_by_agent_id
        .contains_key("planner#swarm-mis-001-clone-01"));
}

#[test]
fn coverage_assigns_only_review_when_priority_already_covers_propose() {
    let mut state = make_coverage_state(
        &[
            ("planner", "p"),
            ("priority-a", "a"),
            ("planner#swarm-mis-001-clone-01", "c1"),
            ("planner#swarm-mis-001-clone-02", "c2"),
        ],
        &[("planner", "integrate"), ("priority-a", "propose")],
    );
    let agents = vec![
        "planner".to_string(),
        "priority-a".to_string(),
        "planner#swarm-mis-001-clone-01".to_string(),
        "planner#swarm-mis-001-clone-02".to_string(),
    ];
    let assignments = assign_clone_roles_for_parallel_coverage(
        &mut state,
        SwarmTemplate::Parallel,
        "planner",
        Some("planner#swarm-mis-001-clone-01"),
        &agents,
    );
    assert_eq!(assignments.len(), 1);
    assert_eq!(assignments[0].1, "review");
    assert_eq!(
        state
            .agents
            .swarm_role_by_agent_id
            .get("planner#swarm-mis-001-clone-02")
            .map(String::as_str),
        Some("review")
    );
}

#[test]
fn coverage_research_role_satisfies_propose_slot() {
    let mut state = make_coverage_state(
        &[
            ("planner", "p"),
            ("priority-a", "a"),
            ("planner#swarm-mis-001-clone-01", "c1"),
            ("planner#swarm-mis-001-clone-02", "c2"),
        ],
        &[("planner", "integrate"), ("priority-a", "research")],
    );
    let agents = vec![
        "planner".to_string(),
        "priority-a".to_string(),
        "planner#swarm-mis-001-clone-01".to_string(),
        "planner#swarm-mis-001-clone-02".to_string(),
    ];
    let assignments = assign_clone_roles_for_parallel_coverage(
        &mut state,
        SwarmTemplate::Parallel,
        "planner",
        Some("planner#swarm-mis-001-clone-01"),
        &agents,
    );
    // research counts as propose — only review needs to be added.
    assert_eq!(assignments.len(), 1);
    assert_eq!(assignments[0].1, "review");
}

#[test]
fn coverage_test_role_satisfies_review_slot() {
    let mut state = make_coverage_state(
        &[
            ("planner", "p"),
            ("priority-a", "a"),
            ("planner#swarm-mis-001-clone-01", "c1"),
            ("planner#swarm-mis-001-clone-02", "c2"),
        ],
        &[("planner", "integrate"), ("priority-a", "test")],
    );
    let agents = vec![
        "planner".to_string(),
        "priority-a".to_string(),
        "planner#swarm-mis-001-clone-01".to_string(),
        "planner#swarm-mis-001-clone-02".to_string(),
    ];
    let assignments = assign_clone_roles_for_parallel_coverage(
        &mut state,
        SwarmTemplate::Parallel,
        "planner",
        Some("planner#swarm-mis-001-clone-01"),
        &agents,
    );
    // test counts as review/test — only propose needs to be added.
    assert_eq!(assignments.len(), 1);
    assert_eq!(assignments[0].1, "propose");
}

#[test]
fn coverage_noop_for_lab_template() {
    let mut state = make_coverage_state(
        &[
            ("planner", "p"),
            ("planner#swarm-mis-001-clone-01", "c1"),
            ("planner#swarm-mis-001-clone-02", "c2"),
        ],
        &[("planner", "integrate")],
    );
    let agents = vec![
        "planner".to_string(),
        "planner#swarm-mis-001-clone-01".to_string(),
        "planner#swarm-mis-001-clone-02".to_string(),
    ];
    let assignments = assign_clone_roles_for_parallel_coverage(
        &mut state,
        SwarmTemplate::Lab,
        "planner",
        None,
        &agents,
    );
    assert!(
        assignments.is_empty(),
        "lab template handles role coverage via fallback_tasks; helper is parallel-only"
    );
}

#[test]
fn coverage_excludes_integrator_clone_from_assignment() {
    // The integrator must stay a writer, so the helper must NOT assign a
    // read-only role to the designated integrator clone.
    let mut state = make_coverage_state(
        &[
            ("planner", "p"),
            ("planner#swarm-mis-001-clone-01", "c1"),
            ("planner#swarm-mis-001-clone-02", "c2"),
            ("planner#swarm-mis-001-clone-03", "c3"),
        ],
        &[("planner", "integrate")],
    );
    let agents = vec![
        "planner".to_string(),
        "planner#swarm-mis-001-clone-01".to_string(),
        "planner#swarm-mis-001-clone-02".to_string(),
        "planner#swarm-mis-001-clone-03".to_string(),
    ];
    let assignments = assign_clone_roles_for_parallel_coverage(
        &mut state,
        SwarmTemplate::Parallel,
        "planner",
        Some("planner#swarm-mis-001-clone-02"),
        &agents,
    );
    let assigned_ids: Vec<&str> = assignments.iter().map(|(id, _)| id.as_str()).collect();
    assert!(
        !assigned_ids.contains(&"planner#swarm-mis-001-clone-02"),
        "integrator clone must be excluded from coverage assignments, got: {assigned_ids:?}"
    );
    assert_eq!(assignments.len(), 2);
}

#[test]
fn coverage_does_not_overwrite_clone_with_existing_role_hint() {
    // A clone that already has a direct role hint (e.g. from a follow-up
    // dispatch re-using clone IDs) must not be overwritten.
    let mut state = make_coverage_state(
        &[
            ("planner", "p"),
            ("planner#swarm-mis-001-clone-01", "c1"),
            ("planner#swarm-mis-001-clone-02", "c2"),
            ("planner#swarm-mis-001-clone-03", "c3"),
        ],
        &[
            ("planner", "integrate"),
            ("planner#swarm-mis-001-clone-01", "judge"),
        ],
    );
    let agents = vec![
        "planner".to_string(),
        "planner#swarm-mis-001-clone-01".to_string(),
        "planner#swarm-mis-001-clone-02".to_string(),
        "planner#swarm-mis-001-clone-03".to_string(),
    ];
    let assignments = assign_clone_roles_for_parallel_coverage(
        &mut state,
        SwarmTemplate::Parallel,
        "planner",
        None,
        &agents,
    );
    // clone-01 already has judge — must remain judge. propose + review go to
    // clone-02 and clone-03.
    assert_eq!(
        state
            .agents
            .swarm_role_by_agent_id
            .get("planner#swarm-mis-001-clone-01")
            .map(String::as_str),
        Some("judge")
    );
    assert_eq!(assignments.len(), 2);
    let assigned_ids: Vec<&str> = assignments.iter().map(|(id, _)| id.as_str()).collect();
    assert!(assigned_ids.contains(&"planner#swarm-mis-001-clone-02"));
    assert!(assigned_ids.contains(&"planner#swarm-mis-001-clone-03"));
}

#[test]
fn coverage_partial_assignment_when_not_enough_clones() {
    // Only 1 unassigned clone available but 2 roles needed — fill the first
    // (propose) and skip the second.
    let mut state = make_coverage_state(
        &[
            ("planner", "p"),
            ("planner#swarm-mis-001-clone-01", "c1"),
            ("planner#swarm-mis-001-clone-02", "c2"),
        ],
        &[("planner", "integrate")],
    );
    let agents = vec![
        "planner".to_string(),
        "planner#swarm-mis-001-clone-01".to_string(),
        "planner#swarm-mis-001-clone-02".to_string(),
    ];
    let assignments = assign_clone_roles_for_parallel_coverage(
        &mut state,
        SwarmTemplate::Parallel,
        "planner",
        // clone-01 is the integrator → only clone-02 is assignable.
        Some("planner#swarm-mis-001-clone-01"),
        &agents,
    );
    assert_eq!(assignments.len(), 1);
    assert_eq!(assignments[0].1, "propose");
}

// -- Gate retry dispatch -----------------------------------------------------

fn make_verifying_run_with_fail_report() -> SwarmRun {
    SwarmRun {
        mission_id: "mis-retry".into(),
        root_prompt: "root".into(),
        template: SwarmTemplate::Lab,
        mission_kind: SwarmMissionKind::General,
        planner_agent_id: "planner".into(),
        integrator_agent_id: Some("integ".into()),
        integrator_locked: false,
        verifier_agent_id: Some("verify".into()),
        gate_bundle: Some(GateBundle::Rust),
        gate_custom: None,
        gate_selection: "auto:rust-ci".into(),
        agent_ids: vec!["planner".into(), "integ".into(), "verify".into()],
        stage: SwarmStage::Verifying,
        tasks: vec![SwarmTask {
            id: "integrate".into(),
            agent_id: "integ".into(),
            role: Some("integrate".into()),
            title: "Integrate".into(),
            task_prompt: "integ".into(),
            deps: Vec::new(),
            writes: true,
            artifacts: Vec::new(),
            done_when: None,
            state: SwarmTaskState::Done,
            output: Some("done".into()),
            parsed_artifacts: None,
            expected_artifacts_missing: false,
            failed: false,
            retries: 0,
        }],
        synthesis_prompt: None,
        gate_output: Some("prior".into()),
        gate_report: None,
        genome_gate_results: Some("stale".into()),
        genome_gate_pending: None,
        genome_review_pending: None,
        report_status: None,
        report_output: None,
        scope_files: Vec::new(),
        initial_genome_baselines: std::collections::HashMap::new(),
        gate_retry_count: 0,
    }
}

fn make_state_for_retry() -> nit_core::AppState {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let editor = Buffer::empty("editor", None);
    let notes = Buffer::empty("notes", None);
    let mut state = nit_core::AppState::new(root, editor, notes);
    state.agents.messages.clear();
    state.agents.missions.clear();
    state.agents.agents.clear();
    state.agents.agents.push(make_lane("planner", "planner"));
    state.agents.agents.push(make_lane("integ", "integrate"));
    state.agents.agents.push(make_lane("verify", "verify"));
    state
}

const FAILING_GATE_REPORT: &str = "```json\n{\"overall_ok\":false,\"gates\":[{\"name\":\"fmt\",\"command\":\"cargo fmt -- --check\",\"ok\":false,\"status\":\"fail\",\"notes\":\"4 files need formatting\"}]}\n```";

#[test]
fn gate_fail_dispatches_retry_to_integrator() {
    let mut state = make_state_for_retry();
    let run = make_verifying_run_with_fail_report();
    let mut runtime = SwarmRuntime::default();
    runtime.runs.insert("mis-retry".into(), run);

    let event = AgentBusEvent::TurnCompleted {
        agent_id: "verify".into(),
        mission_id: Some("mis-retry".into()),
        thread_id: None,
        token_count: None,
        message: FAILING_GATE_REPORT.into(),
    };
    let outcome = runtime.handle_event_outcome(&mut state, &event);

    assert_eq!(outcome.dispatches.len(), 1, "expected retry dispatch");
    let dispatch = &outcome.dispatches[0];
    assert_eq!(dispatch.agent_id, "integ");
    assert_eq!(dispatch.task_role.as_deref(), Some("integrate"));
    assert!(dispatch.prompt.contains("fmt"));
    assert!(dispatch.prompt.contains("4 files need formatting"));

    let run = runtime.runs.get("mis-retry").expect("run still active");
    assert!(matches!(run.stage, SwarmStage::Executing));
    assert_eq!(run.gate_retry_count, 1);
    assert!(run.gate_report.is_none());
    assert!(run.gate_output.is_none());
    assert!(run.genome_gate_results.is_none());
    assert!(run.tasks.iter().any(|t| t.id == "gate-retry-1"));
}

#[test]
fn gate_fail_skips_retry_when_limit_reached() {
    let mut state = make_state_for_retry();
    let mut run = make_verifying_run_with_fail_report();
    run.gate_retry_count = state.settings.swarm.gate_retry_limit;
    let mut runtime = SwarmRuntime::default();
    runtime.runs.insert("mis-retry".into(), run);

    let event = AgentBusEvent::TurnCompleted {
        agent_id: "verify".into(),
        mission_id: Some("mis-retry".into()),
        thread_id: None,
        token_count: None,
        message: FAILING_GATE_REPORT.into(),
    };
    let outcome = runtime.handle_event_outcome(&mut state, &event);

    assert_eq!(outcome.dispatches.len(), 1);
    assert_eq!(outcome.dispatches[0].agent_id, "planner");
    let run = runtime.runs.get("mis-retry").expect("run still active");
    assert!(matches!(run.stage, SwarmStage::Synthesizing));
}

#[test]
fn gate_fail_skips_retry_when_limit_is_zero() {
    let mut state = make_state_for_retry();
    state.settings.swarm.gate_retry_limit = 0;
    let run = make_verifying_run_with_fail_report();
    let mut runtime = SwarmRuntime::default();
    runtime.runs.insert("mis-retry".into(), run);

    let event = AgentBusEvent::TurnCompleted {
        agent_id: "verify".into(),
        mission_id: Some("mis-retry".into()),
        thread_id: None,
        token_count: None,
        message: FAILING_GATE_REPORT.into(),
    };
    let outcome = runtime.handle_event_outcome(&mut state, &event);
    assert_eq!(outcome.dispatches[0].agent_id, "planner");
    let run = runtime.runs.get("mis-retry").expect("run still active");
    assert!(matches!(run.stage, SwarmStage::Synthesizing));
    assert_eq!(run.gate_retry_count, 0);
}

#[test]
fn gate_pass_does_not_retry() {
    let mut state = make_state_for_retry();
    let run = make_verifying_run_with_fail_report();
    let mut runtime = SwarmRuntime::default();
    runtime.runs.insert("mis-retry".into(), run);

    let pass_report = "```json\n{\"overall_ok\":true,\"gates\":[{\"name\":\"fmt\",\"command\":\"cargo fmt -- --check\",\"ok\":true}]}\n```";
    let event = AgentBusEvent::TurnCompleted {
        agent_id: "verify".into(),
        mission_id: Some("mis-retry".into()),
        thread_id: None,
        token_count: None,
        message: pass_report.into(),
    };
    let outcome = runtime.handle_event_outcome(&mut state, &event);
    assert_eq!(outcome.dispatches[0].agent_id, "planner");
    let run = runtime.runs.get("mis-retry").expect("run still active");
    assert!(matches!(run.stage, SwarmStage::Synthesizing));
    assert_eq!(run.gate_retry_count, 0);
}

fn make_run_with_tasks(template: SwarmTemplate, tasks: Vec<SwarmTask>) -> SwarmRun {
    SwarmRun {
        mission_id: "mis-test".into(),
        root_prompt: "root".into(),
        template,
        mission_kind: SwarmMissionKind::General,
        planner_agent_id: "planner".into(),
        integrator_agent_id: None,
        integrator_locked: false,
        verifier_agent_id: None,
        gate_bundle: None,
        gate_custom: None,
        gate_selection: "auto:none".into(),
        agent_ids: Vec::new(),
        stage: SwarmStage::Executing,
        tasks,
        synthesis_prompt: None,
        gate_output: None,
        gate_report: None,
        genome_gate_results: None,
        genome_gate_pending: None,
        genome_review_pending: None,
        report_status: None,
        report_output: None,
        scope_files: Vec::new(),
        initial_genome_baselines: std::collections::HashMap::new(),
        gate_retry_count: 0,
    }
}

#[test]
fn parallel_ensure_deps_resolve_redirects_unresolved_integrator_to_proposers() {
    let mut tasks = vec![
        make_task("propose-survey", "a1", Some("propose"), vec![]),
        SwarmTask {
            writes: true,
            ..make_task("integrate", "a2", Some("integrate"), vec!["judge"])
        },
    ];
    let repairs = ensure_deps_resolve(&mut tasks, SwarmTemplate::Parallel);
    assert_eq!(repairs.len(), 1);
    let integrate = tasks.iter().find(|t| t.id == "integrate").unwrap();
    assert_eq!(integrate.deps, vec!["propose-survey".to_string()]);
}

#[test]
fn lab_ensure_deps_resolve_is_noop() {
    let mut tasks = vec![
        make_task("propose-survey", "a1", Some("propose"), vec![]),
        SwarmTask {
            writes: true,
            ..make_task("integrate", "a2", Some("integrate"), vec!["judge"])
        },
    ];
    let before = tasks
        .iter()
        .find(|t| t.id == "integrate")
        .unwrap()
        .deps
        .clone();
    let repairs = ensure_deps_resolve(&mut tasks, SwarmTemplate::Lab);
    assert!(repairs.is_empty());
    let after = tasks
        .iter()
        .find(|t| t.id == "integrate")
        .unwrap()
        .deps
        .clone();
    assert_eq!(before, after);
}

#[test]
fn collect_unresolved_deps_walks_all_tasks() {
    let tasks = vec![
        make_task("a", "ag", Some("propose"), vec![]),
        make_task("b", "ag", Some("integrate"), vec!["missing-1", "a"]),
        make_task("c", "ag", Some("integrate"), vec!["missing-2"]),
    ];
    let run = make_run_with_tasks(SwarmTemplate::Parallel, tasks);
    let unresolved = collect_unresolved_deps(&run);
    assert_eq!(unresolved.len(), 2);
    let pairs: Vec<(String, String)> = unresolved
        .iter()
        .map(|u| (u.task_id.clone(), u.missing_dep.clone()))
        .collect();
    assert!(pairs.contains(&("b".to_string(), "missing-1".to_string())));
    assert!(pairs.contains(&("c".to_string(), "missing-2".to_string())));
}

#[test]
fn collect_unresolved_deps_empty_when_all_resolve() {
    let tasks = vec![
        make_task("a", "ag", Some("propose"), vec![]),
        make_task("b", "ag", Some("integrate"), vec!["a"]),
        make_task("c", "ag", Some("integrate"), vec!["a", "b"]),
    ];
    let run = make_run_with_tasks(SwarmTemplate::Parallel, tasks);
    assert!(collect_unresolved_deps(&run).is_empty());
}

#[test]
fn detect_incomplete_signoff_accepts_output_with_sentinel() {
    let message =
        "Did the work.\n\n```json\n{\"type\":\"swarm_artifacts\"}\n```\n\n<SWARM_TASK_COMPLETE>\n";
    assert!(detect_incomplete_signoff(message).is_none());
}

#[test]
fn detect_incomplete_signoff_flags_output_without_sentinel() {
    let message = "Did the work.\n\n```json\n{\"type\":\"swarm_artifacts\"}\n```\n";
    let reason = detect_incomplete_signoff(message).expect("flagged");
    assert!(reason.contains("sentinel") || reason.contains("approval"));
}

#[test]
fn detect_incomplete_signoff_flags_ask_for_approval_style_ending() {
    // Pre-sentinel deployment: no sentinel + human-style question tail.
    let message = "Completed swarm.rs refactor. 441 tests pass.\n\n\
                   Remaining: app/mod.rs. Want me to proceed, or pause here so you can review?";
    let reason = detect_incomplete_signoff(message).expect("flagged");
    assert!(reason.contains("approval") || reason.contains("sentinel"));
}

#[test]
fn detect_incomplete_signoff_flags_trailing_question_mark() {
    let message = "Here's what I found. Should I continue?";
    assert!(detect_incomplete_signoff(message).is_some());
}

#[test]
fn detect_incomplete_signoff_flags_empty_output() {
    assert!(detect_incomplete_signoff("").is_some());
    assert!(detect_incomplete_signoff("   \n\n").is_some());
}

#[test]
fn detect_incomplete_signoff_ignores_early_interrogatives_in_body() {
    // A proposer legitimately discussing "should we do X" in the middle of a
    // long output should NOT trip the detector — only tail prose matters.
    let mut message = String::from("Exploring options. Should we split this file?\n");
    for _ in 0..30 {
        message.push_str("Body content that doesn't ask anything.\n");
    }
    message.push_str("\n```json\n{\"type\":\"swarm_artifacts\"}\n```\n<SWARM_TASK_COMPLETE>\n");
    assert!(detect_incomplete_signoff(&message).is_none());
}

// --- swarm_intended_size ---------------------------------------------------
//
// Underpins the "requested X, started Y" clamp message in chat_input. We
// need to detect three classes of clamp:
//   * Count(n) where n exceeds the FD ceiling     -> fd-bound clamp
//   * Count(n) where n exceeds the roster pool    -> pool-bound clamp
//   * All on a host where pool > FD ceiling       -> fd-bound clamp
// The helper only computes the *intended* count; the comparison against
// `started` lives at the call site in chat_input.

fn make_lane_for_intended_size(id: &str, kind: AgentLaneKind) -> AgentLane {
    AgentLane {
        id: id.into(),
        role: id.into(),
        lane: id.into(),
        kind,
        status: nit_core::AgentStatus::Idle,
        heartbeat_age_secs: 0,
        queue_len: 0,
        current_mission: None,
        shadow: false,
        last_message: String::new(),
    }
}

#[test]
fn intended_size_count_returns_user_request() {
    let state = new_state();
    assert_eq!(swarm_intended_size(&state, SwarmSize::Count(100)), 100);
    assert_eq!(swarm_intended_size(&state, SwarmSize::Count(1)), 1);
    // Zero coerces up to 1: a swarm of zero agents would be a no-op, so
    // the intended count is at least the planner.
    assert_eq!(swarm_intended_size(&state, SwarmSize::Count(0)), 1);
}

#[test]
fn intended_size_default_returns_constant() {
    let state = new_state();
    // SwarmSize::Default should resolve to the static DEFAULT_SWARM_SIZE
    // (currently 4) regardless of roster contents.
    assert_eq!(swarm_intended_size(&state, SwarmSize::Default), 4);
}

#[test]
fn intended_size_all_counts_eligible_lanes() {
    let mut state = new_state();
    state.agents.agents = vec![
        make_lane_for_intended_size("codex-1", AgentLaneKind::Codex),
        make_lane_for_intended_size("codex-2", AgentLaneKind::Codex),
        make_lane_for_intended_size("claude-1", AgentLaneKind::Claude),
        // Mock lane is not codex/claude → excluded from the swarm pool.
        make_lane_for_intended_size("local", AgentLaneKind::Mock),
    ];
    assert_eq!(swarm_intended_size(&state, SwarmSize::All), 3);
}

#[test]
fn intended_size_all_excludes_swarm_and_chat_clones() {
    let mut state = new_state();
    state.agents.agents = vec![
        make_lane_for_intended_size("codex-1", AgentLaneKind::Codex),
        make_lane_for_intended_size("codex-1#swarm-mis-001-clone-01", AgentLaneKind::Codex),
        make_lane_for_intended_size("codex-1#chat-clone-02", AgentLaneKind::Codex),
        make_lane_for_intended_size("claude-1", AgentLaneKind::Claude),
    ];
    // Only the two real lanes count — clones are not part of the
    // "intended pool" because the planner spawns them on demand.
    assert_eq!(swarm_intended_size(&state, SwarmSize::All), 2);
}

#[test]
fn intended_size_all_clamps_empty_roster_to_one() {
    let state = new_state(); // no agents
                             // "all" on an empty roster returns 1 so the comparison
                             // (intended > started) doesn't underflow downstream.
    assert_eq!(swarm_intended_size(&state, SwarmSize::All), 1);
}

// --- per_dep_budget --------------------------------------------------------
//
// The DAG dashboard surfaces the per-dep character budget so operators see
// when their bulk proposers are getting truncated. These tests pin the
// formula so any tweak to the underlying constants surfaces in every
// consumer that depends on them.

#[test]
fn per_dep_budget_caps_at_full_ceiling_for_few_deps() {
    // 1 dep on a full-output role gets the per-dep ceiling (48k), not the
    // total budget (240k). The min() in the formula enforces this.
    assert_eq!(per_dep_budget(Some("integrate"), false, 1), 48_000);
    assert_eq!(per_dep_budget(Some("judge"), false, 1), 48_000);
    assert_eq!(per_dep_budget(Some("integrate"), false, 5), 48_000);
}

#[test]
fn per_dep_budget_splits_total_above_5_deps() {
    // Past 5 deps the total budget (240k) starts dividing. 6 → 40k each,
    // 12 → 20k, 50 → 4.8k, 256 → ~937 chars.
    assert_eq!(per_dep_budget(Some("integrate"), false, 6), 40_000);
    assert_eq!(per_dep_budget(Some("integrate"), false, 12), 20_000);
    assert_eq!(per_dep_budget(Some("integrate"), false, 50), 4_800);
    assert!(per_dep_budget(Some("integrate"), false, 256) < 1_000);
}

#[test]
fn per_dep_budget_treats_writes_as_full_output() {
    // A custom write-role task (writes=true, role unknown) shares the same
    // budget path as judge/integrate.
    assert_eq!(
        per_dep_budget(Some("custom-write"), true, 12),
        per_dep_budget(Some("integrate"), false, 12)
    );
}

#[test]
fn per_dep_budget_uses_compact_cap_for_non_full_roles() {
    // Compact-artifact roles (propose, review, test, …) get a flat 8k per
    // dep regardless of fan-in, because their payloads are summarised.
    assert_eq!(per_dep_budget(Some("propose"), false, 1), 8_000);
    assert_eq!(per_dep_budget(Some("propose"), false, 50), 8_000);
    assert_eq!(per_dep_budget(Some("review"), false, 100), 8_000);
    assert_eq!(per_dep_budget(None, false, 50), 8_000);
}

#[test]
fn per_dep_budget_handles_zero_deps_safely() {
    // No deps → effective dep_count of 1 (the .max(1) in the formula).
    // No panic, returns the per-dep ceiling.
    assert_eq!(per_dep_budget(Some("integrate"), false, 0), 48_000);
}

#[test]
fn task_uses_full_output_budget_classifies_correctly() {
    assert!(task_uses_full_output_budget(Some("judge"), false));
    assert!(task_uses_full_output_budget(Some("integrate"), false));
    assert!(task_uses_full_output_budget(Some("anything"), true));
    assert!(!task_uses_full_output_budget(Some("propose"), false));
    assert!(!task_uses_full_output_budget(Some("review"), false));
    assert!(!task_uses_full_output_budget(None, false));
}

// --- abort_mission / abort_all --------------------------------------------
//
// The abort feature must: return the agent ids the caller should send
// CancelTurn to, mark the mission ABORTED in the run state, drain
// queued turns from `state.agents`, push a system alert with
// SYSTEM_ALERT_KIND so the chat console renders it, and remain
// idempotent for unknown / completed missions.

fn lane_for_abort(id: &str) -> AgentLane {
    AgentLane {
        id: id.into(),
        role: id.into(),
        lane: id.into(),
        kind: AgentLaneKind::Codex,
        status: nit_core::AgentStatus::Idle,
        heartbeat_age_secs: 0,
        queue_len: 0,
        current_mission: Some("mis-001".into()),
        shadow: false,
        last_message: String::new(),
    }
}

/// Builds a `SwarmRuntime` (via the existing `test_runtime_with_running_tasks`
/// fixture) and seeds the AppState with matching agent lanes + a mission
/// record so the abort path has everything to operate on.
fn build_active_swarm_run(
    state: &mut nit_core::AppState,
    mission_id: &str,
    agent_ids: &[&str],
) -> SwarmRuntime {
    for id in agent_ids {
        state.agents.agents.push(lane_for_abort(id));
    }
    state.agents.missions.push(nit_core::MissionRecord {
        id: mission_id.into(),
        title: "test mission".into(),
        phase: nit_core::MissionPhase::Execute,
        status: "in progress".into(),
        swarm: true,
        updated_at: "now".into(),
        assigned_agents: agent_ids.iter().map(|s| s.to_string()).collect(),
    });
    test_runtime_with_running_tasks(
        mission_id,
        &agent_ids
            .iter()
            .enumerate()
            .map(|(i, id)| (*id, if i == 0 { "propose" } else { "integrate" }))
            .collect::<Vec<_>>(),
    )
}

#[test]
fn abort_mission_returns_agent_ids_and_marks_aborted() {
    let mut state = new_state();
    let mut runtime = build_active_swarm_run(&mut state, "mis-001", &["a-1", "a-2", "a-3"]);

    let agents = runtime.abort_mission(&mut state, "mis-001");
    assert_eq!(agents.len(), 3, "all assigned agents must be returned");
    for id in &["a-1", "a-2", "a-3"] {
        assert!(agents.iter().any(|a| a == id));
    }

    // Mission record reflects the abort.
    let mission = state
        .agents
        .missions
        .iter()
        .find(|m| m.id == "mis-001")
        .expect("mission record");
    assert_eq!(mission.status, "ABORTED");
    // Mission moved out of active runs.
    assert!(!runtime.is_active_mission("mis-001"));
    assert!(runtime.mission_is_complete("mis-001"));
}

#[test]
fn abort_mission_drains_queued_turns_for_assigned_agents() {
    let mut state = new_state();
    let mut runtime = build_active_swarm_run(&mut state, "mis-001", &["a-1", "a-2"]);

    state
        .agents
        .queued_codex_turns
        .push_back(nit_core::QueuedCodexTurn {
            agent_id: "a-1".into(),
            mission_id: Some("mis-001".into()),
            prompt: "stale".into(),
            prompt_msg_idx: None,
        });
    // Bystander queue entry for an unrelated agent must NOT be dropped.
    state
        .agents
        .queued_codex_turns
        .push_back(nit_core::QueuedCodexTurn {
            agent_id: "outsider".into(),
            mission_id: Some("mis-other".into()),
            prompt: "keep me".into(),
            prompt_msg_idx: None,
        });
    state.agents.agents.push(lane_for_abort("outsider"));

    runtime.abort_mission(&mut state, "mis-001");

    assert_eq!(state.agents.queued_codex_turns.len(), 1);
    assert_eq!(state.agents.queued_codex_turns[0].agent_id, "outsider");
}

#[test]
fn abort_mission_pushes_system_alert_visible_in_chat() {
    let mut state = new_state();
    let mut runtime = build_active_swarm_run(&mut state, "mis-001", &["a-1"]);

    let n_messages_before = state.agents.messages.len();
    runtime.abort_mission(&mut state, "mis-001");

    let new_messages: Vec<&nit_core::AgentMessage> =
        state.agents.messages[n_messages_before..].iter().collect();
    let alert = new_messages
        .iter()
        .find(|m| m.kind.as_deref() == Some(SYSTEM_ALERT_KIND))
        .expect("expected SYSTEM_ALERT_KIND message after abort");
    assert!(alert.text.contains("aborted"));
    assert_eq!(alert.mission_id.as_deref(), Some("mis-001"));
}

#[test]
fn abort_mission_unknown_id_is_idempotent() {
    let mut state = new_state();
    let mut runtime = SwarmRuntime::default();
    let agents = runtime.abort_mission(&mut state, "mis-does-not-exist");
    assert!(agents.is_empty());
    // No spurious messages for unknown missions.
    assert!(state.agents.messages.is_empty());
}

#[test]
fn abort_all_aborts_every_active_run() {
    let mut state = new_state();
    // Build two missions in two separate runtimes, then merge runs into
    // one via direct field access (test_fixtures gives us pub(crate)
    // SwarmRun; this is the simplest way to seed two missions).
    let mut runtime = build_active_swarm_run(&mut state, "mis-001", &["a-1"]);
    let runtime_002 = build_active_swarm_run(&mut state, "mis-002", &["b-1", "b-2"]);
    for (id, run) in runtime_002.runs {
        runtime.runs.insert(id, run);
    }

    let agents = runtime.abort_all(&mut state);
    assert_eq!(agents.len(), 3);
    assert!(!runtime.is_active_mission("mis-001"));
    assert!(!runtime.is_active_mission("mis-002"));
    assert!(runtime.mission_is_complete("mis-001"));
    assert!(runtime.mission_is_complete("mis-002"));
}
