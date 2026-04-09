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
        report_status: None,
        report_output: None,
        scope_files: Vec::new(),
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
        report_status: None,
        report_output: None,
        scope_files: Vec::new(),
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
        report_status: None,
        report_output: None,
        scope_files: Vec::new(),
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
        report_status: None,
        report_output: None,
        scope_files: Vec::new(),
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
