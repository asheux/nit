use super::*;

fn has_arg(args: &[String], expected: &str) -> bool {
    args.iter().any(|a| a == expected)
}

#[test]
fn test_claude_model_slug_for_agent_id() {
    let cases = [
        ("claude-opus-4-6", "claude-opus-4-6"),
        ("claude-opus-4-6#swarm-mis-001-clone-01", "claude-opus-4-6"),
        ("claude-sonnet-4-6#chat-clone-02", "claude-sonnet-4-6"),
        ("claude-opus-4-6#shadow-01-propose-a", "claude-opus-4-6"),
        ("claude-sonnet-4-6#shadow-07-judge", "claude-sonnet-4-6"),
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
