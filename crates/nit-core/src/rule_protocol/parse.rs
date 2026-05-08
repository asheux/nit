//! Wire-format parser for protocol specs.
//!
//! Spec grammar: `<rule>[*<steps>](>...)*(loop)?`. Examples:
//! `conway*100`, `B36/S23*16>conway*256(loop)`, `vote*1>conway*31`.

use crate::gol_rules::RuleCatalog;

use super::types::{RulePhase, RuleProtocol, RuleRef};

pub fn parse_protocol_spec(spec: &str, catalog: &RuleCatalog) -> Result<RuleProtocol, String> {
    let trimmed = spec.trim();
    if trimmed.is_empty() {
        return Err("protocol spec is empty".into());
    }
    let (cleaned, looped) = strip_loop_suffix(trimmed);
    let phases = cleaned
        .split('>')
        .enumerate()
        .map(|(idx, raw)| parse_phase(idx, raw, catalog))
        .collect::<Result<Vec<_>, _>>()?;
    RuleProtocol::new(phases, looped)
}

fn parse_phase(idx: usize, raw: &str, catalog: &RuleCatalog) -> Result<RulePhase, String> {
    let part = raw.trim();
    if part.is_empty() {
        return Err(format!("phase {} is empty", idx + 1));
    }
    let (rule_text, steps) = split_steps(part, idx)?;
    if steps == 0 {
        return Err(format!("phase {} has steps=0", idx + 1));
    }
    let selected = catalog
        .select(rule_text)
        .map_err(|err| format!("invalid rule '{}' in phase {}: {}", rule_text, idx + 1, err))?;
    Ok(RulePhase {
        rule: RuleRef::from_selected(&selected),
        steps,
        label: None,
    })
}

fn split_steps(part: &str, idx: usize) -> Result<(&str, u32), String> {
    let Some((rule, after)) = part.split_once('*') else {
        return Ok((part, 1));
    };
    let steps_text = after.trim();
    let steps = steps_text
        .parse::<u32>()
        .map_err(|_| format!("invalid steps '{}' in phase {}", steps_text, idx + 1))?;
    Ok((rule.trim(), steps))
}

/// Loop-suffix forms `parse_protocol_spec` accepts, longest-first. The bare
/// `"loop"` form is guarded against `/sloop` because that's a Generations
/// rulestring (e.g. `B2/S/sloop`) whose trailing `loop` is part of the rule,
/// not the loop marker.
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
