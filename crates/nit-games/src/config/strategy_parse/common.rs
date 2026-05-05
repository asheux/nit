use crate::game::Action;
use crate::strategy::InputMode;

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
    match normalized.as_str() {
        "opponentlastaction" | "opponent" | "opp" | "opplastaction" => {
            Some(InputMode::OpponentLastAction)
        }
        "selflastaction" | "self" | "selflast" => Some(InputMode::SelfLastAction),
        "jointlastaction" | "joint" | "jointlast" | "combinedlastaction" | "combined"
        | "combinedlast" => Some(InputMode::JointLastAction),
        _ => {
            errors.push(format!(
                "strategy '{id}': invalid input_mode '{raw}' (expected opponent_last_action, self_last_action, or joint_last_action)"
            ));
            None
        }
    }
}
