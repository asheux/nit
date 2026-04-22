use super::probe::PREFERRED_DEFAULT_EFFORT;

pub(crate) fn parse_effort_choices_from_help(help_output: &str) -> Option<Vec<String>> {
    let needle = "--effort";
    let start = help_output.find(needle)?;
    let after = &help_output[start + needle.len()..];
    let open = after.find('(')?;
    let close = after[open + 1..].find(')')?;
    let raw = &after[open + 1..open + 1 + close];

    let mut choices: Vec<String> = raw
        .split(',')
        .map(|piece| piece.trim().to_ascii_lowercase())
        .filter(|piece| !piece.is_empty() && piece.chars().all(|c| c.is_ascii_alphanumeric()))
        .collect();

    choices.sort_by(|a, b| {
        claude_effort_rank(a)
            .cmp(&claude_effort_rank(b))
            .then_with(|| a.cmp(b))
    });
    choices.dedup();

    (!choices.is_empty()).then_some(choices)
}

fn claude_effort_rank(effort: &str) -> u8 {
    match effort.to_ascii_lowercase().as_str() {
        "low" => 0,
        "medium" => 1,
        "high" => 2,
        "xhigh" => 3,
        "max" => 4,
        _ => 10,
    }
}

pub(super) fn pick_claude_default_effort(supported: &[String]) -> String {
    let find = |target: &str| {
        supported
            .iter()
            .find(|effort| effort.eq_ignore_ascii_case(target))
            .cloned()
    };

    find(PREFERRED_DEFAULT_EFFORT)
        .or_else(|| find("medium"))
        .or_else(|| find("low"))
        .or_else(|| supported.first().cloned())
        .unwrap_or_else(|| PREFERRED_DEFAULT_EFFORT.to_string())
}
