use super::*;
use nit_core::{AgentLane, AgentLaneKind, Buffer};
use std::path::PathBuf;

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
        last_message: String::new(),
    }
}

#[test]
fn swarm_clones_do_not_count_towards_swarm_size() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let editor = Buffer::empty("editor", None);
    let notes = Buffer::empty("notes", None);
    let mut state = AppState::new(root, editor, notes);
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
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let editor = Buffer::empty("editor", None);
    let notes = Buffer::empty("notes", None);
    let mut state = AppState::new(root, editor, notes);
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
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let editor = Buffer::empty("editor", None);
    let notes = Buffer::empty("notes", None);
    let mut state = AppState::new(root, editor, notes);
    state.agents.agents.clear();

    state.agents.agents.push(make_lane("planner", "planner"));
    state.agents.agents.push(make_lane("a", "worker"));
    state.agents.agents.push(make_lane("b", "worker"));

    let agents = select_swarm_agents(&state, "planner", SwarmSize::Count(4), Some("parallel"));

    assert_eq!(agents, vec!["planner"]);
}

#[test]
fn parallel_without_priorities_clones_planner_to_swarm_size() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let editor = Buffer::empty("editor", None);
    let notes = Buffer::empty("notes", None);
    let mut state = AppState::new(root, editor, notes);
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
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let editor = Buffer::empty("editor", None);
    let notes = Buffer::empty("notes", None);
    let mut state = AppState::new(root, editor, notes);
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
        message: "done".into(),
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
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let editor = Buffer::empty("editor", None);
    let notes = Buffer::empty("notes", None);
    let mut state = AppState::new(root, editor, notes);
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
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let editor = Buffer::empty("editor", None);
    let notes = Buffer::empty("notes", None);
    let mut state = AppState::new(root, editor, notes);
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
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let editor = Buffer::empty("editor", None);
    let notes = Buffer::empty("notes", None);
    let mut state = AppState::new(root, editor, notes);
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
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let editor = Buffer::empty("editor", None);
    let notes = Buffer::empty("notes", None);
    let mut state = AppState::new(root, editor, notes);
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
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let editor = Buffer::empty("editor", None);
    let notes = Buffer::empty("notes", None);
    let mut state = AppState::new(root, editor, notes);
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
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let editor = Buffer::empty("editor", None);
    let notes = Buffer::empty("notes", None);
    let mut state = AppState::new(root, editor, notes);
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
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let editor = Buffer::empty("editor", None);
    let notes = Buffer::empty("notes", None);
    let mut state = AppState::new(root, editor, notes);
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
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let editor = Buffer::empty("editor", None);
    let notes = Buffer::empty("notes", None);
    let mut state = AppState::new(root, editor, notes);
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
    );

    assert!(prompt.contains("source survey -> modeling / experiments / analysis"));
    assert!(prompt.contains("preferred for quantitative or tool-driven lanes"));
    assert!(prompt.contains("Prefer read-only investigation and synthesis tasks"));
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
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let editor = Buffer::empty("editor", None);
    let notes = Buffer::empty("notes", None);
    let mut state = AppState::new(root, editor, notes);
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
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let editor = Buffer::empty("editor", None);
    let notes = Buffer::empty("notes", None);
    let mut state = AppState::new(root, editor, notes);
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
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let editor = Buffer::empty("editor", None);
    let notes = Buffer::empty("notes", None);
    let mut state = AppState::new(root, editor, notes);
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
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let editor = Buffer::empty("editor", None);
    let notes = Buffer::empty("notes", None);
    let mut state = AppState::new(root, editor, notes);
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
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let editor = Buffer::empty("editor", None);
    let notes = Buffer::empty("notes", None);
    let mut state = AppState::new(root, editor, notes);
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
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let editor = Buffer::empty("editor", None);
    let notes = Buffer::empty("notes", None);
    let mut state = AppState::new(root, editor, notes);
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
// assign_clone_roles_for_parallel_coverage — proactively assigns role hints
// to fresh clones so the parallel-template swarm covers a propose lane and a
// review/test lane, mirroring the lab template's read-only worker structure.
// The user's escape hatch: setting the planner role to `all` (or leaving it
// unset) opts out of this enforcement and lets the LLM decide everything.
// ---------------------------------------------------------------------------

/// Build a fresh AppState with the given lanes and role hints already set up.
/// Used by the clone-coverage tests below.
fn make_coverage_state(lanes: &[(&str, &str)], role_hints: &[(&str, &str)]) -> AppState {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let editor = Buffer::empty("editor", None);
    let notes = Buffer::empty("notes", None);
    let mut state = AppState::new(root, editor, notes);
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
