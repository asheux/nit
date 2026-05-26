use super::test_fixtures::{
    completed_run_with_synthesis, new_state_with_lanes, push_mission_message,
    seed_n_mission_messages,
};
use super::{SwarmMissionKind, SwarmTemplate};

const MISSION: &str = "test-mission-001";

#[test]
fn followup_after_research_threads_synthesis_into_planner_prompt() {
    let synthesis =
        "Strategy 1 (hybrid artifact replay) is the strongest fit for swarm follow-ups.";
    let (mut runtime, agent_ids) = completed_run_with_synthesis(
        MISSION,
        SwarmMissionKind::Research,
        SwarmTemplate::Lab,
        &[(
            "agent-a",
            "research",
            "Cited LangChain summary-buffer pattern.",
        )],
        synthesis,
        &["src/lib.rs", "src/main.rs"],
    );
    let mut state = new_state_with_lanes(&agent_ids);

    runtime.reactivate_for_followup(&mut state, MISSION, Some("expand on option B"));
    let prompt = runtime
        .build_followup_planner_prompt(&state, MISSION, "expand on option B")
        .expect("followup prompt should be built");

    assert!(
        prompt.contains("## PREVIOUS RUN"),
        "expected previous-run prelude header in prompt: {prompt}"
    );
    assert!(
        prompt.contains("Mission kind: `research`"),
        "expected research mission kind label in prompt: {prompt}"
    );
    assert!(
        prompt.contains("Strategy 1 (hybrid artifact replay)"),
        "expected synthesis excerpt in prompt: {prompt}"
    );
    assert!(
        prompt.contains("src/lib.rs"),
        "expected scope file in prompt: {prompt}"
    );
}

#[test]
fn followup_after_general_threads_integrator_diff_and_messages() {
    let synthesis = "Landed the matching-bracket highlight (T13) in editor.rs.";
    let (mut runtime, agent_ids) = completed_run_with_synthesis(
        MISSION,
        SwarmMissionKind::General,
        SwarmTemplate::Lab,
        &[(
            "agent-int",
            "integrate",
            "Wrote bracket-pair match logic to editor.rs lines 220-260.",
        )],
        synthesis,
        &["crates/nit-core/src/editor.rs"],
    );
    let mut state = new_state_with_lanes(&agent_ids);
    push_mission_message(
        &mut state,
        MISSION,
        None,
        "please verify the highlight matches Vim semantics",
    );
    push_mission_message(
        &mut state,
        MISSION,
        Some("agent-int"),
        "vim-semantics regression test added",
    );
    push_mission_message(&mut state, MISSION, None, "ship it");

    runtime.reactivate_for_followup(
        &mut state,
        MISSION,
        Some("now fix the visual selection variant"),
    );
    let prompt = runtime
        .build_followup_planner_prompt(&state, MISSION, "now fix the visual selection variant")
        .expect("followup prompt should be built");

    assert!(
        prompt.contains("Wrote bracket-pair match logic"),
        "expected integrator artifact note in prompt: {prompt}"
    );
    assert!(
        prompt.contains("vim-semantics regression test added"),
        "expected agent message in prompt: {prompt}"
    );
    assert!(
        prompt.contains("please verify the highlight"),
        "expected operator message in prompt: {prompt}"
    );
}

#[test]
fn followup_after_computational_research_threads_methods_and_commands() {
    let synthesis = "Benchmarked five embeddings strategies; AutoGPT retrieval is cheapest.";
    let (mut runtime, agent_ids) = completed_run_with_synthesis(
        MISSION,
        SwarmMissionKind::ComputationalResearch,
        SwarmTemplate::Parallel,
        &[(
            "agent-cr",
            "computational-research",
            "Ran cargo bench, results in target/criterion/embeddings.",
        )],
        synthesis,
        &[],
    );
    let mut state = new_state_with_lanes(&agent_ids);

    runtime.reactivate_for_followup(
        &mut state,
        MISSION,
        Some("expand the benchmark to 10K samples"),
    );
    let prompt = runtime
        .build_followup_planner_prompt(&state, MISSION, "expand the benchmark to 10K samples")
        .expect("followup prompt should be built");

    assert!(
        prompt.contains("Ran cargo bench"),
        "expected command artifact note in prompt: {prompt}"
    );
    assert!(
        prompt.contains("Benchmarked five embeddings strategies"),
        "expected synthesis excerpt in prompt: {prompt}"
    );
    assert!(
        prompt.contains("computational-research") || prompt.contains("computational_research"),
        "expected computational-research role/kind label in prompt: {prompt}"
    );
}

#[test]
fn followup_can_switch_mission_kind_and_still_attach_prior_context() {
    let synthesis = "Identified option A and option B for caching strategy.";
    let (mut runtime, agent_ids) = completed_run_with_synthesis(
        MISSION,
        SwarmMissionKind::Research,
        SwarmTemplate::Lab,
        &[(
            "agent-r",
            "research",
            "Surveyed LRU vs ARC vs adaptive replacement.",
        )],
        synthesis,
        &[],
    );
    let mut state = new_state_with_lanes(&agent_ids);

    // The mission-kind override parser expects `mission: <kind>` on a line
    // (see explicit_swarm_mission_kind_from_prompt in types.rs) — using
    // `mission=` instead would not trip the switch.
    let follow_up = "mission: general ship option B";
    runtime.reactivate_for_followup(&mut state, MISSION, Some(follow_up));
    let prompt = runtime
        .build_followup_planner_prompt(&state, MISSION, follow_up)
        .expect("followup prompt should be built");

    assert!(
        prompt.contains("Mission kind: `general`"),
        "expected NEW (general) mission kind in prompt header: {prompt}"
    );
    assert!(
        prompt.contains("Identified option A and option B"),
        "expected prior synthesis in prelude even after kind switch: {prompt}"
    );
    let switch_logged = state.agents.messages.iter().any(|m| {
        let t = &m.text;
        t.contains("Mission kind switched") && t.contains("research") && t.contains("general")
    });
    assert!(
        switch_logged,
        "expected kind-switch system message recorded in state: messages={:?}",
        state
            .agents
            .messages
            .iter()
            .map(|m| m.text.clone())
            .collect::<Vec<_>>()
    );
}

#[test]
fn followup_caps_threaded_messages_at_20_and_drops_oldest_5() {
    let (mut runtime, agent_ids) = completed_run_with_synthesis(
        MISSION,
        SwarmMissionKind::General,
        SwarmTemplate::Lab,
        &[("agent-x", "integrate", "Artifact note.")],
        "Synthesis paragraph.",
        &[],
    );
    let mut state = new_state_with_lanes(&agent_ids);
    seed_n_mission_messages(&mut state, MISSION, "msg", 25);

    runtime.reactivate_for_followup(&mut state, MISSION, Some("follow-up"));
    let prompt = runtime
        .build_followup_planner_prompt(&state, MISSION, "follow-up")
        .expect("followup prompt should be built");

    for i in 1..=5 {
        let body = format!("msg-{i:03}");
        assert!(
            !prompt.contains(&body),
            "expected oldest message {body} to be elided from prompt: {prompt}"
        );
    }
    for i in 6..=25 {
        let body = format!("msg-{i:03}");
        assert!(
            prompt.contains(&body),
            "expected recent message {body} (within last 20) to be present: {prompt}"
        );
    }
}
