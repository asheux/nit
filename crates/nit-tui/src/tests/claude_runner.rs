use super::*;

fn has_arg(args: &[String], expected: &str) -> bool {
    args.iter().any(|a| a == expected)
}

fn line_with_terminator(prefix: u8, body_len: usize) -> Vec<u8> {
    let mut v = vec![prefix; body_len];
    v.push(b'\n');
    v
}

fn fill_buffer_to_cap(line: &[u8]) -> Vec<u8> {
    let mut buf = Vec::with_capacity(STDOUT_TAIL_CAP_BYTES + 4096);
    while buf.len() + line.len() <= STDOUT_TAIL_CAP_BYTES {
        buf.extend_from_slice(line);
    }
    buf
}

fn lane_with_queue(
    id: &str,
    kind: nit_core::state::AgentLaneKind,
    queue_len: usize,
) -> nit_core::state::AgentLane {
    use nit_core::state::{AgentLane, AgentStatus};
    AgentLane {
        id: id.into(),
        role: id.into(),
        lane: match kind {
            nit_core::state::AgentLaneKind::Claude => "Claude".into(),
            nit_core::state::AgentLaneKind::Codex => "Codex".into(),
            _ => "Lane".into(),
        },
        kind,
        status: AgentStatus::Running,
        heartbeat_age_secs: 0,
        queue_len,
        current_mission: None,
        last_message: String::new(),
        shadow: false,
    }
}

// Pins propose-03 important #3: when the runner-internal queue is non-empty
// and the operator triggers CancelAll/CancelTurn, today's runner_loop calls
// queue.clear() / queue.retain() without emitting TurnFailed for the dropped
// queued commands. Without that emit, the bus-side queue_len decrement at
// agent_bus.rs:482 never runs and the roster carries ghost queue rows.
#[test]
#[ignore = "fails until claude_runner::CancelAll emits TurnFailed for dropped queued items"]
fn queue_len_returns_to_zero_after_cancel_all_with_queued_turns() {
    use nit_core::state::{AgentLaneKind, AppState};
    use nit_core::{AgentBusEvent, OPERATOR_CANCEL_TURN_MESSAGE};

    let editor = nit_core::Buffer::from_str("editor", "", None);
    let notes = nit_core::Buffer::from_str("notes", "", None);
    let mut state = AppState::new(std::path::PathBuf::from("."), editor, notes);
    // 3 queue increments from dispatch: 1 active + 2 in the runner-internal
    // queue (assuming max_parallel_turns = 1).
    state
        .agents
        .agents
        .push(lane_with_queue("claude-opus", AgentLaneKind::Claude, 3));
    state.agents.rebuild_agents_index();

    // Today CancelAll only kills the active turn → exactly one TurnFailed.
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
        // Regression: the original suffix table missed `#mp-pane-` so multipane
        // turns went out with the full id and Claude rejected them. The slug
        // stripper now splits on the FIRST `#`.
        ("claude-haiku-4-5#mp-pane-00", "claude-haiku-4-5"),
        ("claude-opus-4-7#mp-pane-12", "claude-opus-4-7"),
        // Nested multipane → swarm: pane prefix + swarm suffix both peel.
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

// Pin the Lens-E spawn-site invariant: prepare_claude_command must bind
// current_dir to the per-pane cwd. Without it, --add-dir updates only the
// allow-list and the subprocess inherits nit's parent cwd.
#[test]
fn prepare_claude_command_binds_cwd_to_subprocess_working_directory() {
    let config = ClaudeRunnerConfig::default();
    let cwd = Path::new("/tmp/pane0-cwd");
    let cmd = super::prepare_claude_command(
        "claude-opus-4-6",
        cwd,
        true,
        None,
        Path::new("/tmp/out.txt"),
        None,
        false,
        None,
        &config,
    );
    assert_eq!(cmd.get_current_dir(), Some(cwd));
}

// Resume turns must spawn with the SAME cwd as fresh turns. Pre-fix the
// resume branch dropped `-C`, leaving resumed sessions at the workspace root.
#[test]
fn prepare_claude_command_binds_cwd_for_resume_turns() {
    let config = ClaudeRunnerConfig::default();
    let cwd = Path::new("/tmp/pane3-after-dir-change");
    let cmd = super::prepare_claude_command(
        "claude-haiku-4-5",
        cwd,
        true,
        None,
        Path::new("/tmp/out.txt"),
        Some("session-abc-123"),
        false,
        None,
        &config,
    );
    assert_eq!(cmd.get_current_dir(), Some(cwd));
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

// --- append_stdout_line_capped --------------------------------------------
//
// Mirrors codex_runner cap tests; runs in spawn_turn_worker stdout reader.

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
    let line = line_with_terminator(b'x', 1023);
    let mut buf = fill_buffer_to_cap(&line);
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
    let line = line_with_terminator(b'y', 4095);
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

// --- idle-output reaper helpers -------------------------------------------

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

// `claude_turn_idle_timeout` reads a process-wide env var, so this test
// must serialize against any other test that touches the same var. The
// LOCK mutex pairs with Drop-restore at the bottom.
#[test]
fn claude_turn_idle_timeout_env_parsing() {
    use std::sync::Mutex;
    static LOCK: Mutex<()> = Mutex::new(());
    let _guard = LOCK.lock().unwrap();
    const VAR: &str = "NIT_CLAUDE_TURN_IDLE_TIMEOUT_SECS";

    let prior = std::env::var(VAR).ok();

    // Unset / empty → default-on at 15 min.
    std::env::remove_var(VAR);
    assert_eq!(
        claude_turn_idle_timeout(),
        Some(std::time::Duration::from_secs(15 * 60))
    );

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

    match prior {
        Some(value) => std::env::set_var(VAR, value),
        None => std::env::remove_var(VAR),
    }
}
