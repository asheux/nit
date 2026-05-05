//! Cellular automaton rule decoding and shrinking-CA evaluation.

use crate::strategy::math::{checked_pow_usize, integer_digits_unsigned};

/// Minimum symbol count for CA rule tables (binary minimum).
pub(super) const MIN_SYMBOLS: u8 = 2;

#[derive(Clone, Debug)]
pub struct CaRunResult {
    pub rows: Vec<Vec<u8>>,
    pub output_symbol: u8,
    pub steps_executed: u32,
    pub stopped_early: bool,
}

/// Decode a rule code into a lookup table for the given symbol count
/// and neighborhood diameter (`two_r = 2 * radius`).
pub fn decode_ca_rule_table(rule_code: u64, symbols: u8, two_r: u32) -> Vec<u8> {
    let neighborhood = two_r.saturating_add(1) as usize;
    let table_len = checked_pow_usize(symbols.max(MIN_SYMBOLS) as usize, neighborhood).unwrap_or(0);
    integer_digits_unsigned(
        rule_code as u128,
        symbols.max(MIN_SYMBOLS) as usize,
        table_len,
    )
    .into_iter()
    .map(|digit| digit as u8)
    .collect()
}

/// Run a shrinking cellular automaton: each step reduces the row width by `2r`,
/// producing a pyramid of rows until convergence or step limit.
pub fn run_shrinking_ca(
    rule_table: &[u8],
    symbols: u8,
    two_r: u32,
    step_limit: u32,
    initial_row: &[u8],
) -> CaRunResult {
    let mut current_row = if initial_row.is_empty() {
        vec![0]
    } else {
        initial_row.to_vec()
    };
    let mut collected_rows = vec![current_row.clone()];
    let mut steps_executed = 0u32;
    let mut stopped_early = false;
    let diameter = two_r as usize;
    let neighborhood_width = diameter.saturating_add(1);

    for _ in 0..step_limit {
        let shrunk_len = current_row.len().saturating_sub(diameter);
        if neighborhood_width == 0 || shrunk_len == 0 {
            stopped_early = true;
            break;
        }
        let mut next_row = Vec::with_capacity(shrunk_len);
        for window_start in 0..shrunk_len {
            let window_end = window_start.saturating_add(neighborhood_width);
            let cell =
                ca_transition_symbol(rule_table, symbols, &current_row[window_start..window_end]);
            next_row.push(cell);
        }
        current_row = next_row;
        collected_rows.push(current_row.clone());
        steps_executed = steps_executed.saturating_add(1);
    }

    let output_symbol = current_row.last().copied().unwrap_or(0);
    CaRunResult {
        rows: collected_rows,
        output_symbol,
        steps_executed,
        stopped_early,
    }
}

/// Look up the next cell value from the rule table for a neighborhood window.
/// Interprets `window` as a mixed-radix index into the table.
fn ca_transition_symbol(rule_table: &[u8], symbols: u8, window: &[u8]) -> u8 {
    let radix = symbols.max(MIN_SYMBOLS) as usize;
    let table_index = window.iter().fold(0usize, |acc, &digit| {
        acc.saturating_mul(radix).saturating_add(digit as usize)
    });
    rule_table.get(table_index).copied().unwrap_or(0)
}
