use nit_core::AgentTokenCount;

pub(super) fn extract_thread_id_from_jsonl(stdout: &[u8]) -> Option<String> {
    // `codex exec --json` emits a "thread.started" event with a `thread_id` field.
    let text = String::from_utf8_lossy(stdout);
    for raw in text.lines() {
        let raw = raw.trim();
        if raw.is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<serde_json::Value>(raw) else {
            continue;
        };
        let Some(thread_id) = value.get("thread_id").and_then(|v| v.as_str()) else {
            continue;
        };
        if thread_id.trim().is_empty() {
            continue;
        }
        // Prefer the canonical thread lifecycle events, but accept any event containing thread_id.
        if let Some(kind) = value.get("type").and_then(|v| v.as_str()) {
            if kind.starts_with("thread.") {
                return Some(thread_id.to_string());
            }
        }
        return Some(thread_id.to_string());
    }
    None
}

pub(super) fn extract_token_count_from_jsonl(stdout: &[u8]) -> Option<AgentTokenCount> {
    // Codex streams "token_count" events that include total token usage + context window.
    // Accepts both exec-mode JSONL and session-style wrapped events.
    let text = String::from_utf8_lossy(stdout);
    let mut last: Option<AgentTokenCount> = None;
    for raw in text.lines() {
        let raw = raw.trim();
        if raw.is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<serde_json::Value>(raw) else {
            continue;
        };

        if let Some(token_count) = token_count_from_value(&value) {
            last = Some(token_count);
        }
    }
    last
}

pub(super) fn token_count_from_value(value: &serde_json::Value) -> Option<AgentTokenCount> {
    let payload = value.get("payload").unwrap_or(value);
    let kind = payload.get("type").and_then(|v| v.as_str())?;
    if kind == "token_count" {
        let info = payload.get("info")?;
        let context_window = extract_context_window(info)?;
        let total_tokens = extract_total_tokens(info)?;
        if context_window > u32::MAX as u64 || total_tokens > u32::MAX as u64 {
            return None;
        }
        return Some(AgentTokenCount {
            total_tokens: total_tokens as u32,
            context_window: context_window as u32,
        });
    }

    // Fallback: some Codex CLI versions only report per-turn token usage at
    // `turn.completed`. Those payloads often omit the context window size; the
    // UI can stitch that in from the models cache. context_window=0 means
    // "unknown".
    let usage = payload.get("usage")?;
    let input = usage
        .get("input_tokens")
        .or_else(|| usage.get("prompt_tokens"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let output = usage
        .get("output_tokens")
        .or_else(|| usage.get("completion_tokens"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let total = input.saturating_add(output);
    if total == 0 || total > u32::MAX as u64 {
        return None;
    }
    Some(AgentTokenCount {
        total_tokens: total as u32,
        context_window: 0,
    })
}

pub(super) fn extract_context_window(info: &serde_json::Value) -> Option<u64> {
    info.get("model_context_window")
        .or_else(|| info.get("context_window"))
        .or_else(|| info.get("context_window_tokens"))
        .or_else(|| info.get("model_context_window_tokens"))
        .and_then(|v| v.as_u64())
        .filter(|v| *v > 0)
}

pub(super) fn extract_total_tokens(info: &serde_json::Value) -> Option<u64> {
    // Prefer the *last* model-visible token usage over lifetime totals. Codex
    // can auto-compact context when nearing the model context window. When that
    // happens lifetime usage (total_token_usage) keeps increasing but the
    // model-visible history size can decrease; `last_token_usage` reflects the
    // post-compaction size.
    info.get("last_token_usage")
        .and_then(|u| u.get("total_tokens"))
        .and_then(|v| v.as_u64())
        .or_else(|| {
            info.get("total_token_usage")
                .and_then(|u| u.get("total_tokens"))
                .and_then(|v| v.as_u64())
        })
        .or_else(|| info.get("total_tokens").and_then(|v| v.as_u64()))
        .or_else(|| info.get("used_tokens").and_then(|v| v.as_u64()))
}
