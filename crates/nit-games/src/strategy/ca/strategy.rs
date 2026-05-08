//! `CaStrategy` — encodes opponent history as a bit row, evaluates a
//! shrinking cellular automaton each round, and emits the rightmost
//! output symbol.

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
    /// Sliding window of joint action bits; capacity = `2r * steps + 1`.
    /// Older bits roll off as new rounds arrive.
    bits: VecDeque<u8>,
    bit_capacity: usize,
    last_history_len: usize,
}

impl CaStrategy {
    /// Pre-decodes the rule table and sizes the sliding bit window to the
    /// maximum input the CA can consume in one evaluation.
    pub fn new(id: impl Into<String>, rule_code: u64, symbols: u8, two_r: u32, steps: u32) -> Self {
        let rule_table = decode_ca_rule_table(rule_code, symbols, two_r);
        let bit_capacity = two_r.saturating_mul(steps).saturating_add(1).max(1) as usize;
        Self {
            id: id.into(),
            rule_code,
            symbols: symbols.max(MIN_SYMBOLS),
            two_r,
            steps,
            rule_table,
            bits: VecDeque::new(),
            bit_capacity: bit_capacity.max(1),
            last_history_len: 0,
        }
    }

    pub fn rule_code(&self) -> u64 {
        self.rule_code
    }

    fn push_round(&mut self, record: RoundRecord) {
        self.bits.push_back(action_bit(record.a));
        self.bits.push_back(action_bit(record.b));
        while self.bits.len() > self.bit_capacity {
            self.bits.pop_front();
        }
    }

    /// Bring the sliding window up to date with `history`, taking the
    /// incremental fast path when exactly one round has been appended.
    fn sync_window(&mut self, history: &History) {
        let current_len = history.len();
        if current_len == self.last_history_len {
            return;
        }
        if current_len == self.last_history_len + 1 {
            if let Some(most_recent) = history.last() {
                self.push_round(most_recent);
            }
        } else {
            self.bits.clear();
            for record in history.iter() {
                self.push_round(record);
            }
        }
        self.last_history_len = current_len;
    }
}

impl Strategy for CaStrategy {
    fn id(&self) -> &str {
        &self.id
    }

    fn reset(&mut self) {
        self.bits.clear();
        self.last_history_len = 0;
    }

    fn next_action(&mut self, history: &History, _player_a: bool) -> Action {
        self.sync_window(history);
        if history.is_empty() {
            return Action::Cooperate;
        }
        let input_bits: Vec<u8> = self.bits.iter().copied().collect();
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
