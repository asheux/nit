use super::*;

fn has_arg(args: &[String], expected: &str) -> bool {
    args.iter().any(|a| a == expected)
}

// Pins propose-03 important #3: when the runner-internal queue is
// non-empty (`max_parallel_turns` clamped) and the operator triggers a
// `CancelAll` / `CancelTurn`, today's `runner_loop` calls `queue.clear()`
// or `queue.retain(...)` without emitting `TurnFailed` for the dropped
// queued commands. State-side `queue_len` was incremented by
// `enqueue_claude_turn` at dispatch time; without a matching
// `TurnFailed`, the bus-side decrement at `agent_bus.rs:482` never
// runs and the roster carries ghost queue rows until the agent is
// reaped. The fix lives in `claude_runner.rs`'s `CancelAll` /
// `CancelTurn` arms — once they emit `TurnFailed { message:
// OPERATOR_CANCEL_TURN_MESSAGE, ... }` per dropped queued turn, this
// test passes and `#[ignore]` should be removed.
#[test]
#[ignore = "fails until claude_runner::CancelAll emits TurnFailed for dropped queued items"]
fn queue_len_returns_to_zero_after_cancel_all_with_queued_turns() {
    use nit_core::state::{AgentLane, AgentLaneKind, AgentStatus, AppState};
    use nit_core::{AgentBusEvent, OPERATOR_CANCEL_TURN_MESSAGE};

    let editor = nit_core::Buffer::from_str("editor", "", None);
    let notes = nit_core::Buffer::from_str("notes", "", None);
    let mut state = AppState::new(std::path::PathBuf::from("."), editor, notes);
    state.agents.agents.push(AgentLane {
        id: "claude-opus".into(),
        role: "claude-opus".into(),
        lane: "Claude".into(),
        kind: AgentLaneKind::Claude,
        status: AgentStatus::Running,
        heartbeat_age_secs: 0,
        // 3 queue increments from dispatch: 1 active + 2 in the
        // runner-internal queue (assuming max_parallel_turns = 1).
        queue_len: 3,
        current_mission: None,
        last_message: String::new(),
        shadow: false,
    });
    state.agents.rebuild_agents_index();

    // Today CancelAll only kills the active turn → exactly one
    // TurnFailed is emitted. Simulate that single event:
    AgentBusEvent::TurnFailed {
        agent_id: "claude-opus".into(),
        mission_id: None,
        thread_id: None,
        token_count: None,
        message: OPERATOR_CANCEL_TURN_MESSAGE.into(),
    }
    .apply(&mut state);

    let agent = state.agents.agents_get("claude-opus").unwrap();
    assert_eq!(
        agent.queue_len, 0,
        "after CancelAll every queue_len increment must be paired with a \
         TurnFailed decrement; currently leaks because runner does not \
         emit TurnFailed for runner-queued turns"
    );
}

#[test]
fn test_claude_model_slug_for_agent_id() {
    let cases = [
        ("claude-opus-4-6", "claude-opus-4-6"),
        ("claude-opus-4-6#swarm-mis-001-clone-01", "claude-opus-4-6"),
        ("claude-sonnet-4-6#chat-clone-02", "claude-sonnet-4-6"),
        ("claude-opus-4-6#shadow-01-propose-a", "claude-opus-4-6"),
        ("claude-sonnet-4-6#shadow-07-judge", "claude-sonnet-4-6"),
        // Regression: the original suffix table missed `#mp-pane-`.
        // Multipane turns ran out with the full agent_id as the CLI
        // model name, and Claude rejected ("selected model (…) does
        // not exist"). The slug stripper now splits on the FIRST `#`.
        ("claude-haiku-4-5#mp-pane-00", "claude-haiku-4-5"),
        ("claude-opus-4-7#mp-pane-12", "claude-opus-4-7"),
        // Nested: multipane lane spawns a swarm. Pane suffix
        // prepended, swarm suffix appended; both peel on first `#`.
        (
            "claude-opus-4-7#mp-pane-01#swarm-mis-001-clone-01",
            "claude-opus-4-7",
        ),
        (
            "claude-haiku-4-5#mp-pane-03#shadow-07-judge",
            "claude-haiku-4-5",
        ),
    ];
    for (input, expected) in cases {
        assert_eq!(claude_model_slug_for_agent_id(input), expected);
    }
}

#[test]
fn test_extract_session_id() {
    let jsonl =
        br#"{"type":"system","subtype":"init","session_id":"abc-123-def","tools":[],"model":"opus"}
{"type":"assistant","message":"Hello"}
"#;
    assert_eq!(
        extract_session_id_from_jsonl(jsonl),
        Some("abc-123-def".to_string())
    );
}

#[test]
fn test_extract_result_text() {
    let jsonl = br#"{"type":"system","subtype":"init","session_id":"abc"}
{"type":"assistant","message":"working on it..."}
{"type":"result","result":"Here is the answer","usage":{"input_tokens":100,"output_tokens":50}}
"#;
    assert_eq!(
        extract_result_text_from_jsonl(jsonl),
        Some("Here is the answer".to_string())
    );
}

#[test]
fn test_token_count_from_result_event() {
    let value: serde_json::Value = serde_json::json!({
        "type": "result",
        "result": "done",
        "usage": {
            "input_tokens": 100,
            "output_tokens": 50,
            "cache_creation_input_tokens": 9000,
            "cache_read_input_tokens": 6000
        },
        "modelUsage": {
            "claude-opus-4-6": {"contextWindow": 200000, "inputTokens": 100, "outputTokens": 50}
        }
    });
    let count = claude_token_count_from_value(&value).unwrap();
    assert_eq!(count.total_tokens, 100 + 50 + 9000 + 6000);
    assert_eq!(count.context_window, 200000);
}

#[test]
fn test_token_count_from_assistant_event() {
    // Assistant events have usage nested under "message"
    let value: serde_json::Value = serde_json::json!({
        "type": "assistant",
        "message": {
            "usage": {"input_tokens": 500, "output_tokens": 200}
        }
    });
    let count = claude_token_count_from_value(&value).unwrap();
    assert_eq!(count.total_tokens, 700);
    assert_eq!(count.context_window, 0);
}

#[test]
fn test_build_claude_args_basic() {
    let config = ClaudeRunnerConfig::default();
    let args = build_claude_args(
        "claude-opus-4-6",
        Path::new("/tmp/project"),
        true,
        Some("high"),
        Path::new("/tmp/out.txt"),
        None,
        false,
        None,
        &config,
    );
    assert!(has_arg(&args, "-p"));
    assert!(has_arg(&args, "stream-json"));
    assert!(has_arg(&args, "claude-opus-4-6"));
    assert!(has_arg(&args, "high"));
    assert!(!has_arg(&args, "--no-session-persistence"));
    assert!(has_arg(
        &args,
        "Read,Edit,Write,Bash,Glob,Grep,WebSearch,WebFetch"
    ));
}

#[test]
fn test_build_claude_args_resume() {
    let config = ClaudeRunnerConfig::default();
    let args = build_claude_args(
        "claude-sonnet-4-6",
        Path::new("/tmp/project"),
        false,
        None,
        Path::new("/tmp/out.txt"),
        Some("session-abc-123"),
        false,
        None,
        &config,
    );
    assert!(has_arg(&args, "--resume"));
    assert!(has_arg(&args, "session-abc-123"));
    assert!(has_arg(&args, "--no-session-persistence"));
}

#[test]
fn test_build_claude_args_read_only_for_shadow_turns() {
    let config = ClaudeRunnerConfig::default();
    let args = build_claude_args(
        "claude-opus-4-6#shadow-01-propose-a",
        Path::new("/tmp/project"),
        false,
        Some("medium"),
        Path::new("/tmp/out.txt"),
        None,
        true,
        None,
        &config,
    );
    // Read-only turns get a narrow tool allow-list with no Write/Edit/Bash.
    assert!(args.contains(&"Read,Glob,Grep".to_string()));
    assert!(!args
        .iter()
        .any(|a| a.contains("Write") || a.contains("Edit") || a.contains("Bash")));
}

#[test]
fn test_shorten_id() {
    assert_eq!(shorten_id("abcdefghij"), "abcdefgh…");
    assert_eq!(shorten_id("short"), "short");
}

// --- append_stdout_line_capped ---------------------------------------------
//
// Mirrors codex_runner cap tests; the helper has its own copy in this
// module and runs in the spawn_turn_worker stdout reader thread.

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
    let line: Vec<u8> = {
        let mut v = vec![b'x'; 1023];
        v.push(b'\n');
        v
    };
    let mut buf = Vec::with_capacity(STDOUT_TAIL_CAP_BYTES + 4096);
    while buf.len() + line.len() <= STDOUT_TAIL_CAP_BYTES {
        buf.extend_from_slice(&line);
    }
    append_stdout_line_capped(&mut buf, &line);
    assert!(buf.len() <= STDOUT_TAIL_CAP_BYTES);
    assert_eq!(buf.last().copied(), Some(b'\n'));
    assert!(buf.iter().all(|&b| b == b'x' || b == b'\n'));
}

#[test]
fn cap_truncates_single_mega_line_with_no_newline() {
    let mut buf = Vec::with_capacity(STDOUT_TAIL_CAP_BYTES + 1);
    let huge: Vec<u8> = vec![b'A'; STDOUT_TAIL_CAP_BYTES + 1];
    append_stdout_line_capped(&mut buf, &huge);
    assert!(buf.is_empty());
}

#[test]
fn cap_stays_bounded_under_repeated_overflow() {
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
        assert!(buf.len() <= STDOUT_TAIL_CAP_BYTES);
    }
    assert_eq!(buf.last().copied(), Some(b'\n'));
}

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

// --- idle-output reaper helpers --------------------------------------------

#[test]
fn is_stream_result_event_matches_only_result_type() {
    let result = serde_json::json!({"type": "result", "result": "ok"});
    let assistant = serde_json::json!({"type": "assistant", "message": {}});
    let untyped = serde_json::json!({"foo": "bar"});
    let wrong_kind = serde_json::json!({"type": 7});
    assert!(is_stream_result_event(&result));
    assert!(!is_stream_result_event(&assistant));
    assert!(!is_stream_result_event(&untyped));
    assert!(!is_stream_result_event(&wrong_kind));
}

// `claude_turn_idle_timeout` reads a process-wide env var, so these tests
// must run serially (cargo runs `#[test]` items in parallel by default).
// Bracketing the env var with set-then-remove around each assertion keeps
// the polluted state contained; the `_lock` mutex serializes against
// concurrent invocations of this single test entry.
#[test]
fn claude_turn_idle_timeout_env_parsing() {
    use std::sync::Mutex;
    static LOCK: Mutex<()> = Mutex::new(());
    let _guard = LOCK.lock().unwrap();
    const VAR: &str = "NIT_CLAUDE_TURN_IDLE_TIMEOUT_SECS";

    // Snapshot whatever the caller had set so we can restore it.
    let prior = std::env::var(VAR).ok();

    // Unset → default-on at 15 min.
    std::env::remove_var(VAR);
    assert_eq!(
        claude_turn_idle_timeout(),
        Some(std::time::Duration::from_secs(15 * 60))
    );

    // Empty / whitespace → default.
    std::env::set_var(VAR, "   ");
    assert_eq!(
        claude_turn_idle_timeout(),
        Some(std::time::Duration::from_secs(15 * 60))
    );

    // "0" → explicit disable.
    std::env::set_var(VAR, "0");
    assert_eq!(claude_turn_idle_timeout(), None);

    // Positive integer → that many seconds.
    std::env::set_var(VAR, "300");
    assert_eq!(
        claude_turn_idle_timeout(),
        Some(std::time::Duration::from_secs(300))
    );

    // Garbage → fall back to default rather than disabling.
    std::env::set_var(VAR, "not-a-number");
    assert_eq!(
        claude_turn_idle_timeout(),
        Some(std::time::Duration::from_secs(15 * 60))
    );

    // Restore caller's environment.
    match prior {
        Some(value) => std::env::set_var(VAR, value),
        None => std::env::remove_var(VAR),
    }
}
