use crate::gol_rules::RuleCatalog;

use super::types::{RulePhase, RuleProtocol, RuleRef};

pub fn parse_protocol_spec(spec: &str, catalog: &RuleCatalog) -> Result<RuleProtocol, String> {
    let trimmed = spec.trim();
    if trimmed.is_empty() {
        return Err("protocol spec is empty".into());
    }
    let (cleaned, looped) = strip_loop_suffix(trimmed);
    let mut phases = Vec::new();
    for (idx, raw) in cleaned.split('>').enumerate() {
        let part = raw.trim();
        if part.is_empty() {
            return Err(format!("phase {} is empty", idx + 1));
        }
        let (rule_text, steps) = split_phase(part, idx)?;
        if steps == 0 {
            return Err(format!("phase {} has steps=0", idx + 1));
        }
        let selected = catalog
            .select(rule_text)
            .map_err(|err| format!("invalid rule '{}' in phase {}: {}", rule_text, idx + 1, err))?;
        phases.push(RulePhase {
            rule: RuleRef::from_selected(&selected),
            steps,
            label: None,
        });
    }
    RuleProtocol::new(phases, looped)
}

fn strip_loop_suffix(spec: &str) -> (String, bool) {
    let lowered = spec.to_ascii_lowercase();
    let mut cleaned = spec.to_string();
    let looped = if lowered.ends_with("(loop)") {
        cleaned.truncate(cleaned.len() - 6);
        true
    } else if lowered.ends_with(" loop") {
        cleaned.truncate(cleaned.len() - 5);
        true
    } else if lowered.ends_with("loop") && !lowered.ends_with("/sloop") {
        cleaned.truncate(cleaned.len() - 4);
        true
    } else {
        false
    };
    (cleaned, looped)
}

fn split_phase(part: &str, idx: usize) -> Result<(&str, u32), String> {
    let Some((left, right)) = part.split_once('*') else {
        return Ok((part, 1));
    };
    let steps_text = right.trim();
    let steps = steps_text
        .parse::<u32>()
        .map_err(|_| format!("invalid steps '{}' in phase {}", steps_text, idx + 1))?;
    Ok((left.trim(), steps))
}
