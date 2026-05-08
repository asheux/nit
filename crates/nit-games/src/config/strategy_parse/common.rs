use crate::game::Action;
use crate::strategy::InputMode;

const INPUT_MODE_ALIASES: &[(&[&str], InputMode)] = &[
    (
        &["opponentlastaction", "opponent", "opp", "opplastaction"],
        InputMode::OpponentLastAction,
    ),
    (
        &["selflastaction", "self", "selflast"],
        InputMode::SelfLastAction,
    ),
    (
        &[
            "jointlastaction",
            "joint",
            "jointlast",
            "combinedlastaction",
            "combined",
            "combinedlast",
        ],
        InputMode::JointLastAction,
    ),
];

pub(super) fn parse_actions(
    id: &str,
    field: &str,
    values: Vec<String>,
    errors: &mut Vec<String>,
) -> Vec<Action> {
    values
        .into_iter()
        .filter_map(|value| {
            Action::parse(&value).or_else(|| {
                errors.push(format!(
                    "strategy '{id}': invalid action '{value}' in {field}"
                ));
                None
            })
        })
        .collect()
}

pub(super) fn parse_input_mode(
    id: &str,
    raw: Option<&str>,
    errors: &mut Vec<String>,
) -> Option<InputMode> {
    let raw = raw?;
    let normalized: String = raw
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_lowercase())
        .collect();
    for (aliases, mode) in INPUT_MODE_ALIASES {
        if aliases.contains(&normalized.as_str()) {
            return Some(*mode);
        }
    }
    errors.push(format!(
        "strategy '{id}': invalid input_mode '{raw}' (expected opponent_last_action, self_last_action, or joint_last_action)"
    ));
    None
}
