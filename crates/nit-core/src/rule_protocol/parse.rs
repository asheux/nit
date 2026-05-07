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

/// Loop-suffix forms accepted by `parse_protocol_spec`, in priority order.
/// First match wins; the bare `"loop"` form is guarded against `/sloop`
/// because that's a Generations-rule rulestring (e.g. `B2/S/sloop`)
/// whose terminal `loop` is not the looping marker.
const LOOP_SUFFIXES: &[(&str, usize)] = &[("(loop)", 6), (" loop", 5), ("loop", 4)];

fn strip_loop_suffix(spec: &str) -> (String, bool) {
    let lowered = spec.to_ascii_lowercase();
    for (suffix, len) in LOOP_SUFFIXES {
        if !lowered.ends_with(suffix) {
            continue;
        }
        if *suffix == "loop" && lowered.ends_with("/sloop") {
            continue;
        }
        let mut cleaned = spec.to_string();
        cleaned.truncate(cleaned.len() - len);
        return (cleaned, true);
    }
    (spec.to_string(), false)
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
