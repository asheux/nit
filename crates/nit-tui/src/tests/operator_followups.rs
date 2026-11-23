use super::*;
use crate::swarm::test_fixtures::test_runtime_with_running_tasks;
use nit_core::{followups::directory, AgentBusEvent, Buffer};
use std::fs;
const MID: &str = "mis-001";
fn fixture() -> (SwarmRuntime, AppState, PathBuf) {
    let dir = std::env::temp_dir().join(format!("nit-operator-note-{}", unique_id()));
    fs::create_dir_all(&dir).unwrap();
    let mut runtime = test_runtime_with_running_tasks(MID, &[("a", "review"), ("b", "test")]);
    runtime.set_test_workspace(MID, &dir);
    let state = AppState::new(
        dir.clone(),
        Buffer::empty("editor", None),
        Buffer::empty("notes", None),
    );
    (runtime, state, dir)
}
fn completed(agent: &str) -> AgentBusEvent {
    AgentBusEvent::TurnCompleted {
        agent_id: agent.into(),
        mission_id: Some(MID.into()),
        thread_id: None,
        token_count: None,
        message: "Prior review result.\n<SWARM_TASK_COMPLETE>".into(),
    }
}
fn receipt(workspace: &Path) -> Receipt {
    let file = fs::read_dir(directory(workspace, MID))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    serde_json::from_slice(&fs::read(file).unwrap()).unwrap()
}
#[test]
fn active_worker_continues_before_dependents_and_confirms_exact_provider_input() {
    let (mut runtime, mut state, dir) = fixture();
    let text = "Please cover the empty input.";
    assert!(runtime
        .queue_operator_followup(&state, MID, None, text, None)
        .is_err());
    runtime
        .queue_operator_followup(&state, MID, Some("task-00"), text, None)
        .unwrap();
    assert_eq!(receipt(&dir).status, Status::Pending);
    assert!(runtime
        .handle_event(&mut state, &completed("b"))
        .iter()
        .all(|d| !d.prompt.contains(text)));
    let dispatch = runtime
        .handle_event(&mut state, &completed("a"))
        .pop()
        .unwrap();
    assert_eq!(dispatch.agent_id, "a");
    assert_eq!(dispatch.task_role.as_deref(), Some("review"));
    assert!(dispatch.prompt.contains(text) && dispatch.prompt.contains("Prior review result."));
    assert_eq!(receipt(&dir).status, Status::Submitted);
    // An old completion from the same lane cannot acknowledge a queued
    // follow-up before the correct worker input has reached the runner.
    assert!(runtime.handle_event(&mut state, &completed("a")).is_empty());
    assert_eq!(receipt(&dir).status, Status::Submitted);
    let (tx, rx) = std::sync::mpsc::channel();
    report_provider_followup_input(&tx, "a", Some(MID), &dispatch.prompt);
    runtime.handle_event(&mut state, &rx.try_recv().unwrap());
    runtime.handle_event(&mut state, &completed("a"));
    assert_eq!(receipt(&dir).status, Status::Delivered);
    fs::remove_dir_all(dir).unwrap();
}
#[test]
fn waiting_task_receives_its_note_on_first_dispatch_and_provider_failure_stays_recoverable() {
    let (mut runtime, mut state, dir) = fixture();
    let task = &mut runtime.runs.get_mut(MID).unwrap().tasks[1];
    task.deps = vec!["task-00".into()];
    task.state = SwarmTaskState::Pending;
    runtime
        .queue_operator_followup(
            &state,
            MID,
            Some("task-01"),
            "Test unicode empty input",
            None,
        )
        .unwrap();
    let dispatches = runtime.handle_event(&mut state, &completed("a"));
    assert_eq!(dispatches.len(), 1);
    let dispatch = &dispatches[0];
    assert_eq!(dispatch.agent_id, "b");
    assert!(dispatch.prompt.contains("Test unicode empty input"));
    assert_eq!(state.agents.followup_workspaces[MID], dir);
    let (tx, rx) = std::sync::mpsc::channel();
    report_provider_followup_input(&tx, "b", Some(MID), &dispatch.prompt);
    runtime.handle_event(&mut state, &rx.try_recv().unwrap());
    runtime.handle_event(
        &mut state,
        &AgentBusEvent::TurnFailed {
            agent_id: "b".into(),
            mission_id: Some(MID.into()),
            thread_id: None,
            token_count: None,
            message: "Provider unavailable".into(),
        },
    );
    let failure = receipt(&dir);
    assert_eq!(failure.status, Status::Failed);
    assert_eq!(failure.text, "Test unicode empty input");
    fs::remove_dir_all(dir).unwrap();
}

#[cfg(unix)]
#[test]
fn receipt_write_failure_retains_operator_text_and_does_not_follow_workspace_symlinks() {
    let (mut runtime, state, dir) = fixture();
    let outside = dir.join("outside");
    fs::create_dir(&outside).unwrap();
    std::os::unix::fs::symlink(&outside, dir.join(".nit")).unwrap();
    assert!(runtime
        .queue_operator_followup(&state, MID, Some("task-00"), "Retain draft", None)
        .is_err());
    assert_eq!(fs::read_dir(&outside).unwrap().count(), 0);
    assert!(runtime.followups[MID].entries.is_empty());
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn cancellation_and_restart_do_not_implicitly_reassign_an_operator_note() {
    let (mut rt, mut st, dir) = fixture();
    rt.queue_operator_followup(&st, MID, Some("task-00"), "Keep this note", None)
        .unwrap();
    rt.abort_mission(&mut st, MID);
    let saved = receipt(&dir);
    assert_eq!(saved.status, Status::Failed);
    assert_eq!(saved.text, "Keep this note");
    let mut fresh = test_runtime_with_running_tasks(MID, &[("a", "review")]);
    fresh.set_test_workspace(MID, &dir);
    assert!(fresh
        .handle_event(&mut st, &completed("a"))
        .iter()
        .all(|d| !d.prompt.contains(&saved.text)));
    assert_eq!(receipt(&dir), saved);
    fs::remove_dir_all(dir).unwrap();
}
#[test]
fn exact_active_write_owns_a_file_but_history_and_ambiguous_sources_need_selection() {
    let (mut rt, mut st, dir) = fixture();
    let source = Source {
        path: "src/lib.rs".into(),
        ..Default::default()
    };
    rt.runs.get_mut(MID).unwrap().tasks[0].writes = true;
    st.genome_turn_modified
        .entry("a".into())
        .or_default()
        .insert(dir.join(&source.path));
    assert_eq!(
        rt.operator_followup_task_for_path(&st, MID, Path::new(&source.path))
            .unwrap(),
        "task-00"
    );
    assert!(rt
        .queue_operator_followup(
            &st,
            MID,
            None,
            "clarify",
            Some(&Source {
                revision: Some("abc".into()),
                ..source.clone()
            })
        )
        .is_err());
    rt.runs.get_mut(MID).unwrap().tasks[1].writes = true;
    st.genome_turn_modified
        .entry("b".into())
        .or_default()
        .insert(dir.join(&source.path));
    assert!(rt
        .queue_operator_followup(&st, MID, None, "clarify", Some(&source))
        .is_err());
    rt.queue_operator_followup(&st, MID, Some("task-01"), "clarify", Some(&source))
        .unwrap();
    assert_eq!(receipt(&dir).source, Some(source));
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn sibling_tasks_on_one_lane_keep_separate_turns_and_receipts() {
    let (mut rt, mut state, dir) = fixture();
    let tasks = &mut rt.runs.get_mut(MID).unwrap().tasks;
    tasks[1].agent_id = "a".into();
    tasks[1].state = SwarmTaskState::Ready;
    assert!(rt.operator_followup_task_for_agent(MID, "a").is_err());
    assert!(
        crate::swarm::graph_exec::dispatch_ready_tasks(rt.runs.get_mut(MID).unwrap()).is_empty()
    );
    rt.queue_operator_followup(&state, MID, Some("task-01"), "Only for second task", None)
        .unwrap();
    let dispatch = rt.handle_event(&mut state, &completed("a")).pop().unwrap();
    assert_eq!(dispatch.task_role.as_deref(), Some("test"));
    assert!(dispatch.prompt.contains("Only for second task"));
    assert_eq!(receipt(&dir).task_id, "task-01");
    fs::remove_dir_all(dir).unwrap();
}
