//! Cellular automaton (CA) strategy implementation.
//!
//! Provides [`CaStrategy`] and the shrinking-CA evaluation used by the
//! tournament engine and Metal GPU accelerator.

use super::math::{checked_pow_usize, integer_digits_unsigned};
use crate::game::Action;
use crate::history::{History, RoundRecord};
use std::collections::VecDeque;

/// Result of a single shrinking-CA evaluation pass.
#[derive(Clone, Debug)]
pub struct CaRunResult {
    pub rows: Vec<Vec<u8>>,
    pub output_symbol: u8,
    pub steps_executed: u32,
    pub stopped_early: bool,
}

/// Decode a rule code into a lookup table for the given symbol count
/// and neighborhood radius (`two_r = 2 * r`).
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

/// Look up the next cell symbol from the rule table for a given neighborhood window.
///
/// Interprets `window` as a mixed-radix index into `rule_table`, where each
/// cell is a digit in base `symbols`. Returns 0 if the index is out of range.
fn ca_transition_symbol(rule_table: &[u8], symbols: u8, window: &[u8]) -> u8 {
    let base = symbols.max(2) as usize;
    let mut idx = 0usize;
    for &digit in window {
        idx = idx.saturating_mul(base).saturating_add(digit as usize);
    }
    rule_table.get(idx).copied().unwrap_or(0)
}

// ── CA strategy ──────────────────────────────────────────────

/// CA-based strategy: encodes opponent history as a bit row, then evaluates
/// a shrinking cellular automaton to produce an action.
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
    /// Construct from rule parameters.
    ///
    /// Pre-decodes the rule table and allocates the sliding bit window
    /// sized to the maximum input the CA can consume.
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

    /// The numeric rule code for this CA.
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

/// Fixed-capacity sliding window of binary symbols.
///
/// Maintains at most `max_len` bits in FIFO order. Older bits are
/// discarded when the window is full. Used by [`CaStrategy`] to
/// build the CA input row incrementally from game history.
#[derive(Clone, Debug)]
struct BitWindow {
    max_len: usize,
    bits: VecDeque<u8>,
}

impl BitWindow {
    /// Create a new empty window with the given maximum length.
    fn new(max_len: usize) -> Self {
        Self {
            max_len: max_len.max(1),
            bits: VecDeque::new(),
        }
    }

    /// Remove all bits from the window.
    fn clear(&mut self) {
        self.bits.clear();
    }

    /// Append both player bits from a single round record.
    fn push_round(&mut self, record: RoundRecord) {
        self.push_bit(super::action_bit(record.a));
        self.push_bit(super::action_bit(record.b));
    }

    /// Append a single bit, evicting the oldest if at capacity.
    fn push_bit(&mut self, bit: u8) {
        self.bits.push_back(bit.min(1));
        while self.bits.len() > self.max_len {
            self.bits.pop_front();
        }
    }

    /// Snapshot the current window contents as a contiguous vector.
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
