//! `CaStrategy` — encodes opponent history as a bit row, evaluates a shrinking
//! cellular automaton each round, and emits the rightmost output symbol.

use std::collections::VecDeque;

use super::eval::{decode_ca_rule_table, run_shrinking_ca, MIN_SYMBOLS};
use crate::game::Action;
use crate::history::{History, RoundRecord};
use crate::strategy::{action_bit, symbol_to_action, Strategy};

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

impl Strategy for CaStrategy {
    fn id(&self) -> &str {
        &self.id
    }

    fn reset(&mut self) {
        self.bit_window.bits.clear();
        self.last_history_len = 0;
    }

    fn next_action(&mut self, history: &History, _player_a: bool) -> Action {
        self.bit_window.sync(history, &mut self.last_history_len);
        if history.is_empty() {
            return Action::Cooperate;
        }
        let input_bits: Vec<u8> = self.bit_window.bits.iter().copied().collect();
        let ca_result = run_shrinking_ca(
            &self.rule_table,
            self.symbols,
            self.two_r,
            self.steps,
            &input_bits,
        );
        symbol_to_action(ca_result.output_symbol)
    }
}

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

    fn push_round(&mut self, record: RoundRecord) {
        self.bits.push_back(action_bit(record.a));
        self.bits.push_back(action_bit(record.b));
        while self.bits.len() > self.capacity {
            self.bits.pop_front();
        }
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
            self.bits.clear();
            for record in history.iter() {
                self.push_round(record);
            }
        }
        *last_len = current_len;
    }
}
