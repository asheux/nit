use std::path::Path;

use serde_json::json;

use super::build_codex_exec_args;
use super::build_codex_mcp_tool_call;
use super::codex_model_slug_for_agent_id;
use super::extract_thread_id_from_jsonl;
use super::extract_token_count_from_jsonl;
use super::handle_codex_mcp_notification;
use super::CodexRunnerConfig;
use nit_core::AgentBusEvent;
use nit_core::AgentTokenCount;
use std::sync::mpsc;
use std::time::Instant;

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
fn mcp_new_session_uses_base_model_slug_for_swarm_clone() {
    let config = CodexRunnerConfig {
        sandbox: Some("workspace-write".into()),
        approval_policy: Some("never".into()),
        max_parallel_turns: 2,
    };

    let (tool_name, arguments) = build_codex_mcp_tool_call(
        "gpt-5.2#swarm-mis-001-clone-01",
        "solve it",
        Path::new("/tmp/work"),
        Some("high"),
        &config,
        None,
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
    let config = CodexRunnerConfig {
        sandbox: Some("workspace-write".into()),
        approval_policy: Some("never".into()),
        max_parallel_turns: 2,
    };

    let args = build_codex_exec_args(
        "gpt-5.2#swarm-mis-001-clone-01",
        Path::new("/tmp/work"),
        false,
        Some("high"),
        Path::new("/tmp/out.txt"),
        None,
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
