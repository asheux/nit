use super::{parse_swarm_mission_kind, SwarmMissionKind, SwarmSize};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SwarmCommand {
    pub size: SwarmSize,
    pub template: Option<String>,
    pub mission_kind: Option<SwarmMissionKind>,
    pub prompt: String,
}

pub fn parse_swarm_command(raw: &str) -> Option<SwarmCommand> {
    // Require whitespace after `@swarm` so we don't match `@swarmies`,
    // `@swarmlet`, etc. as the prefix.
    let after = raw.trim_start().strip_prefix("@swarm")?;
    if after.is_empty() || !after.starts_with(char::is_whitespace) {
        return None;
    }
    let mut rest = after.trim_start();
    if rest.is_empty() {
        return None;
    }

    let size = parse_size_token(&mut rest);
    let (template, mission_kind) = parse_named_args(&mut rest);

    let prompt = rest.trim().to_string();
    if prompt.is_empty() {
        return None;
    }
    Some(SwarmCommand {
        size,
        template,
        mission_kind,
        prompt,
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

fn parse_named_args(rest: &mut &str) -> (Option<String>, Option<SwarmMissionKind>) {
    let mut template = None;
    let mut mission_kind = None;
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
        } else {
            break;
        }
    }
    (template, mission_kind)
}

fn strip_kv_prefix<'a>(token: &'a str, prefixes: &[&str]) -> Option<&'a str> {
    prefixes
        .iter()
        .find_map(|prefix| token.strip_prefix(prefix))
}

fn consume_token(rest: &mut &str, token: &str) {
    *rest = rest.strip_prefix(token).unwrap_or(rest).trim_start();
}
