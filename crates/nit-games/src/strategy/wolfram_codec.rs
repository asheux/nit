//! Wolfram-style mixed-radix codec for one-sided Turing machine rule numbers.
//!
//! A rule code is interpreted as a base-`2 * states * symbols` integer. Each
//! digit packs `(move_flag, write_symbol, next_state)` in mixed-radix order;
//! we walk the table in row-major `(state, read_symbol)` order, with the
//! state axis traversed from highest down to 1 so the most-significant
//! digits land in the highest state's row.

use super::math::checked_pow_u128;
use super::{TmMove, TmTransition};

/// Decode a Wolfram-style rule code into a flat transition table.
/// `remaining` in the return is any unused higher-order digits.
pub fn decode_tm_rule_code_wolfram(
    rule_code: u64,
    states: usize,
    symbols: usize,
) -> (Vec<TmTransition>, u64) {
    let entry_count = states.saturating_mul(symbols);
    let mut transitions = vec![
        TmTransition {
            write: 0,
            move_dir: TmMove::Left,
            next: 1,
        };
        entry_count
    ];
    if states == 0 || symbols == 0 {
        return (transitions, rule_code);
    }
    let digit_radix = (symbols as u64) * (states as u64) * 2;
    let mut undecoded_suffix = rule_code;
    for wolfram_state in (1..=states).rev() {
        for read_symbol in 0..symbols {
            let decoded_rule = decode_single_wolfram_digit(undecoded_suffix, digit_radix, symbols);
            undecoded_suffix /= digit_radix;
            let table_offset = (wolfram_state - 1) * symbols + read_symbol;
            transitions[table_offset] = decoded_rule;
        }
    }
    (transitions, undecoded_suffix)
}

/// Extract a single TM transition from the lowest digit of the rule code:
/// `digit = move_flag + 2 * write_symbol + 2 * symbols * (next_state - 1)`.
fn decode_single_wolfram_digit(
    remaining_code: u64,
    mixed_radix_base: u64,
    symbol_count: usize,
) -> TmTransition {
    let digit_value = remaining_code % mixed_radix_base;
    let move_flag = (digit_value % 2) as u8;
    let write_symbol = ((digit_value / 2) % symbol_count as u64) as u8;
    let next_state = (digit_value / (2 * symbol_count as u64)) as u16 + 1;
    let head_direction = if move_flag == 0 {
        TmMove::Left
    } else {
        TmMove::Right
    };
    TmTransition {
        write: write_symbol,
        move_dir: head_direction,
        next: next_state,
    }
}

/// Maximum valid Wolfram rule index for the given state/symbol counts, or
/// `None` when the index space overflows `u128`.
pub fn tm_max_index(states: usize, symbols: usize) -> Option<u128> {
    let mixed_radix = (2u128)
        .checked_mul(states as u128)?
        .checked_mul(symbols as u128)?;
    let total_entries = states.checked_mul(symbols)? as u32;
    checked_pow_u128(mixed_radix, total_entries)?.checked_sub(1)
}
