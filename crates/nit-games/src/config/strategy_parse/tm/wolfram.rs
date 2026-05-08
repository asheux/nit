use crate::strategy::{decode_tm_rule_code_wolfram, TmTransition};

/// Decodes a Wolfram-style `rule_code` into the transition table for a TM
/// over `states` × `symbols`. Each transition occupies one base-`(states+1)`
/// digit for the `next` state, one base-`symbols` digit for the `write`
/// symbol, and a base-3 digit for the `move` direction; the encoder visits
/// `(state, read)` pairs in row-major order and accumulates the digits as
/// successive higher places of `rule_code`.
///
/// `decode_tm_rule_code_wolfram` returns any leftover bits in `remaining`.
/// A non-zero `remaining` for a non-empty machine means the rule code carried
/// information beyond the declared shape — almost always a config typo or a
/// machine size mismatch — and is reported as a strategy-level error.
pub(super) fn decode_tm_rule_code(
    id: &str,
    rule_code: u64,
    states: usize,
    symbols: usize,
    errors: &mut Vec<String>,
) -> Vec<TmTransition> {
    let (transitions, remaining) = decode_tm_rule_code_wolfram(rule_code, states, symbols);
    if states > 0 && symbols > 0 && remaining != 0 {
        errors.push(format!(
            "strategy '{id}': rule_code has unused higher digits for states={states} symbols={symbols}"
        ));
    }
    transitions
}
