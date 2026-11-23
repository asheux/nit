use super::append_stdout_line_capped;
use super::build_codex_exec_args;
use super::build_codex_mcp_tool_call;
use super::codex_model_slug_for_agent_id;
use super::extract_thread_id_from_jsonl;
use super::extract_token_count_from_jsonl;
use super::handle_codex_mcp_notification;
use super::push_json_error_capped;
use super::CodexRunnerConfig;
use super::JSON_ERRORS_CAP;
use super::STDOUT_TAIL_CAP_BYTES;
use nit_core::AgentBusEvent;
use nit_core::AgentTokenCount;
use serde_json::json;
use std::path::Path;
use std::sync::mpsc;
use std::time::Instant;

/// `CodexRunnerConfig` with an explicit sandbox + approval policy — the
/// shape every swarm/shadow test uses to exercise arg-building code paths.
fn custom_config() -> CodexRunnerConfig {
    CodexRunnerConfig {
        sandbox: Some("workspace-write".into()),
        approval_policy: Some("never".into()),
        max_parallel_turns: 2,
        mcp_backchannel_socket: None,
    }
}

// Pin the Lens-E spawn-site invariant: every codex turn (fresh or resume)
// must spawn with current_dir bound to the per-pane cwd. `-C <cwd>` is
// dropped on resume, so this is the only consistent channel.
#[test]
fn prepare_codex_command_binds_current_dir_with_empty_args() {
    let cwd = Path::new("/tmp/pane0-cwd");
    let cmd = super::prepare_codex_command(cwd, Vec::new());
    assert_eq!(cmd.get_current_dir(), Some(cwd));
}

#[test]
fn prepare_codex_command_binds_current_dir_when_args_present() {
    let cwd = Path::new("/tmp/pane2-after-dir-change");
    let argv = vec![
        "exec".into(),
        "-C".into(),
        cwd.to_string_lossy().into_owned(),
        "--model".into(),
        "gpt-5".into(),
    ];
    let cmd = super::prepare_codex_command(cwd, argv);
    assert_eq!(cmd.get_current_dir(), Some(cwd));
}

#[test]
fn extracts_thread_id_from_event_stream() {
    let jsonl = br#"{"type":"thread.started","thread_id":"019ca7c5-536f-7f81-82a7-7a38fa483cb2"}
{"type":"turn.started"}
{"type":"turn.completed"}"#;
    assert_eq!(
        extract_thread_id_from_jsonl(jsonl).as_deref(),
        Some("019ca7c5-536f-7f81-82a7-7a38fa483cb2")
    );
}

#[test]
fn ignores_empty_thread_id() {
    let jsonl = br#"{"type":"thread.started","thread_id":"  "}
{"type":"turn.started"}"#;
    assert!(extract_thread_id_from_jsonl(jsonl).is_none());
}

#[test]
fn swarm_clone_agent_ids_resolve_to_base_model_slug() {
    assert_eq!(
        codex_model_slug_for_agent_id("gpt-5.2#swarm-mis-001-clone-01"),
        "gpt-5.2"
    );
    assert_eq!(codex_model_slug_for_agent_id("gpt-5.2"), "gpt-5.2");
}

#[test]
fn chat_clone_agent_ids_resolve_to_base_model_slug() {
    assert_eq!(
        codex_model_slug_for_agent_id("gpt-5.4#chat-clone-01"),
        "gpt-5.4"
    );
    assert_eq!(
        codex_model_slug_for_agent_id("gpt-5.4#chat-clone-12"),
        "gpt-5.4"
    );
}

#[test]
fn shadow_clone_agent_ids_resolve_to_base_model_slug() {
    assert_eq!(
        codex_model_slug_for_agent_id("gpt-5.2#shadow-01-propose-a"),
        "gpt-5.2"
    );
    assert_eq!(
        codex_model_slug_for_agent_id("gpt-5.4#shadow-07-judge"),
        "gpt-5.4"
    );
}

// Regression: original suffix table omitted `#mp-pane-`, so multipane-spawned
// turns went out with the full id and Codex/Claude rejected with "selected
// model (… #mp-pane-NN) does not exist". Strips on the FIRST `#`.
#[test]
fn multipane_pane_agent_ids_resolve_to_base_model_slug() {
    assert_eq!(codex_model_slug_for_agent_id("gpt-5#mp-pane-00"), "gpt-5");
    assert_eq!(
        codex_model_slug_for_agent_id("gpt-5.4#mp-pane-12"),
        "gpt-5.4"
    );
}

// Multipane lane spawns a swarm: pane suffix prepended + swarm suffix appended.
// `<base>#mp-pane-NN#swarm-…`. Slug stripper splits on the FIRST `#`.
#[test]
fn nested_multipane_swarm_clone_agent_ids_resolve_to_base_model_slug() {
    assert_eq!(
        codex_model_slug_for_agent_id("gpt-5#mp-pane-01#swarm-mis-001-clone-01"),
        "gpt-5"
    );
    assert_eq!(
        codex_model_slug_for_agent_id("gpt-5#mp-pane-01#chat-clone-02"),
        "gpt-5"
    );
}

#[test]
fn mcp_new_session_uses_base_model_slug_for_swarm_clone() {
    let config = custom_config();

    let (tool_name, arguments) = build_codex_mcp_tool_call(
        "gpt-5.2#swarm-mis-001-clone-01",
        "solve it",
        Path::new("/tmp/work"),
        Some("high"),
        &config,
        None,
        false,
    );

    assert_eq!(tool_name, "codex");
    assert_eq!(
        arguments,
        json!({
            "prompt": "solve it",
            "model": "gpt-5.2",
            "cwd": "/tmp/work",
            "config": { "model_reasoning_effort": "high" },
            "sandbox": "workspace-write",
            "approval-policy": "never"
        })
    );
}

#[test]
fn mcp_resume_uses_codex_reply_without_model_lookup() {
    let config = CodexRunnerConfig::default();

    let (tool_name, arguments) = build_codex_mcp_tool_call(
        "gpt-5.2#swarm-mis-001-clone-01",
        "continue",
        Path::new("/tmp/work"),
        Some("high"),
        &config,
        Some("thread-123"),
        false,
    );

    assert_eq!(tool_name, "codex-reply");
    assert_eq!(
        arguments,
        json!({
            "threadId": "thread-123",
            "prompt": "continue"
        })
    );
}

#[test]
fn exec_args_use_base_model_slug_for_swarm_clone() {
    let config = custom_config();

    let args = build_codex_exec_args(
        "gpt-5.2#swarm-mis-001-clone-01",
        Path::new("/tmp/work"),
        false,
        Some("high"),
        Path::new("/tmp/out.txt"),
        None,
        false,
        &config,
    );

    assert_eq!(
        args,
        vec![
            "-a",
            "never",
            "-s",
            "workspace-write",
            "exec",
            "--json",
            "--color",
            "never",
            "--ephemeral",
            "-m",
            "gpt-5.2",
            "-C",
            "/tmp/work",
            "-c",
            "model_reasoning_effort=\"high\"",
            "-o",
            "/tmp/out.txt",
            "-",
        ]
    );
}

#[test]
fn exec_resume_args_use_base_model_slug_for_swarm_clone() {
    let config = CodexRunnerConfig::default();

    let args = build_codex_exec_args(
        "gpt-5.2#swarm-mis-001-clone-01",
        Path::new("/tmp/work"),
        true,
        Some("medium"),
        Path::new("/tmp/out.txt"),
        Some("thread-123"),
        false,
        &config,
    );

    assert_eq!(
        args,
        vec![
            "-a",
            "never",
            "exec",
            "resume",
            "--json",
            "-m",
            "gpt-5.2",
            "-c",
            "model_reasoning_effort=\"medium\"",
            "-o",
            "/tmp/out.txt",
            "thread-123",
            "-",
        ]
    );
}

#[test]
fn read_only_shadow_turn_forces_read_only_sandbox_in_mcp_args() {
    let config = custom_config();

    let (_, arguments) = build_codex_mcp_tool_call(
        "gpt-5.2#shadow-01-propose-a",
        "propose something",
        Path::new("/tmp/work"),
        Some("medium"),
        &config,
        None,
        true,
    );
    assert_eq!(
        arguments.get("sandbox"),
        Some(&json!("read-only")),
        "shadow turn must override sandbox to read-only regardless of config"
    );
}

#[test]
fn read_only_shadow_turn_forces_read_only_sandbox_in_exec_args() {
    let config = custom_config();

    let args = build_codex_exec_args(
        "gpt-5.2#shadow-01-judge",
        Path::new("/tmp/work"),
        false,
        Some("medium"),
        Path::new("/tmp/out.txt"),
        None,
        true,
        &config,
    );
    let pos = args
        .iter()
        .position(|a| a == "-s")
        .expect("-s flag present");
    assert_eq!(args.get(pos + 1).map(String::as_str), Some("read-only"));
    assert!(!args.contains(&"workspace-write".to_string()));
}

#[test]
fn extracts_last_token_count_from_wrapped_events() {
    let jsonl = br#"{"timestamp":"t","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"total_tokens":100},"model_context_window":1000}}}
{"timestamp":"t","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"total_tokens":250},"model_context_window":1000}}}"#;
    assert_eq!(
        extract_token_count_from_jsonl(jsonl),
        Some(AgentTokenCount {
            total_tokens: 250,
            context_window: 1000
        })
    );
}

#[test]
fn token_count_prefers_last_token_usage_over_lifetime_totals() {
    let jsonl = br#"{"type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"total_tokens":999999},"last_token_usage":{"total_tokens":1234},"model_context_window":10000}}}"#;
    assert_eq!(
        extract_token_count_from_jsonl(jsonl),
        Some(AgentTokenCount {
            total_tokens: 1234,
            context_window: 10000
        })
    );
}

#[test]
fn extracts_token_count_from_turn_completed_usage() {
    let jsonl = br#"{"type":"thread.started","thread_id":"thread-123"}
{"type":"turn.started"}
{"type":"turn.completed","usage":{"input_tokens":10916,"cached_input_tokens":9984,"output_tokens":72}}"#;
    assert_eq!(
        extract_token_count_from_jsonl(jsonl),
        Some(AgentTokenCount {
            total_tokens: 10988,
            context_window: 0
        })
    );
}

#[test]
fn mcp_token_count_notifications_emit_agent_bus_token_count() {
    let (tx, rx) = mpsc::channel::<AgentBusEvent>();
    let value = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "codex/event",
        "params": {
            "_meta": { "requestId": 42 },
            "msg": {
                "type": "token_count",
                "info": {
                    "total_token_usage": { "total_tokens": 123 },
                    "model_context_window": 1000
                }
            }
        }
    });

    let mut last_stage = None;
    let mut last_stage_sent_at = Instant::now();
    let mut last_token_count = None;
    let cwd = std::path::PathBuf::from("/tmp");
    assert!(handle_codex_mcp_notification(
        &tx,
        "gpt-test",
        None,
        42,
        &cwd,
        &value,
        &mut last_stage,
        &mut last_stage_sent_at,
        &mut last_token_count,
    ));
    assert_eq!(
        last_token_count,
        Some(AgentTokenCount {
            total_tokens: 123,
            context_window: 1000
        })
    );

    let mut saw_token_count = false;
    while let Ok(event) = rx.try_recv() {
        if let AgentBusEvent::TokenCount { token_count, .. } = event {
            assert_eq!(
                token_count,
                AgentTokenCount {
                    total_tokens: 123,
                    context_window: 1000
                }
            );
            saw_token_count = true;
            break;
        }
    }
    assert!(saw_token_count);
}

// --- append_stdout_line_capped --------------------------------------------
//
// The cap path runs inside spawn_turn_worker's stdout reader thread; a bug
// here would corrupt every Codex turn — guarantee no panic / OOB / silent
// drop of the wrong bytes.

#[test]
fn cap_is_noop_when_under_threshold() {
    let mut buf = Vec::new();
    append_stdout_line_capped(&mut buf, b"hello\n");
    append_stdout_line_capped(&mut buf, b"world\n");
    assert_eq!(buf, b"hello\nworld\n");
}

#[test]
fn cap_handles_empty_input_safely() {
    let mut buf = Vec::new();
    append_stdout_line_capped(&mut buf, b"");
    assert!(buf.is_empty());
    append_stdout_line_capped(&mut buf, b"a\n");
    append_stdout_line_capped(&mut buf, b"");
    assert_eq!(buf, b"a\n");
}

#[test]
fn cap_drains_at_newline_boundary_on_overflow() {
    // Build a buffer just over the cap with frequent newlines, then push
    // one more line to trip the drain. The result must still parse as JSONL
    // (every retained line bounded by '\n').
    let line: Vec<u8> = {
        let mut v = vec![b'x'; 1023];
        v.push(b'\n');
        v
    };
    let mut buf = Vec::with_capacity(STDOUT_TAIL_CAP_BYTES + 4096);
    while buf.len() + line.len() <= STDOUT_TAIL_CAP_BYTES {
        buf.extend_from_slice(&line);
    }
    let pre_overflow_len = buf.len();
    assert!(pre_overflow_len <= STDOUT_TAIL_CAP_BYTES);
    append_stdout_line_capped(&mut buf, &line);
    // Must have shrunk to ≤75% of the cap.
    assert!(buf.len() <= STDOUT_TAIL_CAP_BYTES);
    assert!(buf.len() >= STDOUT_TAIL_CAP_BYTES * 3 / 4 - line.len());
    // First byte after the drain is the start of a complete record (the
    // previous byte was '\n'), so the trailing buffer ends in '\n' and
    // contains only complete records.
    assert_eq!(buf.last().copied(), Some(b'\n'));
    // Every byte is either '\n' or 'x' from our padding — proves no record
    // got split mid-way.
    assert!(buf.iter().all(|&b| b == b'x' || b == b'\n'));
}

// A pathological subprocess emits a single huge byte stream with no newline.
// The cap can't preserve a record boundary, so it must drain everything
// rather than leave a half-record. Critical: bounded final size, no panic.
#[test]
fn cap_truncates_single_mega_line_with_no_newline() {
    let mut buf = Vec::with_capacity(STDOUT_TAIL_CAP_BYTES + 1);
    let huge: Vec<u8> = vec![b'A'; STDOUT_TAIL_CAP_BYTES + 1];
    append_stdout_line_capped(&mut buf, &huge);
    assert!(buf.is_empty(), "no newline ⇒ drain everything");
}

// Single line bigger than the cap: only the terminating '\n' is in the
// trailing 75% slice, so drain runs through end-of-buffer.
#[test]
fn cap_truncates_single_mega_line_with_terminal_newline() {
    let mut buf = Vec::with_capacity(STDOUT_TAIL_CAP_BYTES + 1);
    let mut huge: Vec<u8> = vec![b'A'; STDOUT_TAIL_CAP_BYTES];
    huge.push(b'\n');
    append_stdout_line_capped(&mut buf, &huge);
    assert!(buf.is_empty());
}

#[test]
fn cap_stays_bounded_under_repeated_overflow() {
    // Append 4× the cap worth of data. The buffer must never exceed the cap
    // and must always end on a newline.
    let line: Vec<u8> = {
        let mut v = vec![b'y'; 4095];
        v.push(b'\n');
        v
    };
    let mut buf = Vec::with_capacity(STDOUT_TAIL_CAP_BYTES + 4096);
    let target = STDOUT_TAIL_CAP_BYTES * 4;
    let mut written = 0usize;
    while written < target {
        append_stdout_line_capped(&mut buf, &line);
        written += line.len();
        assert!(
            buf.len() <= STDOUT_TAIL_CAP_BYTES,
            "buf grew past cap: {} > {}",
            buf.len(),
            STDOUT_TAIL_CAP_BYTES
        );
    }
    assert_eq!(buf.last().copied(), Some(b'\n'));
}

// --- push_json_error_capped ----------------------------------------------

#[test]
fn json_errors_cap_keeps_size_bounded() {
    let mut errors: Vec<String> = Vec::new();
    for i in 0..(JSON_ERRORS_CAP * 4) {
        push_json_error_capped(&mut errors, format!("err{i}"));
    }
    assert!(errors.len() <= JSON_ERRORS_CAP);
    assert_eq!(
        errors.last().unwrap(),
        &format!("err{}", JSON_ERRORS_CAP * 4 - 1)
    );
}

#[test]
fn json_errors_cap_drains_oldest_half_on_overflow() {
    let mut errors: Vec<String> = (0..JSON_ERRORS_CAP).map(|i| format!("e{i}")).collect();
    push_json_error_capped(&mut errors, "new".into());
    // After the drain (oldest 128) + push (1), len = 256 - 128 + 1 = 129.
    assert_eq!(errors.len(), JSON_ERRORS_CAP / 2 + 1);
    assert_eq!(errors.last().unwrap(), "new");
    assert_eq!(
        errors.first().unwrap(),
        &format!("e{}", JSON_ERRORS_CAP / 2)
    );
}

#[test]
fn queue_len_returns_to_zero_after_cancel_all_with_queued_turns() {
    use nit_core::state::{AgentLane, AgentLaneKind, AgentStatus, AppState};
    use nit_core::OPERATOR_CANCEL_TURN_MESSAGE;

    let editor = nit_core::Buffer::from_str("editor", "", None);
    let notes = nit_core::Buffer::from_str("notes", "", None);
    let mut state = AppState::new(std::path::PathBuf::from("."), editor, notes);
    state.agents.agents.push(AgentLane {
        id: "gpt-test".into(),
        role: "gpt-test".into(),
        lane: "Codex".into(),
        kind: AgentLaneKind::Codex,
        status: AgentStatus::Running,
        heartbeat_age_secs: 0,
        // 3 dispatch increments: 1 active + 2 runner-queued.
        queue_len: 3,
        current_mission: None,
        last_message: String::new(),
        shadow: false,
    });
    state.agents.rebuild_agents_index();

    let pending = |lane: &str, mission: &str| super::CodexCommand::RunTurn {
        model: lane.into(),
        cwd: ".".into(),
        mission_id: Some(mission.into()),
        resume_thread_id: None,
        persist_session: false,
        reasoning_effort: None,
        prompt: "queued".into(),
        read_only: true,
    };
    let mut queue = std::collections::VecDeque::from([
        pending("gpt-test", "first"),
        pending("other", "unrelated"),
        pending("gpt-test", "second"),
    ]);
    let (tx, rx) = std::sync::mpsc::channel();
    super::cancel_queued_turns(&mut queue, &tx, Some("other"));
    assert!(
        matches!(rx.try_recv().unwrap(), AgentBusEvent::TurnFailed { agent_id, .. } if agent_id == "other")
    );
    assert_eq!(queue.len(), 2);

    super::cancel_queued_turns(&mut queue, &tx, None);
    super::cancel_queued_turns(&mut queue, &tx, None);
    assert!(queue.is_empty());
    for expected in ["first", "second"] {
        let event = rx.try_recv().unwrap();
        assert!(
            matches!(&event, AgentBusEvent::TurnFailed { agent_id, mission_id: Some(mission), message, .. } if agent_id == "gpt-test" && mission == expected && message == OPERATOR_CANCEL_TURN_MESSAGE)
        );
        event.apply(&mut state);
    }
    assert!(rx.try_recv().is_err());
    assert_eq!(state.agents.agents_get("gpt-test").unwrap().queue_len, 1);
    // The active worker reports its own cancellation when it exits.
    AgentBusEvent::TurnFailed {
        agent_id: "gpt-test".into(),
        mission_id: None,
        thread_id: None,
        token_count: None,
        message: OPERATOR_CANCEL_TURN_MESSAGE.into(),
    }
    .apply(&mut state);

    let agent = state.agents.agents_get("gpt-test").unwrap();
    assert_eq!(agent.queue_len, 0);
}

#[test]
fn mcp_tool_error_is_distinct_from_a_model_reply() {
    let provider_error = r#"{"type":"error","status":400,"error":{"message":"Unsupported model"}}"#;
    let mut response = json!({"result": {"isError": true, "structuredContent": {"threadId":"abc", "content": provider_error}}});
    assert_eq!(
        super::mcp_tool_failure(&response).as_deref(),
        Some("Unsupported model")
    );
    response["result"]["isError"] = json!(false);
    assert!(super::mcp_tool_failure(&response).is_none());
    assert_eq!(
        super::mcp_tool_failure(
            &json!({"result":{"isError":true,"content":[{"type":"text","text":"No credentials"}]}})
        )
        .as_deref(),
        Some("No credentials")
    );
}

#[test]
fn only_a_matching_completed_final_item_can_publish_an_exec_reply() {
    use super::jsonl::ExecReply;
    let started = json!({"type":"thread.started","thread_id":"thread-a"});
    let final_item =
        json!({"type":"item.completed","item":{"type":"agent_message","text":"Answer."}});
    let complete = json!({"type":"turn.completed","usage":{"input_tokens":7,"output_tokens":3}});
    for (id, items, valid) in [
        (
            Some("thread-a"),
            vec![started.clone(), final_item.clone()],
            true,
        ),
        (
            Some("other-thread"),
            vec![started.clone(), final_item.clone()],
            false,
        ),
        (None, vec![final_item.clone()], false),
        (
            None,
            vec![
                started.clone(),
                json!({"type":"item.completed","item":{"type":"agent_message","phase":"commentary","text":"Checking..."}}),
            ],
            false,
        ),
        (
            None,
            vec![
                started.clone(),
                final_item.clone(),
                json!({"type":"item.started","item":{"type":"command_execution"}}),
            ],
            false,
        ),
        (
            None,
            vec![started, final_item, json!({"type":"turn.failed"})],
            false,
        ),
    ] {
        let mut capture = ExecReply::default();
        for item in items {
            assert!(capture.observe(&item, id).is_none());
        }
        let actual = capture.observe(&complete, id);
        assert_eq!(actual.is_some(), valid);
        if let Some((id, text, Some(tokens))) = actual {
            assert_eq!(
                (id.as_str(), text.as_str(), tokens.total_tokens),
                ("thread-a", "Answer.", 10)
            );
        }
        assert!(capture.observe(&complete, id).is_none());
    }
}

#[cfg(unix)]
#[test]
fn exec_publishes_before_exit_and_reaps_without_a_second_terminal_event() {
    use std::{process::Command, sync::Arc, thread, time::Duration};
    for exit in [0, 9] {
        let dir =
            std::env::temp_dir().join(format!("nit-protocol-exit-{}-{exit}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let gate = dir.join("exit-gate");
        let _ = std::fs::remove_file(&gate);
        let output = dir.join("last-message");
        let mut cmd = Command::new("sh");
        cmd.current_dir(&dir).args([
            "-c",
            r#"
cat >/dev/null
echo '{"type":"thread.started","thread_id":"session"}'
echo '{"type":"item.completed","item":{"type":"agent_message","text":"Provider answer"}}'
echo '{"type":"turn.completed","usage":{"input_tokens":5,"output_tokens":3}}'
i=0
while [ ! -e exit-gate ] && [ "$i" -lt 120 ]; do sleep 0.05; i=$((i+1)); done
exit "$1"
"#,
            "shim",
            &exit.to_string(),
        ]);
        let (tx, rx) = mpsc::channel();
        let worker = thread::spawn(move || {
            super::run_exec_command(
                cmd,
                &tx,
                1,
                "codex".into(),
                None,
                Some("session".into()),
                "question".into(),
                output,
                Arc::new(super::ExecStop::default()),
                Instant::now(),
            )
        });
        loop {
            match rx
                .recv_timeout(Duration::from_secs(3))
                .expect("provider reply must be available before exit gate opens")
            {
                AgentBusEvent::TurnCompleted {
                    message,
                    thread_id,
                    token_count,
                    ..
                } => {
                    assert_eq!(message, "Provider answer");
                    assert_eq!(thread_id.as_deref(), Some("session"));
                    assert_eq!(token_count.unwrap().total_tokens, 8);
                    break;
                }
                AgentBusEvent::TurnFailed { message, .. } => panic!("{message}"),
                _ => {}
            }
        }
        assert!(!worker.is_finished());
        std::fs::write(&gate, []).unwrap();
        worker.join().unwrap();
        let after = rx.try_iter().collect::<Vec<_>>();
        assert!(!after.iter().any(|e| matches!(
            e,
            AgentBusEvent::TurnCompleted { .. } | AgentBusEvent::TurnFailed { .. }
        )));
        if exit != 0 {
            assert!(after.iter().any(|e|matches!(e,AgentBusEvent::DiagnosticAppend {event} if event.severity==nit_core::AgentAlertSeverity::Warn)));
        }
        std::fs::remove_dir_all(dir).unwrap();
    }
}

#[cfg(unix)]
#[test]
fn exec_falls_back_to_last_message_and_honors_failures_and_cancellation() {
    use std::{process::Command, sync::Arc};
    for (case, script, expected_success) in [
        (
            "legacy",
            "cat >/dev/null; echo 'Legacy document' > last-message",
            true,
        ),
        (
            "protocol-failure",
            r#"cat >/dev/null; echo 'Partial' > last-message; echo '{"type":"turn.failed","error":{"message":"provider failed"}}'"#,
            false,
        ),
        (
            "exit-failure",
            "cat >/dev/null; echo 'provider unavailable' >&2; exit 1",
            false,
        ),
        ("cancel", "cat >/dev/null; sleep 0.3", false),
    ] {
        let root = tempfile::tempdir().unwrap();
        let mut command = Command::new("sh");
        command.current_dir(root.path()).args(["-c", script]);
        let stop = Arc::new(super::ExecStop::default());
        stop.cancelled
            .store(case == "cancel", std::sync::atomic::Ordering::Release);
        let (tx, rx) = mpsc::channel();
        super::run_exec_command(
            command,
            &tx,
            1,
            "codex".into(),
            None,
            None,
            "prompt".into(),
            root.path().join("last-message"),
            stop,
            Instant::now(),
        );
        drop(tx);
        let terminal: Vec<_> = rx
            .into_iter()
            .filter(|e| {
                matches!(
                    e,
                    AgentBusEvent::TurnCompleted { .. } | AgentBusEvent::TurnFailed { .. }
                )
            })
            .collect();
        assert_eq!(terminal.len(), 1, "{case}");
        match &terminal[0] {
            AgentBusEvent::TurnCompleted { message, .. } => {
                assert!(expected_success);
                assert_eq!(message, "Legacy document");
            }
            AgentBusEvent::TurnFailed { message, .. } => {
                assert!(!expected_success);
                if case == "cancel" {
                    assert_eq!(message, nit_core::OPERATOR_CANCEL_TURN_MESSAGE);
                }
            }
            _ => unreachable!(),
        }
    }
}

#[cfg(unix)]
#[test]
fn exec_resumes_same_lane_while_previous_cli_is_still_draining() {
    use std::{os::unix::fs::PermissionsExt, process::Command, thread, time::Duration};
    const KEY: &str = "NIT_TEST_RESUME_DRAIN_TMP";
    if let Some(dir) = std::env::var_os(KEY) {
        let dir = std::path::PathBuf::from(dir);
        let (request, cmds) = mpsc::channel();
        let (events, rx) = mpsc::channel();
        let runner = thread::spawn(move || {
            super::runner_loop_exec(
                cmds,
                events,
                CodexRunnerConfig {
                    max_parallel_turns: 1,
                    ..custom_config()
                },
            )
        });
        for (prompt, resume) in [("first", None), ("second", Some("saved-session"))] {
            request
                .send(super::CodexCommand::RunTurn {
                    model: "codex".into(),
                    cwd: dir.clone(),
                    mission_id: None,
                    resume_thread_id: resume.map(str::to_owned),
                    persist_session: true,
                    reasoning_effort: None,
                    prompt: prompt.into(),
                    read_only: true,
                })
                .unwrap();
            loop {
                match rx
                    .recv_timeout(Duration::from_secs(3))
                    .expect("next turn must run before the old exit gate opens")
                {
                    AgentBusEvent::TurnCompleted {
                        message, thread_id, ..
                    } => {
                        assert_eq!(message, prompt);
                        assert_eq!(thread_id.as_deref(), Some("saved-session"));
                        break;
                    }
                    AgentBusEvent::TurnFailed { message, .. } => panic!("{message}"),
                    _ => {}
                }
            }
        }
        std::fs::write(dir.join("exit-gate"), []).unwrap();
        let mut stale = false;
        while let Ok(event) = rx.recv_timeout(Duration::from_millis(400)) {
            if matches!(
                event,
                AgentBusEvent::TokenCount { .. }
                    | AgentBusEvent::TurnStage { .. }
                    | AgentBusEvent::TurnLog { .. }
                    | AgentBusEvent::TurnFailed { .. }
                    | AgentBusEvent::TurnCompleted { .. }
                    | AgentBusEvent::McpStatus { .. }
            ) {
                stale = true;
            }
        }
        request.send(super::CodexCommand::Shutdown).unwrap();
        runner.join().unwrap();
        assert!(
            !stale,
            "finished worker changed the lane after the next reply"
        );
        return;
    }
    // Isolate the fake CLI on this test process's PATH without changing the
    // environment observed by parallel tests.
    let dir = tempfile::tempdir().unwrap();
    let cli = dir.path().join("codex");
    std::fs::write(
        &cli,
        r#"#!/bin/sh
prompt=$(cat)
if [ "$prompt" = second ]; then
    case " $* " in *" resume "*" saved-session "*) ;; *) echo 'missing resume ID' >&2; exit 1;; esac
fi
echo '{"type":"thread.started","thread_id":"saved-session"}'
printf '{"type":"item.completed","item":{"type":"agent_message","text":"%s"}}\n' "$prompt"
echo '{"type":"turn.completed","usage":{"input_tokens":2}}'
if [ "$prompt" = first ]; then
    i=0
    while [ ! -e exit-gate ] && [ "$i" -lt 100 ]; do sleep 0.05; i=$((i+1)); done
    echo '{"type":"tool.started"}'
    echo '{"type":"token_count","info":{"context_window":500,"total_tokens":450}}'
    echo 'old CLI cleanup' >&2
fi
"#,
    )
    .unwrap();
    std::fs::set_permissions(&cli, std::fs::Permissions::from_mode(0o700)).unwrap();
    let path = std::env::join_paths(std::iter::once(dir.path().to_owned()).chain(
        std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()),
    ))
    .unwrap();
    let output = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "codex_runner::tests::exec_resumes_same_lane_while_previous_cli_is_still_draining",
            "--nocapture",
        ])
        .env(KEY, dir.path())
        .env("PATH", path)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[cfg(unix)]
#[test]
fn finished_or_failed_cli_releases_worker_while_descendant_holds_streams() {
    use std::{process::Command, sync::Arc, thread, time::Duration};
    for status in [0, 1] {
        let root = tempfile::tempdir().unwrap();
        let mut cmd = Command::new("sh");
        cmd.current_dir(root.path()).args([
            "-c",
            r#"
cat >/dev/null
sleep 20 &
echo "$!" > descendant
echo '{"type":"thread.started","thread_id":"session"}'
echo '{"type":"item.completed","item":{"type":"agent_message","text":"Done"}}'
if [ "$1" = 0 ]; then echo '{"type":"turn.completed"}'; fi
exit "$1"
"#,
            "shim",
            &status.to_string(),
        ]);
        let (tx, rx) = mpsc::channel();
        let (done, finished) = mpsc::channel();
        let file = root.path().join("last");
        let worker = thread::spawn(move || {
            super::run_exec_command(
                cmd,
                &tx,
                1,
                "codex".into(),
                None,
                None,
                String::new(),
                file,
                Arc::new(super::ExecStop::default()),
                Instant::now(),
            );
            done.send(()).unwrap();
        });
        let released = finished.recv_timeout(Duration::from_secs(2)).is_ok();
        let pid = std::fs::read_to_string(root.path().join("descendant")).unwrap();
        let _ = Command::new("kill").arg(pid.trim()).status();
        worker.join().unwrap();
        assert!(released, "inherited streams retained an execution worker");
        assert_eq!(
            rx.try_iter()
                .filter(|event| match event {
                    AgentBusEvent::TurnCompleted { .. } => status == 0,
                    AgentBusEvent::TurnFailed { .. } => status != 0,
                    _ => false,
                })
                .count(),
            1
        );
    }
}
