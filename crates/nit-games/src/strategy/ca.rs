//! Cellular automaton (CA) strategy implementation.

use super::math::{checked_pow_usize, integer_digits_unsigned};
use crate::game::Action;
use crate::history::{History, RoundRecord};
use std::collections::VecDeque;

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
    let table_len = checked_pow_usize(symbols.max(2) as usize, neighborhood).unwrap_or(0);
    integer_digits_unsigned(rule_code as u128, symbols.max(2) as usize, table_len)
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
    steps: u32,
    input_row: &[u8],
) -> CaRunResult {
    let mut row = if input_row.is_empty() {
        vec![0]
    } else {
        input_row.to_vec()
    };
    let mut rows = vec![row.clone()];
    let mut steps_executed = 0u32;
    let mut stopped_early = false;
    let two_r = two_r as usize;
    let neighborhood = two_r.saturating_add(1);

    for _ in 0..steps {
        if neighborhood == 0 || row.len() <= two_r {
            stopped_early = true;
            break;
        }
        let next_len = row.len().saturating_sub(two_r);
        if next_len == 0 {
            stopped_early = true;
            break;
        }
        let mut next = Vec::with_capacity(next_len);
        for start in 0..next_len {
            let end = start.saturating_add(neighborhood);
            let value = ca_transition_symbol(rule_table, symbols, &row[start..end]);
            next.push(value);
        }
        row = next;
        rows.push(row.clone());
        steps_executed = steps_executed.saturating_add(1);
    }

    let output_symbol = row.last().copied().unwrap_or(0);
    CaRunResult {
        rows,
        output_symbol,
        steps_executed,
        stopped_early,
    }
}

/// Look up the next cell value from the rule table for a neighborhood window.
/// Interprets `window` as a mixed-radix index into the table.
fn ca_transition_symbol(rule_table: &[u8], symbols: u8, window: &[u8]) -> u8 {
    let radix = symbols.max(2) as usize;
    let mut table_index = 0usize;
    for &digit in window {
        table_index = table_index
            .saturating_mul(radix)
            .saturating_add(digit as usize);
    }
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
            symbols: symbols.max(2),
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

    fn sync_history(&mut self, history: &History) {
        sync_bit_window(&mut self.bit_window, history, &mut self.last_history_len);
    }

    fn evaluate_ca(&self, row: Vec<u8>) -> u8 {
        let run = run_shrinking_ca(&self.rule_table, self.symbols, self.two_r, self.steps, &row);
        run.output_symbol
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
        self.sync_history(history);
        if history.is_empty() {
            return Action::Cooperate;
        }
        let bits = self.bit_window.to_vec();
        let symbol = self.evaluate_ca(bits);
        if symbol == 0 {
            Action::Cooperate
        } else {
            Action::Defect
        }
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
        self.push_bit(super::action_bit(record.a));
        self.push_bit(super::action_bit(record.b));
    }

    fn push_bit(&mut self, bit: u8) {
        self.bits.push_back(bit.min(1));
        while self.bits.len() > self.capacity {
            self.bits.pop_front();
        }
    }

    fn to_vec(&self) -> Vec<u8> {
        self.bits.iter().copied().collect()
    }
}

fn sync_bit_window(window: &mut BitWindow, history: &History, last_history_len: &mut usize) {
    let len = history.len();
    if len < *last_history_len || len > (*last_history_len).saturating_add(1) {
        window.clear();
        for record in history.iter() {
            window.push_round(record);
        }
        *last_history_len = len;
        return;
    }
    if len == *last_history_len + 1 {
        if let Some(last) = history.last() {
            window.push_round(last);
        }
        *last_history_len = len;
    }
}
