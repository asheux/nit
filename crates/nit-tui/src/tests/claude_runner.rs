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

#[test]
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

    let pending = |lane: &str, mission: &str| super::ClaudeCommand::RunTurn {
        model: lane.into(),
        cwd: ".".into(),
        mission_id: Some(mission.into()),
        resume_session_id: None,
        persist_session: false,
        effort: None,
        prompt: "queued".into(),
        read_only: true,
        max_turns: None,
    };
    let mut queue = std::collections::VecDeque::from([
        pending("claude-opus", "first"),
        pending("other", "unrelated"),
        pending("claude-opus", "second"),
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
            matches!(&event, AgentBusEvent::TurnFailed { agent_id, mission_id: Some(mission), message, .. } if agent_id == "claude-opus" && mission == expected && message == OPERATOR_CANCEL_TURN_MESSAGE)
        );
        event.apply(&mut state);
    }
    assert!(rx.try_recv().is_err());
    assert_eq!(state.agents.agents_get("claude-opus").unwrap().queue_len, 1);
    // The active worker reports its own cancellation when it exits.
    AgentBusEvent::TurnFailed {
        agent_id: "claude-opus".into(),
        mission_id: None,
        thread_id: None,
        token_count: None,
        message: OPERATOR_CANCEL_TURN_MESSAGE.into(),
    }
    .apply(&mut state);

    let agent = state.agents.agents_get("claude-opus").unwrap();
    assert_eq!(agent.queue_len, 0);
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
        None,
        false,
        None,
        &config,
    );
    assert!(!has_arg(&args, "--strict-mcp-config"));
    assert!(!has_arg(&args, "--tools"));
    assert!(args.windows(2).any(|args| args
        == [
            "--allowedTools",
            "Read,Edit,Write,Bash,Glob,Grep,WebSearch,WebFetch",
        ]));
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
        None,
        true,
        None,
        &config,
    );
    assert!(args.windows(5).any(|args| args
        == [
            "--tools",
            "Read,Glob,Grep",
            "--allowedTools",
            "Read,Glob,Grep",
            "--strict-mcp-config",
        ]));
    if let Some(dir) = nit_core::skills::reference_dir() {
        assert!(args
            .windows(2)
            .any(|args| args == ["--add-dir", &dir.display().to_string()]));
    }
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

#[test]
fn turn_supports_pool_only_when_all_vanilla_inputs() {
    assert!(super::turn_supports_pool(&None, true, &None, None));
    // Any non-default knob disqualifies the pool path.
    assert!(!super::turn_supports_pool(
        &Some("session-abc".into()),
        true,
        &None,
        None
    ));
    assert!(!super::turn_supports_pool(&None, false, &None, None));
    assert!(!super::turn_supports_pool(
        &None,
        true,
        &Some("high".into()),
        None
    ));
    assert!(!super::turn_supports_pool(&None, true, &None, Some(500)));
}

#[test]
fn cold_spawn_args_unchanged_by_pool_addition() {
    // Pin propose-judge requirement: `NIT_CLAUDE_POOL=0` keeps the cold-spawn
    // argv list byte-identical. The pool-specific flag `--input-format
    // stream-json` MUST NOT leak into the cold path.
    let config = ClaudeRunnerConfig::default();
    let args = build_claude_args(
        "claude-opus-4-7",
        Path::new("/tmp/project"),
        true,
        None,
        None,
        false,
        None,
        &config,
    );
    assert!(!has_arg(&args, "--input-format"));
    assert!(has_arg(&args, "stream-json"));
    assert_eq!(
        args.iter().filter(|a| a.as_str() == "stream-json").count(),
        1,
        "cold path only carries `--output-format stream-json`"
    );
}

#[test]
fn pool_worker_args_carry_stream_json_input_format() {
    let config = ClaudeRunnerConfig::default();
    let key = crate::claude_pool::WorkerKey::new(
        "claude-opus-4-7",
        std::path::PathBuf::from("/tmp/work"),
        false,
    );
    let args = super::build_pool_worker_args(&key, &config);
    assert!(!has_arg(&args, "--strict-mcp-config"));
    assert!(!has_arg(&args, "--tools"));
    assert!(args.windows(2).any(|args| args
        == [
            "--allowedTools",
            "Read,Edit,Write,Bash,Glob,Grep,WebSearch,WebFetch",
        ]));
    assert!(has_arg(&args, "--input-format"));
    assert_eq!(
        args.iter().filter(|a| a.as_str() == "stream-json").count(),
        2,
        "pool worker carries both --input-format and --output-format stream-json"
    );
    assert!(has_arg(&args, "--no-session-persistence"));
}

#[test]
fn pool_worker_args_honour_read_only_tool_allowlist() {
    let config = ClaudeRunnerConfig::default();
    let key = crate::claude_pool::WorkerKey::new(
        "claude-opus-4-7",
        std::path::PathBuf::from("/tmp/work"),
        true,
    );
    let args = super::build_pool_worker_args(&key, &config);
    assert!(args.windows(5).any(|args| args
        == [
            "--tools",
            "Read,Glob,Grep",
            "--allowedTools",
            "Read,Glob,Grep",
            "--strict-mcp-config",
        ]));
    if let Some(dir) = nit_core::skills::reference_dir() {
        assert!(args
            .windows(2)
            .any(|args| args == ["--add-dir", &dir.display().to_string()]));
    }
    assert!(!args
        .iter()
        .any(|a| a.contains("Write") || a.contains("Edit") || a.contains("Bash")));
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

#[test]
fn provider_error_result_never_completes_the_turn() {
    let (tx, rx) = std::sync::mpsc::channel();
    let stream = br#"{"type":"assistant","message":"Starting"}
{"type":"result","subtype":"success","is_error":true,"session_id":"abc","result":"Subscription access disabled"}"#;
    assert!(report_terminal_error(
        stream,
        &tx,
        "claude",
        &None,
        None,
        Instant::now()
    ));
    assert!(matches!(
        rx.recv().unwrap(),
        AgentBusEvent::McpStatus { .. }
    ));
    assert!(
        matches!(rx.recv().unwrap(), AgentBusEvent::TurnFailed { message, .. } if message == "Subscription access disabled")
    );
    assert_eq!(
        terminal_result_error(
            br#"{"type":"result","subtype":"error_during_execution","errors":["bad argument"]}"#
        )
        .as_deref(),
        Some("bad argument")
    );
    assert!(terminal_result_error(
        br#"{"type":"result","subtype":"success","is_error":false,"result":"ok"}"#
    )
    .is_none());
}

#[cfg(unix)]
#[test]
fn cold_result_and_same_lane_successor_finish_while_descendants_hold_streams() {
    let dir = tempfile::tempdir().unwrap();
    let (tx, rx) = mpsc::channel();
    let worker = tx.clone();
    let work = dir.path().to_owned();
    let join = thread::spawn(move || {
        for seq in [1, 2] {
            let mut cmd = Command::new("/bin/sh");
            cmd.arg("-c").arg(r#"sleep 5 & printf '%s\n' '{"type":"result","subtype":"success","result":"answer","session_id":"session-fixture","usage":{"input_tokens":9,"output_tokens":2}}'"#);
            run_cold_process(
                cmd,
                &worker,
                seq,
                "model".into(),
                work.clone(),
                None,
                None,
                "hello".into(),
                Arc::new(AtomicBool::new(false)),
            );
        }
    });
    drop(tx);
    let end = Instant::now() + Duration::from_secs(3);
    let mut replies = 0;
    while replies < 2 {
        let event = rx
            .recv_timeout(end.saturating_duration_since(Instant::now()))
            .expect("successor waited on inherited output streams");
        match event {
            AgentBusEvent::TurnCompleted {
                message,
                thread_id,
                token_count,
                ..
            } => {
                assert_eq!(message, "answer");
                assert_eq!(thread_id.as_deref(), Some("session-fixture"));
                assert!(token_count.is_some());
                replies += 1;
            }
            AgentBusEvent::TurnFailed { message, .. } => panic!("{message}"),
            _ => {}
        }
    }
    join.join().unwrap();
    assert!(!rx.try_iter().any(|e| matches!(
        e,
        AgentBusEvent::TurnCompleted { .. } | AgentBusEvent::TurnFailed { .. }
    )));
}

#[cfg(unix)]
#[test]
fn cold_error_malformed_and_cancel_emit_one_failure_while_streams_are_inherited() {
    let dir = tempfile::tempdir().unwrap();
    for (result, cancelled, expected) in [
        (
            r#"{"type":"result","subtype":"error_during_execution","errors":["provider error"]}"#,
            false,
            "provider error",
        ),
        (r#"{"type":"result"}"#, false, "incomplete result"),
        (
            r#"{"type":"result","result":"late answer"}"#,
            true,
            nit_core::OPERATOR_CANCEL_TURN_MESSAGE,
        ),
    ] {
        let (tx, rx) = mpsc::channel();
        let mut cmd = Command::new("/bin/sh");
        cmd.arg("-c")
            .arg("sleep 5 & printf '%s\\n' \"$1\"")
            .arg("fixture")
            .arg(result);
        let before = Instant::now();
        run_cold_process(
            cmd,
            &tx,
            17,
            "model".into(),
            dir.path().into(),
            None,
            None,
            "hi".into(),
            Arc::new(AtomicBool::new(cancelled)),
        );
        assert!(before.elapsed() < Duration::from_secs(3));
        let results: Vec<_> = rx
            .try_iter()
            .filter_map(|e| match e {
                AgentBusEvent::TurnCompleted { .. } => panic!("unexpected success for {result}"),
                AgentBusEvent::TurnFailed { message, .. } => Some(message),
                _ => None,
            })
            .collect();
        assert_eq!(results.len(), 1);
        assert!(results[0].contains(expected), "{results:?}");
    }
}

#[cfg(unix)]
#[test]
fn live_followup_keeps_both_uuid_correlated_results_in_their_original_session() {
    use crate::steering::SteerOutcome;
    use std::sync::atomic::AtomicUsize;
    struct Scripted {
        spawns: Arc<AtomicUsize>,
    }
    impl WorkerSpawn for Scripted {
        fn spawn(&self, key: &WorkerKey, _: &str) -> std::io::Result<SpawnedChild> {
            self.spawns.fetch_add(1, Ordering::Relaxed);
            let args = super::build_pool_worker_args(key, &ClaudeRunnerConfig::default());
            assert!(args.windows(2).any(|a| a == ["--effort", "high"]));
            assert!(args.windows(2).any(|a| a == ["--tools", "Read,Glob,Grep"]));
            assert!(args.iter().any(|a| a == "--replay-user-messages"));
            assert!(!args.iter().any(|a| a == "--no-session-persistence"));
            let code = r#"
import json, sys
def get(): return json.loads(sys.stdin.readline())
def emit(kind, **fields):
    print(json.dumps(dict(type=kind, session_id=session, **fields)), flush=True)
a=get(); session=a['session_id']; aid=a['uuid']
emit('system',subtype='init',user_message_uuids=[aid])
emit('user',uuid=aid,isReplay=True)
b=get(); bid=b['uuid']; assert b['session_id']==session
assert b['message']['content']=='please check the limit'
emit('result',subtype='success',result='original answer',user_message_uuid=aid,is_error=False)
emit('user',uuid=bid,isReplay=True)
emit('result',subtype='success',result='follow-up answer',user_message_uuid=bid,is_error=False)
c=get()
assert c['session_id']==session and c['message']['content']=='resume here'
emit('result',subtype='success',result='resumed answer',user_message_uuid=c['uuid'],is_error=False)
sys.stdin.read()
"#;
            let mut child = Command::new("python3")
                .args(["-u", "-c", code])
                .current_dir(&key.cwd)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()?;
            Ok(SpawnedChild {
                stdin: child.stdin.take().unwrap(),
                stdout: child.stdout.take().unwrap(),
                stderr: child.stderr.take().unwrap(),
                child,
            })
        }
    }
    let spawns = Arc::new(AtomicUsize::new(0));
    let pool = ClaudePool::new(
        1,
        Box::new(Scripted {
            spawns: spawns.clone(),
        }),
    );
    let mut runner = ClaudeRunner::spawn_with_pool(ClaudeRunnerConfig::default(), Some(pool));
    let cwd = tempfile::tempdir().unwrap();
    let send = |prompt: &str, session| ClaudeCommand::RunTurn {
        model: "sonnet".into(),
        cwd: cwd.path().into(),
        mission_id: None,
        resume_session_id: session,
        persist_session: true,
        effort: Some("high".into()),
        prompt: prompt.into(),
        read_only: true,
        max_turns: None,
    };
    runner.send(send("first question", None));
    let deadline = Instant::now() + Duration::from_secs(8);
    let target = loop {
        if let Some(t) = runner.live_turn("sonnet", None, cwd.path()) {
            break t;
        }
        assert!(
            Instant::now() < deadline,
            "worker failed to advertise a correlated stream"
        );
        thread::sleep(Duration::from_millis(10));
    };
    assert!(target.matches_chat_prompt("first question"));
    assert!(runner
        .live_turn("sonnet", Some("different-mission"), cwd.path())
        .is_none());
    assert!(runner
        .live_turn("sonnet", None, &cwd.path().join("other"))
        .is_none());
    let outcome = runner.steer(
        target.clone(),
        live::uuid().unwrap(),
        "please check the limit".into(),
    );
    let get_answer = || loop {
        if let AgentBusEvent::TurnCompleted {
            message, thread_id, ..
        } = runner.events.recv_timeout(Duration::from_secs(5)).unwrap()
        {
            break (message, thread_id.unwrap());
        }
    };
    let (answer, sid) = get_answer();
    assert_eq!(answer, "original answer");
    runner.send(send("resume here", Some(sid.clone())));
    assert_eq!(
        outcome.recv_timeout(Duration::from_secs(5)).unwrap(),
        SteerOutcome::Received
    );
    assert_eq!(
        outcome.recv_timeout(Duration::from_secs(5)).unwrap(),
        SteerOutcome::Completed {
            message: "follow-up answer".into(),
            failed: false
        }
    );
    assert_eq!(get_answer(), ("resumed answer".into(), sid));
    assert_eq!(
        runner
            .steer(target, live::uuid().unwrap(), "stale".into())
            .recv()
            .unwrap(),
        SteerOutcome::Stale
    );
    assert_eq!(spawns.load(Ordering::Relaxed), 1);
    runner.shutdown();
}
