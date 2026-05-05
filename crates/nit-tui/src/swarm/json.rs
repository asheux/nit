pub(super) fn extract_json_code_block(text: &str) -> Option<String> {
    if let Some(first) = extract_json_code_blocks(text).into_iter().next() {
        return Some(first);
    }

    // Fallback: scan for the outer-most `{ ... }` span so we can recover JSON
    // even when the agent forgot the ```json fence.
    let trimmed = text.trim();
    let start = trimmed.find('{')?;
    let end = trimmed.rfind('}')?;
    if start >= end {
        return None;
    }
    let candidate = trimmed[start..=end].trim().to_string();
    (!candidate.is_empty()).then_some(candidate)
}

pub(super) fn extract_json_code_blocks(text: &str) -> Vec<String> {
    let mut blocks = Vec::new();
    let mut lines = text.lines();
    while let Some(line) = lines.next() {
        let trimmed = line.trim_start();
        let is_json_fence = trimmed.starts_with("```json") || trimmed.starts_with("```JSON");
        if !is_json_fence {
            continue;
        }
        let mut buf = String::new();
        for inner in &mut lines {
            if inner.trim() == "```" {
                break;
            }
            buf.push_str(inner);
            buf.push('\n');
        }
        let candidate = buf.trim().trim_end_matches('`').trim().to_string();
        if !candidate.is_empty() {
            blocks.push(candidate);
        }
    }
    blocks
}
