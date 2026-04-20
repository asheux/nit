use super::{parse_swarm_mission_kind, SwarmMissionKind, SwarmSize};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SwarmCommand {
    pub size: SwarmSize,
    pub template: Option<String>,
    pub mission_kind: Option<SwarmMissionKind>,
    pub prompt: String,
}

pub fn parse_swarm_command(raw: &str) -> Option<SwarmCommand> {
    let after = raw.trim_start().strip_prefix("@swarm")?;
    if after.is_empty() {
        return None;
    }
    if !after.starts_with(char::is_whitespace) {
        // Avoid treating "@swarmies" as a command.
        return None;
    }
    let mut rest = after.trim_start();
    if rest.is_empty() {
        return None;
    }

    let mut size = SwarmSize::Default;
    if let Some(next) = rest.split_whitespace().next() {
        if next.eq_ignore_ascii_case("all") {
            size = SwarmSize::All;
            rest = rest.strip_prefix(next).unwrap_or(rest).trim_start();
        } else if next.chars().all(|ch| ch.is_ascii_digit()) {
            if let Ok(n) = next.parse::<usize>() {
                size = SwarmSize::Count(n);
                rest = rest.strip_prefix(next).unwrap_or(rest).trim_start();
            }
        }
    }

    let mut template = None;
    let mut mission_kind = None;
    loop {
        let Some(next) = rest.split_whitespace().next() else {
            break;
        };
        if let Some(value) = next
            .strip_prefix("template=")
            .or_else(|| next.strip_prefix("t="))
        {
            let value = value.trim();
            if !value.is_empty() {
                template = Some(value.to_ascii_lowercase());
            }
            rest = rest.strip_prefix(next).unwrap_or(rest).trim_start();
            continue;
        }
        if let Some(value) = next
            .strip_prefix("mission=")
            .or_else(|| next.strip_prefix("m="))
        {
            mission_kind = parse_swarm_mission_kind(Some(value));
            rest = rest.strip_prefix(next).unwrap_or(rest).trim_start();
            continue;
        }
        break;
    }

    let prompt = rest.to_string();
    if prompt.trim().is_empty() {
        return None;
    }

    Some(SwarmCommand {
        size,
        template,
        mission_kind,
        prompt,
    })
}
