use crate::state::{AgentLaneKind, AppState};

pub(super) fn backend_source_for_agent(state: &AppState, agent_id: &str) -> &'static str {
    state
        .agents
        .agents_get(agent_id)
        .map(|a| match a.kind {
            AgentLaneKind::Claude => "claude",
            AgentLaneKind::Gemini => "gemini",
            AgentLaneKind::Mock => "local",
            AgentLaneKind::Codex | AgentLaneKind::Unknown => "codex",
        })
        .unwrap_or("codex")
}

// Runner-internal cancels emitted by `codex_runner` when an operator
// reconfigures MCP transport (`McpStop`, `McpReconnect`). These are
// deliberate cancels, not errors — they ride the soft-cancel path.
// Server-exit / disconnect messages stay on the error path so MCP
// health issues remain visible.
pub(super) fn is_runner_internal_cancel(message: &str) -> bool {
    matches!(
        message,
        "Cancelled (MCP stop)" | "Cancelled (MCP reconnect)"
    )
}

pub(super) fn timestamp_label(state: &AppState) -> String {
    format!("t+{}", state.metrics.frame_count)
}

pub(super) fn estimate_codex_context_tokens(text: &str) -> u32 {
    if text.is_empty() {
        return 0;
    }
    let bytes = text.len() as u32;
    bytes.div_ceil(4)
}

// Best-effort one-line summary for `TurnFailed.message`. The chat status banner
// has room for one line; runners often hand us full JSON or multi-line traces.
// Try a structured `error.message` (or top-level `message`) first, then fall
// back to the first non-empty line of the raw text.
pub(super) fn summarize_agent_error(message: &str) -> String {
    let trimmed = message.trim();
    if trimmed.is_empty() {
        return "unknown error".into();
    }
    if let Some(msg) = parse_error_json(trimmed)
        .as_ref()
        .and_then(extract_error_message)
    {
        let msg = msg.trim();
        if !msg.is_empty() {
            return msg.to_string();
        }
    }
    trimmed.lines().next().unwrap_or(trimmed).trim().to_string()
}

fn parse_error_json(text: &str) -> Option<serde_json::Value> {
    if (text.starts_with('{') && text.ends_with('}'))
        || (text.starts_with('[') && text.ends_with(']'))
    {
        return serde_json::from_str(text).ok();
    }
    let start = text.find('{')?;
    let end = text.rfind('}')?;
    if start >= end {
        return None;
    }
    serde_json::from_str(&text[start..=end]).ok()
}

fn extract_error_message(value: &serde_json::Value) -> Option<&str> {
    value
        .get("error")
        .and_then(|err| err.get("message"))
        .and_then(|v| v.as_str())
        .or_else(|| value.get("message").and_then(|v| v.as_str()))
}
