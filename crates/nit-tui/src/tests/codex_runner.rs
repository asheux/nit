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

// Codex-side companion to claude_runner's queue_len parity test. Pins
// propose-03 important #3: when CancelAll/CancelTurn drops queued
// commands, today it does NOT emit TurnFailed for them, so the bus-side
// queue_len decrement at agent_bus.rs:482 never runs.
#[test]
#[ignore = "fails until codex_runner::CancelAll emits TurnFailed for dropped queued items"]
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

    AgentBusEvent::TurnFailed {
        agent_id: "gpt-test".into(),
        mission_id: None,
        thread_id: None,
        token_count: None,
        message: OPERATOR_CANCEL_TURN_MESSAGE.into(),
    }
    .apply(&mut state);

    let agent = state.agents.agents_get("gpt-test").unwrap();
    assert_eq!(
        agent.queue_len, 0,
        "after CancelAll every queue_len increment must be paired with a \
         TurnFailed decrement; currently leaks because the runner does \
         not emit TurnFailed for runner-queued turns"
    );
}
