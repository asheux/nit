//! Best-effort one-line error summaries for agent turn-failure messages.
//!
//! Runners often hand us full JSON payloads or multi-line traces; the chat
//! status banner only has space for one line. We try to extract a structured
//! `error.message` field first, then fall back to the first non-empty line.

pub(crate) fn summarize_agent_error(message: &str) -> String {
    let trimmed = message.trim();
    if trimmed.is_empty() {
        return "unknown error".into();
    }

    if let Some(value) = parse_error_json(trimmed) {
        if let Some(msg) = extract_error_message(&value) {
            let msg = msg.trim();
            if !msg.is_empty() {
                return msg.to_string();
            }
        }
    }

    trimmed.lines().next().unwrap_or(trimmed).trim().to_string()
}

fn parse_error_json(text: &str) -> Option<serde_json::Value> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }
    if (trimmed.starts_with('{') && trimmed.ends_with('}'))
        || (trimmed.starts_with('[') && trimmed.ends_with(']'))
    {
        return serde_json::from_str(trimmed).ok();
    }

    let start = trimmed.find('{')?;
    let end = trimmed.rfind('}')?;
    if start >= end {
        return None;
    }
    serde_json::from_str(&trimmed[start..=end]).ok()
}

fn extract_error_message(value: &serde_json::Value) -> Option<&str> {
    value
        .get("error")
        .and_then(|err| err.get("message"))
        .and_then(|v| v.as_str())
        .or_else(|| value.get("message").and_then(|v| v.as_str()))
}
