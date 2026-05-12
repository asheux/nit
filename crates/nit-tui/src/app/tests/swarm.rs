//! Swarm template / lab / parallel / propose_dispatch / shadow tests.
//! Verifies prompt construction, genome-landscape augment, role gates.

use super::*;

#[test]
fn swarm_bulk_preserves_operator_selected_ops_tab() {
    // The bulk template used to auto-switch the Agent Ops dock to DAG on
    // dispatch — operators reported that as a surprise that yanked focus
    // away from the tab they were watching. The dispatch now leaves
    // dock_tab alone; operators can switch to DAG themselves if they want
    // to watch the graph build.
    let mut state = state_for_test();
    state.focus = PaneId::Notes;
    state.agents.dock_tab = AgentOpsTab::Roster;
    state.agents.agents.push(nit_core::AgentLane {
        id: "planner".into(),
        role: "Planner".into(),
        lane: "codex".into(),
        kind: nit_core::AgentLaneKind::Codex,
        status: nit_core::AgentStatus::Idle,
        heartbeat_age_secs: 0,
        queue_len: 0,
        current_mission: None,
        shadow: false,
        last_message: String::new(),
    });
    state.agents.agents.push(nit_core::AgentLane {
        id: "worker".into(),
        role: "Worker".into(),
        lane: "codex".into(),
        kind: nit_core::AgentLaneKind::Codex,
        status: nit_core::AgentStatus::Idle,
        heartbeat_age_secs: 0,
        queue_len: 0,
        current_mission: None,
        shadow: false,
        last_message: String::new(),
    });
    state.agents.chat_input = "@swarm 2 template=bulk do thing".into();
    state.agents.chat_input_cursor = state.agents.chat_input.chars().count();
    let mut vitals = VitalsState::default();
    let mut swarm = SwarmRuntime::default();

    assert!(handle_agent_station_key(
        KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
        &mut state,
        &mut vitals,
        None,
        None,
        &mut swarm
    ));
    assert_eq!(state.agents.dock_tab, AgentOpsTab::Roster);
}

#[test]
fn swarm_auto_detects_template_line_without_prefix() {
    let mut state = state_for_test();
    state.focus = PaneId::Notes;
    state.agents.agents.push(nit_core::AgentLane {
        id: "planner".into(),
        role: "Planner".into(),
        lane: "codex".into(),
        kind: nit_core::AgentLaneKind::Codex,
        status: nit_core::AgentStatus::Idle,
        heartbeat_age_secs: 0,
        queue_len: 0,
        current_mission: None,
        shadow: false,
        last_message: String::new(),
    });
    state.agents.agents.push(nit_core::AgentLane {
        id: "worker".into(),
        role: "Worker".into(),
        lane: "codex".into(),
        kind: nit_core::AgentLaneKind::Codex,
        status: nit_core::AgentStatus::Idle,
        heartbeat_age_secs: 0,
        queue_len: 0,
        current_mission: None,
        shadow: false,
        last_message: String::new(),
    });

    state.agents.chat_input =
        "You are the SWARM PLANNER inside nit.\nTemplate: `parallel`\nDo thing.".into();
    state.agents.chat_input_cursor = state.agents.chat_input.chars().count();
    let mut vitals = VitalsState::default();
    let mut swarm = SwarmRuntime::default();

    assert!(handle_agent_station_key(
        KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
        &mut state,
        &mut vitals,
        None,
        None,
        &mut swarm
    ));
    assert!(state.agents.missions.iter().any(|mission| mission.swarm));
    assert!(state
        .agents
        .missions
        .first()
        .is_some_and(|mission| mission.title.contains("Swarm[parallel]")));
}

#[test]
fn swarm_auto_detects_swarm_role_and_uses_default_template() {
    let mut state = state_for_test();
    state.focus = PaneId::Notes;
    state.agents.dock_tab = AgentOpsTab::Roster;
    state.agents.swarm_default_template = "bulk".into();
    state.agents.agents.push(nit_core::AgentLane {
        id: "planner".into(),
        role: "Planner".into(),
        lane: "codex".into(),
        kind: nit_core::AgentLaneKind::Codex,
        status: nit_core::AgentStatus::Idle,
        heartbeat_age_secs: 0,
        queue_len: 0,
        current_mission: None,
        shadow: false,
        last_message: String::new(),
    });
    state.agents.agents.push(nit_core::AgentLane {
        id: "worker".into(),
        role: "Worker".into(),
        lane: "codex".into(),
        kind: nit_core::AgentLaneKind::Codex,
        status: nit_core::AgentStatus::Idle,
        heartbeat_age_secs: 0,
        queue_len: 0,
        current_mission: None,
        shadow: false,
        last_message: String::new(),
    });
    state.agents.chat_input = "You are the SWARM SYNTHESIZER.\nCombine agent outputs.".into();
    state.agents.chat_input_cursor = state.agents.chat_input.chars().count();
    let mut vitals = VitalsState::default();
    let mut swarm = SwarmRuntime::default();

    assert!(handle_agent_station_key(
        KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
        &mut state,
        &mut vitals,
        None,
        None,
        &mut swarm
    ));
    assert_eq!(state.agents.dock_tab, AgentOpsTab::Roster);
    assert!(state
        .agents
        .missions
        .first()
        .is_some_and(|mission| mission.title.contains("Swarm[bulk]")));
}

#[test]
fn swarm_auto_detects_plain_prompt_when_bulk_template_selected() {
    let mut state = state_for_test();
    state.focus = PaneId::Notes;
    state.agents.dock_tab = AgentOpsTab::Roster;
    state.agents.swarm_default_template = "bulk".into();
    state.agents.agents.push(nit_core::AgentLane {
        id: "planner".into(),
        role: "Planner".into(),
        lane: "codex".into(),
        kind: nit_core::AgentLaneKind::Codex,
        status: nit_core::AgentStatus::Idle,
        heartbeat_age_secs: 0,
        queue_len: 0,
        current_mission: None,
        shadow: false,
        last_message: String::new(),
    });
    state.agents.agents.push(nit_core::AgentLane {
        id: "worker".into(),
        role: "Worker".into(),
        lane: "codex".into(),
        kind: nit_core::AgentLaneKind::Codex,
        status: nit_core::AgentStatus::Idle,
        heartbeat_age_secs: 0,
        queue_len: 0,
        current_mission: None,
        shadow: false,
        last_message: String::new(),
    });
    state.agents.chat_input = "do a quick repo health check and suggest next steps".into();
    state.agents.chat_input_cursor = state.agents.chat_input.chars().count();
    let mut vitals = VitalsState::default();
    let mut swarm = SwarmRuntime::default();

    assert!(handle_agent_station_key(
        KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
        &mut state,
        &mut vitals,
        None,
        None,
        &mut swarm
    ));
    assert_eq!(state.agents.dock_tab, AgentOpsTab::Roster);
    assert!(state
        .agents
        .missions
        .first()
        .is_some_and(|mission| mission.title.contains("Swarm[bulk]")));
}

#[test]
fn swarm_autostart_uses_codex_max_parallel_turns_as_size_hint() {
    let mut state = state_for_test();
    state.focus = PaneId::Notes;
    state.agents.swarm_default_template = "bulk".into();
    state.agents.codex_max_parallel_turns = 6;
    for idx in 0..6 {
        let id = if idx == 0 {
            "planner".to_string()
        } else {
            format!("worker-{idx}")
        };
        state.agents.agents.push(nit_core::AgentLane {
            id,
            role: "Codex".into(),
            lane: "codex".into(),
            kind: nit_core::AgentLaneKind::Codex,
            status: nit_core::AgentStatus::Idle,
            heartbeat_age_secs: 0,
            queue_len: 0,
            current_mission: None,
            shadow: false,
            last_message: String::new(),
        });
    }
    state.agents.chat_input = "do a quick repo health check and suggest next steps".into();
    state.agents.chat_input_cursor = state.agents.chat_input.chars().count();
    let mut vitals = VitalsState::default();
    let mut swarm = SwarmRuntime::default();

    assert!(handle_agent_station_key(
        KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
        &mut state,
        &mut vitals,
        None,
        None,
        &mut swarm
    ));
    let assigned = state
        .agents
        .missions
        .first()
        .map(|mission| mission.assigned_agents.len())
        .unwrap_or(0);
    assert_eq!(assigned, 6);
}

#[test]
fn swarm_uses_roster_default_template_when_argument_missing() {
    let mut state = state_for_test();
    state.focus = PaneId::Notes;
    state.agents.swarm_default_template = "parallel".into();
    state.agents.agents.push(nit_core::AgentLane {
        id: "planner".into(),
        role: "Planner".into(),
        lane: "codex".into(),
        kind: nit_core::AgentLaneKind::Codex,
        status: nit_core::AgentStatus::Idle,
        heartbeat_age_secs: 0,
        queue_len: 0,
        current_mission: None,
        shadow: false,
        last_message: String::new(),
    });
    state.agents.agents.push(nit_core::AgentLane {
        id: "worker".into(),
        role: "Worker".into(),
        lane: "codex".into(),
        kind: nit_core::AgentLaneKind::Codex,
        status: nit_core::AgentStatus::Idle,
        heartbeat_age_secs: 0,
        queue_len: 0,
        current_mission: None,
        shadow: false,
        last_message: String::new(),
    });
    state.agents.chat_input = "@swarm 2 do thing".into();
    state.agents.chat_input_cursor = state.agents.chat_input.chars().count();
    let mut vitals = VitalsState::default();
    let mut swarm = SwarmRuntime::default();

    assert!(handle_agent_station_key(
        KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
        &mut state,
        &mut vitals,
        None,
        None,
        &mut swarm
    ));
    assert!(state
        .agents
        .missions
        .first()
        .is_some_and(|mission| mission.title.contains("Swarm[parallel]")));
}

#[test]
fn swarm_uses_roster_default_mission_when_argument_missing() {
    let mut state = state_for_test();
    state.focus = PaneId::Notes;
    state.agents.swarm_default_template = "parallel".into();
    state.agents.swarm_default_mission = "research".into();
    state.agents.agents.push(nit_core::AgentLane {
        id: "planner".into(),
        role: "Planner".into(),
        lane: "codex".into(),
        kind: nit_core::AgentLaneKind::Codex,
        status: nit_core::AgentStatus::Idle,
        heartbeat_age_secs: 0,
        queue_len: 0,
        current_mission: None,
        shadow: false,
        last_message: String::new(),
    });
    state.agents.agents.push(nit_core::AgentLane {
        id: "worker".into(),
        role: "Worker".into(),
        lane: "codex".into(),
        kind: nit_core::AgentLaneKind::Codex,
        status: nit_core::AgentStatus::Idle,
        heartbeat_age_secs: 0,
        queue_len: 0,
        current_mission: None,
        shadow: false,
        last_message: String::new(),
    });
    state.agents.chat_input = "@swarm 2 read papers and compare ideas".into();
    state.agents.chat_input_cursor = state.agents.chat_input.chars().count();
    let mut vitals = VitalsState::default();
    let mut swarm = SwarmRuntime::default();

    assert!(handle_agent_station_key(
        KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
        &mut state,
        &mut vitals,
        None,
        None,
        &mut swarm
    ));
    assert!(state
        .agents
        .missions
        .first()
        .is_some_and(|mission| mission.title.contains("(research)")));
}

#[test]
fn explicit_prompt_mission_overrides_roster_default_mission() {
    let mut state = state_for_test();
    state.focus = PaneId::Notes;
    state.agents.swarm_default_template = "parallel".into();
    state.agents.swarm_default_mission = "general".into();
    state.agents.agents.push(nit_core::AgentLane {
        id: "planner".into(),
        role: "Planner".into(),
        lane: "codex".into(),
        kind: nit_core::AgentLaneKind::Codex,
        status: nit_core::AgentStatus::Idle,
        heartbeat_age_secs: 0,
        queue_len: 0,
        current_mission: None,
        shadow: false,
        last_message: String::new(),
    });
    state.agents.agents.push(nit_core::AgentLane {
        id: "worker".into(),
        role: "Worker".into(),
        lane: "codex".into(),
        kind: nit_core::AgentLaneKind::Codex,
        status: nit_core::AgentStatus::Idle,
        heartbeat_age_secs: 0,
        queue_len: 0,
        current_mission: None,
        shadow: false,
        last_message: String::new(),
    });
    state.agents.chat_input = "@swarm 2 Mission: research\nread papers and compare ideas".into();
    state.agents.chat_input_cursor = state.agents.chat_input.chars().count();
    let mut vitals = VitalsState::default();
    let mut swarm = SwarmRuntime::default();

    assert!(handle_agent_station_key(
        KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
        &mut state,
        &mut vitals,
        None,
        None,
        &mut swarm
    ));
    assert!(state
        .agents
        .missions
        .first()
        .is_some_and(|mission| mission.title.contains("(research)")));
}

#[test]
fn propose_dispatch_receives_genome_landscape_from_cached_reports() {
    // Setup: workspace with one scope file, `state.genome_reports` has its
    // report (as would be the case after `WorkspaceScanRuntime::hydrate`
    // loaded it from `.nit/genome/`). The propose-role augment must pull
    // that report into the dispatch prompt.
    let mut state = state_for_test_in_workspace("propose-landscape");
    let rel = "src/lib.rs".to_string();
    let abs = state.workspace_root.join(&rel);
    fs::create_dir_all(abs.parent().unwrap()).unwrap();
    fs::write(&abs, "fn main() {}\n").unwrap();
    state.genome_reports.insert(
        abs.clone(),
        seeded_genome_report(abs.clone(), nit_core::GenomeTier::Oscillator),
    );

    // Drive the core landscape builder — this is what both swarm and shadow
    // paths ultimately call.
    let section = super::super::dispatch::build_propose_genome_landscape(
        &state,
        std::slice::from_ref(&rel),
        Some("propose"),
    )
    .expect("landscape section should be produced when reports exist");
    assert!(section.contains("GENOME LANDSCAPE"));
    assert!(section.contains("src/lib.rs"));
    assert!(section.contains("tier II"));
    assert!(section.contains("0.42"));
}

#[test]
fn propose_dispatch_has_no_landscape_when_reports_are_empty() {
    // Cold cache: no reports loaded yet. Builder returns None and the
    // proposer dispatches without a landscape section (acceptable per the
    // design — don't block the agent on scan completion).
    let state = state_for_test_in_workspace("propose-no-landscape");
    let rel = "src/lib.rs".to_string();
    let section = super::super::dispatch::build_propose_genome_landscape(
        &state,
        std::slice::from_ref(&rel),
        Some("propose"),
    );
    assert!(section.is_none());
}

#[test]
fn shadow_proposer_prompt_receives_genome_landscape() {
    // Shadow proposer augmentation uses the active editor buffer's path as
    // the scope. Seed a report for that path; augment; assert the prompt
    // carries the landscape block.
    let mut state = state_for_test_in_workspace("shadow-landscape");
    let file = state.workspace_root.join("main.rs");
    fs::write(&file, "fn main() {}\n").unwrap();
    // Rebind the active editor buffer to the scope file so the shadow
    // augmenter derives its path from `state.editor_buffer()`.
    let text = std::fs::read_to_string(&file).unwrap();
    let buf = nit_core::Buffer::from_str("editor", &text, Some(file.clone()));
    let buf_id = state.active_editor_buffer_id;
    state.buffers[buf_id] = buf;

    state.genome_reports.insert(
        file.clone(),
        seeded_genome_report(file.clone(), nit_core::GenomeTier::StillLife),
    );

    let mut dispatch = crate::shadow::ShadowDispatch {
        // Shadow lane id format: <base>#shadow-<run>-<role>
        agent_id: "codex-main#shadow-01-propose-a".into(),
        prompt: String::from("original prompt body"),
        mission_id: None,
        prompt_msg_idx: None,
    };
    super::augment_shadow_prompt_with_landscape(&state, &mut dispatch);

    assert!(dispatch.prompt.contains("original prompt body"));
    assert!(
        dispatch.prompt.contains("GENOME LANDSCAPE"),
        "prompt was: {}",
        dispatch.prompt
    );
    assert!(dispatch.prompt.contains("main.rs"));
    assert!(dispatch.prompt.contains("tier I"));
}

#[test]
fn shadow_judge_role_receives_judge_framing_not_propose_framing() {
    // The landscape framing differs by role (propose / judge / integrate).
    // Shadow's "judge" lane maps to the "judge" framing — verify the
    // judge-specific wording appears.
    let mut state = state_for_test_in_workspace("shadow-judge-landscape");
    let file = state.workspace_root.join("main.rs");
    fs::write(&file, "fn main() {}\n").unwrap();
    let text = std::fs::read_to_string(&file).unwrap();
    let buf = nit_core::Buffer::from_str("editor", &text, Some(file.clone()));
    let buf_id = state.active_editor_buffer_id;
    state.buffers[buf_id] = buf;

    state.genome_reports.insert(
        file.clone(),
        seeded_genome_report(file.clone(), nit_core::GenomeTier::Oscillator),
    );

    let mut dispatch = crate::shadow::ShadowDispatch {
        agent_id: "codex-main#shadow-01-judge".into(),
        prompt: String::new(),
        mission_id: None,
        prompt_msg_idx: None,
    };
    super::augment_shadow_prompt_with_landscape(&state, &mut dispatch);
    // Judge framing includes the phrase "weigh proposals against this".
    assert!(
        dispatch.prompt.contains("weigh proposals against this"),
        "judge framing missing; prompt was: {}",
        dispatch.prompt
    );
}

#[test]
fn shadow_review_role_uses_integrate_framing() {
    // Shadow's "review" role maps to the landscape's "integrate" framing
    // (target these metrics with your edits).
    let mut state = state_for_test_in_workspace("shadow-review-landscape");
    let file = state.workspace_root.join("main.rs");
    fs::write(&file, "fn main() {}\n").unwrap();
    let text = std::fs::read_to_string(&file).unwrap();
    let buf = nit_core::Buffer::from_str("editor", &text, Some(file.clone()));
    let buf_id = state.active_editor_buffer_id;
    state.buffers[buf_id] = buf;

    state.genome_reports.insert(
        file.clone(),
        seeded_genome_report(file.clone(), nit_core::GenomeTier::Spaceship),
    );

    let mut dispatch = crate::shadow::ShadowDispatch {
        agent_id: "codex-main#shadow-01-review".into(),
        prompt: String::new(),
        mission_id: None,
        prompt_msg_idx: None,
    };
    super::augment_shadow_prompt_with_landscape(&state, &mut dispatch);
    assert!(
        dispatch
            .prompt
            .contains("target these metrics with your edits"),
        "integrate framing missing; prompt was: {}",
        dispatch.prompt
    );
}

#[test]
fn swarm_dispatch_augment_gates_non_landscape_roles() {
    // research / test / planner must NOT receive a landscape section —
    // they either operate on external sources or run verification commands,
    // neither of which wants per-file tier numbers bloating the prompt.
    let state = state_for_test_in_workspace("swarm-role-gate");
    let swarm = crate::swarm::SwarmRuntime::default();
    for role in ["research", "test", "planner", "computational-research"] {
        let mut dispatch = crate::swarm::SwarmDispatch {
            agent_id: format!("clone-{role}"),
            mission_id: "mission-xyz".into(),
            prompt: format!("{role} prompt"),
            task_role: Some(role.into()),
        };
        let original = dispatch.prompt.clone();
        super::augment_dispatch_prompt_with_landscape(&state, &swarm, &mut dispatch);
        assert_eq!(
            dispatch.prompt, original,
            "role '{role}' must not receive landscape"
        );
        assert!(
            !dispatch.prompt.contains("GENOME LANDSCAPE"),
            "role '{role}' prompt leaked a GENOME LANDSCAPE section"
        );
    }
}

// Task-level prompts (what each agent actually receives) must be identical
// across swarm templates for the same role. Template only affects the
// PLANNER prompt and DAG scheduling (single-writer invariant, concurrent
// integrate fan-out, DAG shape) — it does NOT affect what a dispatched
// propose/judge/integrate/review role sees. `wrap_task_prompt` takes no
// template argument, so the output is necessarily template-agnostic, but
// this test catches any future drift and proves the landscape augment
// produces byte-identical output for lab vs parallel dispatches of the
// same role.
#[test]
fn lab_and_parallel_templates_produce_equivalent_propose_prompts() {
    use crate::swarm::{
        test_runtime_with_running_tasks_and_template, SwarmDispatch, SwarmTemplateForTests,
    };

    let mut state = state_for_test_in_workspace("swarm-template-parity");
    let rel = "src/lib.rs".to_string();
    let abs = state.workspace_root.join(&rel);
    fs::create_dir_all(abs.parent().unwrap()).unwrap();
    fs::write(&abs, "fn main() {}\n").unwrap();
    state.genome_reports.insert(
        abs.clone(),
        seeded_genome_report(abs.clone(), nit_core::GenomeTier::Oscillator),
    );

    let mut parallel_runtime = test_runtime_with_running_tasks_and_template(
        "mission-parallel",
        &[("clone-1", "propose")],
        SwarmTemplateForTests::Parallel,
    );
    parallel_runtime.set_scope_files_for_test("mission-parallel", vec![rel.clone()]);

    let mut lab_runtime = test_runtime_with_running_tasks_and_template(
        "mission-lab",
        &[("clone-1", "propose")],
        SwarmTemplateForTests::Lab,
    );
    lab_runtime.set_scope_files_for_test("mission-lab", vec![rel.clone()]);

    let make_dispatch = |mission: &str| SwarmDispatch {
        agent_id: "clone-1".into(),
        mission_id: mission.into(),
        prompt: "propose task body".into(),
        task_role: Some("propose".into()),
    };

    let mut parallel_dispatch = make_dispatch("mission-parallel");
    super::augment_dispatch_prompt_with_landscape(
        &state,
        &parallel_runtime,
        &mut parallel_dispatch,
    );

    let mut lab_dispatch = make_dispatch("mission-lab");
    super::augment_dispatch_prompt_with_landscape(&state, &lab_runtime, &mut lab_dispatch);

    let parallel_landscape = parallel_dispatch
        .prompt
        .split_once("## GENOME LANDSCAPE")
        .map(|(_, rest)| rest)
        .expect("parallel dispatch missing landscape");
    let lab_landscape = lab_dispatch
        .prompt
        .split_once("## GENOME LANDSCAPE")
        .map(|(_, rest)| rest)
        .expect("lab dispatch missing landscape");
    assert_eq!(
        parallel_landscape, lab_landscape,
        "landscape content must be template-agnostic"
    );

    for expected in [
        "GENOME LANDSCAPE",
        "use this to ground your proposal",
        "tier II",
        "0.42",
        "src/lib.rs",
    ] {
        assert!(
            parallel_dispatch.prompt.contains(expected),
            "parallel prompt missing: {expected}"
        );
        assert!(
            lab_dispatch.prompt.contains(expected),
            "lab prompt missing: {expected}"
        );
    }
}

// Landscape augment fires for every landscape-eligible role under the
// lab template — proves the strong clauses aren't gated on parallel.
#[test]
fn lab_template_augments_all_landscape_roles() {
    use crate::swarm::{test_runtime_with_running_tasks_and_template, SwarmTemplateForTests};

    let mut state = state_for_test_in_workspace("swarm-lab-role-coverage");
    let rel = "src/lib.rs".to_string();
    let abs = state.workspace_root.join(&rel);
    fs::create_dir_all(abs.parent().unwrap()).unwrap();
    fs::write(&abs, "fn main() {}\n").unwrap();
    state.genome_reports.insert(
        abs.clone(),
        seeded_genome_report(abs.clone(), nit_core::GenomeTier::Oscillator),
    );

    for role in ["propose", "integrate", "judge", "review"] {
        let mut rt = test_runtime_with_running_tasks_and_template(
            &format!("mission-{role}"),
            &[("clone-1", role)],
            SwarmTemplateForTests::Lab,
        );
        rt.set_scope_files_for_test(&format!("mission-{role}"), vec![rel.clone()]);

        let mut dispatch = crate::swarm::SwarmDispatch {
            agent_id: "clone-1".into(),
            mission_id: format!("mission-{role}"),
            prompt: format!("{role} task body"),
            task_role: Some(role.into()),
        };
        super::augment_dispatch_prompt_with_landscape(&state, &rt, &mut dispatch);

        assert!(
            dispatch.prompt.contains("GENOME LANDSCAPE"),
            "lab-template {role} dispatch missing landscape"
        );
        assert!(
            dispatch.prompt.contains("tier II"),
            "lab-template {role} dispatch missing landscape numbers"
        );
    }
}

// Regression: swarm's review role used to be the only read-only role that
// didn't receive the landscape augment, even though its role contract
// requires citing encoders. `augment_dispatch_prompt_with_landscape` now
// includes "review" in the landscape-eligible set and
// `build_propose_genome_landscape` emits a review-specific framing ("cite
// these metrics when flagging issues") instead of falling through to the
// propose framing.
#[test]
fn swarm_review_role_receives_genome_landscape() {
    use crate::swarm::SwarmRuntime;

    // Seed the scope and a cached report so the landscape builder has
    // content. Use the same `state_for_test_in_workspace` scaffold as the
    // propose-landscape tests for consistency.
    let mut state = state_for_test_in_workspace("swarm-review-landscape");
    let rel = "src/lib.rs".to_string();
    let abs = state.workspace_root.join(&rel);
    fs::create_dir_all(abs.parent().unwrap()).unwrap();
    fs::write(&abs, "fn main() {}\n").unwrap();
    state.genome_reports.insert(
        abs.clone(),
        seeded_genome_report(abs.clone(), nit_core::GenomeTier::Oscillator),
    );

    // Exercise the landscape builder directly with the review role: the
    // framing must mention citing encoders when flagging issues.
    let section = super::super::dispatch::build_propose_genome_landscape(
        &state,
        std::slice::from_ref(&rel),
        Some("review"),
    )
    .expect("review framing should produce a landscape section");
    assert!(section.contains("GENOME LANDSCAPE"));
    assert!(section.contains("cite these metrics when flagging issues"));
    assert!(
        section.contains("complexity_field"),
        "review framing should name concrete encoders"
    );
    assert!(section.contains("src/lib.rs"));
    assert!(section.contains("tier II"));

    // And the runtime-wide augment helper must now include review in its
    // eligible-role set. Build a SwarmRuntime with a run that has
    // `scope_files = [rel]` via the existing fixture.
    let mut runtime: SwarmRuntime =
        crate::swarm::test_runtime_with_running_tasks("mission-review", &[("clone-1", "review")]);
    // Point the run's scope_files at our seeded file so the augment helper
    // can resolve it. `scope_files_for_mission` is public; we mutate via a
    // test-only fixture accessor below.
    runtime.set_scope_files_for_test("mission-review", vec![rel.clone()]);

    let mut dispatch = crate::swarm::SwarmDispatch {
        agent_id: "clone-1".into(),
        mission_id: "mission-review".into(),
        prompt: "review task body".into(),
        task_role: Some("review".into()),
    };
    super::augment_dispatch_prompt_with_landscape(&state, &runtime, &mut dispatch);

    assert!(
        dispatch.prompt.contains("review task body"),
        "original body preserved"
    );
    assert!(
        dispatch.prompt.contains("GENOME LANDSCAPE"),
        "review dispatch must receive the landscape section"
    );
    assert!(dispatch
        .prompt
        .contains("cite these metrics when flagging issues"));
    assert!(dispatch.prompt.contains("tier II"));
}
