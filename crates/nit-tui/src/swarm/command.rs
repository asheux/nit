use std::collections::HashMap;

use super::budgets::parse_override_token;
use super::{parse_swarm_mission_kind, SwarmMissionKind, SwarmSize};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SwarmCommand {
    pub size: SwarmSize,
    pub template: Option<String>,
    pub mission_kind: Option<SwarmMissionKind>,
    pub prompt: String,
    /// Per-mission `budget=ROLE:N` overrides parsed from the `@swarm`
    /// command line. Keyed by canonical role label; values are byte
    /// ceilings forwarded to `SwarmRun.prompt_budgets`. Unrecognised tokens
    /// are dropped silently so an operator typo can't kill the dispatch.
    pub prompt_budgets: HashMap<String, usize>,
}

pub fn parse_swarm_command(raw: &str) -> Option<SwarmCommand> {
    let after = raw.trim_start().strip_prefix("@swarm")?;
    if after.is_empty() || !after.starts_with(char::is_whitespace) {
        return None;
    }
    let mut rest = after.trim_start();
    if rest.is_empty() {
        return None;
    }

    let size = parse_size_token(&mut rest);
    let (template, mission_kind, prompt_budgets) = parse_named_args(&mut rest);

    let prompt = rest.trim().to_string();
    if prompt.is_empty() {
        return None;
    }
    Some(SwarmCommand {
        size,
        template,
        mission_kind,
        prompt,
        prompt_budgets,
    })
}

fn parse_size_token(rest: &mut &str) -> SwarmSize {
    let Some(next) = rest.split_whitespace().next() else {
        return SwarmSize::Default;
    };
    if next.eq_ignore_ascii_case("all") {
        consume_token(rest, next);
        return SwarmSize::All;
    }
    if next.chars().all(|ch| ch.is_ascii_digit()) {
        if let Ok(n) = next.parse::<usize>() {
            consume_token(rest, next);
            return SwarmSize::Count(n);
        }
    }
    SwarmSize::Default
}

fn parse_named_args(
    rest: &mut &str,
) -> (
    Option<String>,
    Option<SwarmMissionKind>,
    HashMap<String, usize>,
) {
    let mut template = None;
    let mut mission_kind = None;
    let mut prompt_budgets: HashMap<String, usize> = HashMap::new();
    while let Some(next) = rest.split_whitespace().next() {
        if let Some(value) = strip_kv_prefix(next, &["template=", "t="]) {
            let value = value.trim();
            if !value.is_empty() {
                template = Some(value.to_ascii_lowercase());
            }
            consume_token(rest, next);
        } else if let Some(value) = strip_kv_prefix(next, &["mission=", "m="]) {
            mission_kind = parse_swarm_mission_kind(Some(value));
            consume_token(rest, next);
        } else if let Some(value) = strip_kv_prefix(next, &["budget="]) {
            if let Ok((role, bytes)) = parse_override_token(value) {
                prompt_budgets.insert(role, bytes);
            }
            consume_token(rest, next);
        } else {
            break;
        }
    }
    (template, mission_kind, prompt_budgets)
}

fn strip_kv_prefix<'a>(token: &'a str, prefixes: &[&str]) -> Option<&'a str> {
    prefixes
        .iter()
        .find_map(|prefix| token.strip_prefix(prefix))
}

fn consume_token(rest: &mut &str, token: &str) {
    *rest = rest.strip_prefix(token).unwrap_or(rest).trim_start();
}
