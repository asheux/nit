use crate::strategy::{TmMove, TmTransition};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub(super) struct TmTransitionRule {
    pub(super) state: usize,
    pub(super) read: usize,
    pub(super) write: usize,
    #[serde(rename = "move")]
    pub(super) move_dir: TmMove,
    pub(super) next: usize,
}

pub(super) fn apply_tm_transition_rules_from_value(
    id: &str,
    raw: toml::Value,
    states: usize,
    symbols: usize,
    blank: usize,
    errors: &mut Vec<String>,
) -> Vec<TmTransition> {
    match raw.try_into::<Vec<TmTransitionRule>>() {
        Ok(rules) => apply_tm_transition_rules(id, &rules, states, symbols, blank, errors),
        Err(err) => {
            errors.push(format!("strategy '{id}': invalid tm transitions: {err}"));
            Vec::new()
        }
    }
}

fn apply_tm_transition_rules(
    id: &str,
    rules: &[TmTransitionRule],
    states: usize,
    symbols: usize,
    blank: usize,
    errors: &mut Vec<String>,
) -> Vec<TmTransition> {
    let total = states.saturating_mul(symbols);
    let mut transitions = vec![
        TmTransition {
            write: blank as u8,
            move_dir: TmMove::Stay,
            next: 0,
        };
        total
    ];
    let mut seen = vec![false; total];
    for rule in rules {
        let Some(idx) = validate_tm_rule_bounds(id, rule, states, symbols, errors) else {
            continue;
        };
        match seen.get_mut(idx) {
            Some(slot) if *slot => {
                errors.push(format!(
                    "strategy '{id}': duplicate tm transition for state {} read {}",
                    rule.state, rule.read
                ));
                continue;
            }
            Some(slot) => *slot = true,
            None => continue,
        }
        if let Some(entry) = transitions.get_mut(idx) {
            *entry = TmTransition {
                write: rule.write as u8,
                move_dir: rule.move_dir,
                next: rule.next as u16,
            };
        }
    }
    let missing = seen.iter().filter(|&&v| !v).count();
    if missing > 0 {
        errors.push(format!(
            "strategy '{id}': tm transitions missing {missing} (state, read) pairs"
        ));
    }
    transitions
}

fn validate_tm_rule_bounds(
    id: &str,
    rule: &TmTransitionRule,
    states: usize,
    symbols: usize,
    errors: &mut Vec<String>,
) -> Option<usize> {
    let TmTransitionRule {
        state,
        read,
        write,
        next,
        ..
    } = *rule;
    if state == 0 || state > states {
        errors.push(format!(
            "strategy '{id}': tm transition state {state} out of range (1..={states})"
        ));
        return None;
    }
    if read >= symbols {
        errors.push(format!(
            "strategy '{id}': tm transition read {read} out of range (symbols={symbols})"
        ));
        return None;
    }
    if write >= symbols {
        errors.push(format!(
            "strategy '{id}': tm transition write {write} out of range (symbols={symbols})"
        ));
        return None;
    }
    if next > states {
        errors.push(format!(
            "strategy '{id}': tm transition next {next} out of range (0..={states})"
        ));
        return None;
    }
    Some((state - 1) * symbols + read)
}
