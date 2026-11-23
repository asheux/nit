use super::*;

fn target() -> CodexLiveTurn {
    CodexLiveTurn {
        agent_id: "writer".into(),
        mission_id: Some("mission".into()),
        cwd: "/work".into(),
        thread_id: "thread".into(),
        turn_id: "turn".into(),
        origin: "question\n[nit-task-turn aa01]".into(),
    }
}

#[test]
fn exact_lane_workspace_and_turn_are_required_before_sending() {
    let t = target();
    let (tx, rx) = mpsc::sync_channel(1);
    let live = LiveRoutes::default();
    live.0
        .lock()
        .unwrap()
        .insert("writer".into(), (t.clone(), tx));
    assert!(live.get("writer", None, Path::new("/work")).is_none());
    assert!(live
        .get("writer", Some("mission"), Path::new("/elsewhere"))
        .is_none());
    assert!(t.matches_task_turn("aa01"));
    assert!(!t.matches_task_turn("aa0"));
    let quoted = CodexLiveTurn {
        origin: "old\n[nit-task-turn aa01]\ncurrent\n[nit-task-turn bb02]".into(),
        ..t.clone()
    };
    assert!(!quoted.matches_task_turn("aa01"));
    assert!(quoted.matches_task_turn("bb02"));
    let stale = CodexLiveTurn {
        turn_id: "different".into(),
        ..t.clone()
    };
    assert_eq!(
        live.steer(stale, "id".into(), "message".into())
            .recv()
            .unwrap(),
        SteerOutcome::Stale
    );
    assert!(rx.try_recv().is_err());
    let result = live.steer(t, "id".into(), "message".into());
    assert!(result.try_recv().is_err());
    let req = rx.recv().unwrap();
    req.reply.send(SteerOutcome::Accepted).unwrap();
    assert_eq!(result.recv().unwrap(), SteerOutcome::Accepted);
}

#[test]
fn missing_or_wrong_acknowledgement_never_implies_delivery() {
    let t = target();
    for (wire, expected) in [
        (
            json!({"id":7,"result":{"turnId":"turn"}}),
            SteerOutcome::Accepted,
        ),
        (
            json!({"id":7,"error":{"message":"no active turn"}}),
            SteerOutcome::Stale,
        ),
        (
            json!({"id":7,"result":{"turnId":"other"}}),
            SteerOutcome::Unknown("Codex acknowledgement did not match the requested turn".into()),
        ),
    ] {
        let (tx, rx) = mpsc::channel();
        let mut pending = Pending::default();
        pending.requests.insert(7, (Instant::now(), tx));
        assert!(!pending.ack(&json!({"id":8,"result":{"turnId":"turn"}}), &t));
        assert!(rx.try_recv().is_err());
        assert!(pending.ack(&wire, &t));
        assert_eq!(rx.recv().unwrap(), expected);
    }
    let (tx, rx) = mpsc::channel();
    let mut pending = Pending::default();
    pending.requests.insert(10, (Instant::now(), tx));
    drop(pending);
    assert!(matches!(rx.recv().unwrap(), SteerOutcome::Unknown(_)));
}

#[cfg(unix)]
#[test]
fn process_negotiates_explicit_settings_then_steers_only_the_running_turn() {
    use std::fs;
    let dir = tempfile::tempdir().unwrap();
    let mut cmd = Command::new("sh");
    cmd.current_dir(dir.path()).args(["-c",r#"
readrpc() { IFS= read -r msg || exit 1; printf '%s\n' "$msg" >> requests; }
readrpc; echo '{"id":1,"result":{}}'
readrpc; readrpc; echo '{"id":2,"result":{"thread":{"id":"thread","status":{"type":"idle"}},"sandbox":{"type":"readOnly"},"approvalPolicy":"on-request"}}'
readrpc
echo '{"method":"item/completed","params":{"threadId":"thread","turnId":"turn","item":{"type":"agentMessage","text":"real answer"}}}'
echo '{"id":3,"result":{"turn":{"id":"turn"}}}'
readrpc
echo '{"id":77,"method":"item/commandExecution/requestApproval","params":{}}'
readrpc
echo '{"id":78,"method":"item/fileChange/requestApproval","params":{}}'
readrpc
echo '{"id":79,"method":"item/permissions/requestApproval","params":{"permissions":{"network":{"enabled":true}}}}'
readrpc
echo '{"id":80,"method":"other/requestApproval","params":{}}'
readrpc
echo '{"id":11,"result":{"turnId":"turn"}}'
echo '{"method":"turn/completed","params":{"threadId":"other","turn":{"id":"turn","status":"failed"}}}'
echo '{"method":"thread/tokenUsage/updated","params":{"threadId":"thread","turnId":"turn","tokenUsage":{"last":{"totalTokens":8},"modelContextWindow":1000}}}'
echo '{"method":"turn/completed","params":{"threadId":"thread","turn":{"id":"turn","status":"completed","items":[]}}}'
"#]);
    let mut rpc = Rpc::connect(cmd).unwrap();
    let stop = ExecStop::default();
    let mission = Some("m".into());
    let config = CodexRunnerConfig {
        sandbox: Some("workspace-write".into()),
        approval_policy: Some("on-request".into()),
        ..Default::default()
    };
    let task = Task {
        agent: "model#swarm-1",
        mission: &mission,
        cwd: dir.path(),
        resume: Some("thread"),
        persist: true,
        effort: Some("low"),
        prompt: "question",
        read_only: true,
        config: &config,
    };
    let target = task.open(&mut rpc, &stop).unwrap();
    let (send, incoming) = mpsc::sync_channel(1);
    let (out, reply) = mpsc::channel();
    send.send(Request {
        id: "my-receipt".into(),
        text: "one correction".into(),
        reply: out,
    })
    .unwrap();
    let (events, _) = mpsc::channel();
    let mut session = Session {
        target: &target,
        events: &events,
        cancel: &stop,
        incoming,
        tokens: None,
        text: None,
        written: Default::default(),
        final_answer_seen: false,
    };
    assert_eq!(session.drive(&mut rpc).unwrap(), "real answer");
    assert_eq!(reply.recv().unwrap(), SteerOutcome::Accepted);
    assert_eq!(session.tokens.unwrap().total_tokens, 8);
    let all: Vec<Value> = fs::read_to_string(dir.path().join("requests"))
        .unwrap()
        .lines()
        .map(|s| serde_json::from_str(s).unwrap())
        .collect();
    assert_eq!(all[2]["method"], "thread/resume");
    let p = &all[2]["params"];
    assert_eq!(p["threadId"], "thread");
    assert_eq!(p["sandbox"], "read-only");
    assert_eq!(p["approvalPolicy"], "on-request");
    assert_eq!(p["model"], "model");
    assert_eq!(p["cwd"], dir.path().to_str().unwrap());
    assert_eq!(all[3]["params"]["effort"], "low");
    assert_eq!(all[4]["method"], "turn/steer");
    let p = &all[4]["params"];
    assert_eq!(p["expectedTurnId"], "turn");
    assert_eq!(p["threadId"], "thread");
    assert_eq!(p["clientUserMessageId"], "my-receipt");
    assert_eq!(p["input"][0]["text"], "one correction");
    assert_eq!(all[5]["result"]["decision"], "decline");
    assert_eq!(all[6]["result"]["decision"], "decline");
    assert_eq!(all[7]["result"], json!({"permissions":{},"scope":"turn"}));
    assert!(all[8].get("error").is_some());
}

#[test]
fn only_completed_workspace_changes_are_attributed_to_the_turn_once() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("before.rs"), "fixture").unwrap();
    let t = CodexLiveTurn {
        cwd: root.path().into(),
        ..target()
    };
    let (events, out) = mpsc::channel();
    let (_, input) = mpsc::channel();
    let stop = ExecStop::default();
    let mut session = Session {
        target: &t,
        events: &events,
        cancel: &stop,
        incoming: input,
        tokens: None,
        text: None,
        written: Default::default(),
        final_answer_seen: false,
    };
    let mut change = json!({"type":"fileChange","status":"failed","changes":[{"path":"before.rs","kind":{"type":"update","move_path":"after.rs"}},{"path":"../outside","kind":{"type":"add"}}]});
    session.observe_item(&json!({"type":"agentMessage","text":"Starting my inspection"}));
    session.observe_item(&change);
    assert!(
        session.text.is_none(),
        "tool work invalidates a provisional legacy answer"
    );
    assert!(out.try_recv().is_err());
    change["status"] = json!("completed");
    session.observe_item(&change);
    session.observe_item(&change);
    let paths = out
        .try_iter()
        .map(|event| match event {
            AgentBusEvent::FileWrite {
                agent_id,
                mission_id,
                path,
            } => {
                assert_eq!(agent_id, "writer");
                assert_eq!(mission_id.as_deref(), Some("mission"));
                path
            }
            _ => panic!("unexpected event"),
        })
        .collect::<Vec<_>>();
    assert_eq!(
        paths,
        [
            root.path().canonicalize().unwrap().join("before.rs"),
            root.path().canonicalize().unwrap().join("after.rs")
        ]
    );
    assert_eq!(std::fs::read_to_string(&paths[0]).unwrap(), "fixture");
    #[cfg(unix)]
    {
        let outside = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(outside.path(), root.path().join("escape")).unwrap();
        assert!(workspace_change(root.path(), "escape/file").is_none());
    }
}

#[cfg(unix)]
#[test]
fn missing_sandbox_confirmation_stops_before_any_model_input() {
    let dir = tempfile::tempdir().unwrap();
    let mut cmd = Command::new("sh");
    cmd.args(["-c",r#"read -r a; echo '{"id":1,"result":{}}'; read -r b; read -r c; echo '{"id":2,"result":{"thread":{"id":"t"},"approvalPolicy":"never","sandbox":{"type":"dangerFullAccess"}}}'; read -r d && touch sent"#]).current_dir(dir.path());
    let mut rpc = Rpc::connect(cmd).unwrap();
    let config = CodexRunnerConfig::default();
    let stop = ExecStop::default();
    let task = Task {
        agent: "test",
        mission: &None,
        cwd: dir.path(),
        resume: None,
        persist: false,
        effort: None,
        prompt: "private input",
        read_only: true,
        config: &config,
    };
    assert!(task
        .open(&mut rpc, &stop)
        .unwrap_err()
        .contains("did not confirm"));
    drop(rpc);
    assert!(!dir.path().join("sent").exists());
}
