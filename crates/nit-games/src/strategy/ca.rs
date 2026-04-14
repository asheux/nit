//! Cellular automaton (CA) strategy implementation.

use std::collections::VecDeque;

use super::math::{checked_pow_usize, integer_digits_unsigned};
use crate::game::Action;
use crate::history::{History, RoundRecord};

/// Minimum symbol count for CA rule tables (binary minimum).
const MIN_SYMBOLS: u8 = 2;

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

// ── CA strategy ──────────────────────────────────────────────

/// Encodes opponent history as a bit row, then evaluates a shrinking CA
/// to produce an action.
#[derive(Clone, Debug)]
pub struct CaStrategy {
    id: String,
    rule_code: u64,
    symbols: u8,
    two_r: u32,
    steps: u32,
    rule_table: Vec<u8>,
    bit_window: BitWindow,
    last_history_len: usize,
}

impl CaStrategy {
    /// Pre-decodes the rule table and sizes the sliding bit window to the
    /// maximum input the CA can consume.
    pub fn new(id: impl Into<String>, rule_code: u64, symbols: u8, two_r: u32, steps: u32) -> Self {
        let rule_table = decode_ca_rule_table(rule_code, symbols, two_r);
        let suffix_len = two_r.saturating_mul(steps).saturating_add(1).max(1) as usize;
        Self {
            id: id.into(),
            rule_code,
            symbols: symbols.max(MIN_SYMBOLS),
            two_r,
            steps,
            rule_table,
            bit_window: BitWindow::new(suffix_len),
            last_history_len: 0,
        }
    }

    pub fn rule_code(&self) -> u64 {
        self.rule_code
    }
}

impl super::Strategy for CaStrategy {
    fn id(&self) -> &str {
        &self.id
    }

    fn reset(&mut self) {
        self.bit_window.clear();
        self.last_history_len = 0;
    }

    fn next_action(&mut self, history: &History, _player_a: bool) -> Action {
        self.bit_window.sync(history, &mut self.last_history_len);
        if history.is_empty() {
            return Action::Cooperate;
        }
        let input_bits = self.bit_window.to_vec();
        let ca_result = run_shrinking_ca(
            &self.rule_table,
            self.symbols,
            self.two_r,
            self.steps,
            &input_bits,
        );
        super::symbol_to_action(ca_result.output_symbol)
    }
}

// ── Sliding bit window ───────────────────────────────────────

/// Fixed-capacity FIFO sliding window of binary symbols. Older bits are
/// discarded when the window is full.
#[derive(Clone, Debug)]
struct BitWindow {
    capacity: usize,
    bits: VecDeque<u8>,
}

impl BitWindow {
    fn new(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            bits: VecDeque::new(),
        }
    }

    fn clear(&mut self) {
        self.bits.clear();
    }

    fn push_round(&mut self, record: RoundRecord) {
        let action_a_bit = super::action_bit(record.a);
        let action_b_bit = super::action_bit(record.b);
        self.bits.push_back(action_a_bit);
        self.bits.push_back(action_b_bit);
        while self.bits.len() > self.capacity {
            self.bits.pop_front();
        }
    }

    fn to_vec(&self) -> Vec<u8> {
        self.bits.iter().copied().collect()
    }

    fn sync(&mut self, history: &History, last_len: &mut usize) {
        let current_len = history.len();
        if current_len == *last_len {
            return;
        }
        if current_len == *last_len + 1 {
            if let Some(most_recent) = history.last() {
                self.push_round(most_recent);
            }
        } else {
            self.clear();
            for record in history.iter() {
                self.push_round(record);
            }
        }
        *last_len = current_len;
    }
}
