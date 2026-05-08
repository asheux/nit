//! Incremental base-`base` digit window over the joint action history.
//!
//! Each round contributes a base-4 pair `(a_bit << 1) | b_bit`. The suffix
//! tracks only the `width` least-significant digits in little-endian order so
//! it can drive a one-sided TM tape without re-scanning the full history.
//! When the prefix discards a non-zero digit, `prefix_nonzero` latches and the
//! caller receives the full-width window with leading zeros preserved — that
//! signals "input is at least width digits wide".

use crate::history::RoundRecord;
use crate::strategy::action_bit;

#[derive(Clone, Debug)]
pub(crate) struct InputSuffix {
    base: u8,
    width: usize,
    digits_le: Vec<u8>,
    prefix_nonzero: bool,
    pub(crate) history_len: usize,
}

impl InputSuffix {
    pub(crate) fn new(base: u8, width: usize) -> Self {
        Self {
            base: base.max(2),
            width: width.max(1),
            digits_le: vec![0],
            prefix_nonzero: false,
            history_len: 0,
        }
    }

    pub(crate) fn reset(&mut self) {
        self.digits_le.clear();
        self.digits_le.push(0);
        self.prefix_nonzero = false;
        self.history_len = 0;
    }

    pub(crate) fn push_round(&mut self, round: RoundRecord) {
        self.push_pair_bits(action_bit(round.a), action_bit(round.b));
    }

    pub(crate) fn push_pair_bits(&mut self, a_bit: u8, b_bit: u8) {
        let action_pair = ((a_bit << 1) | b_bit) as u16;
        self.mul_add(4, action_pair);
    }

    fn mul_add(&mut self, multiplier: u16, addend: u16) {
        let radix = self.base as u16;
        let mut overflow = addend;
        for slot in &mut self.digits_le {
            let product = (*slot as u16)
                .saturating_mul(multiplier)
                .saturating_add(overflow);
            *slot = (product % radix) as u8;
            overflow = product / radix;
        }
        self.propagate_carry(overflow, radix);
        self.truncate_to_width();
        if self.prefix_nonzero {
            self.trim_most_significant_zeros_with_prefix();
        } else {
            self.trim_redundant_high_zeros();
        }
    }

    fn propagate_carry(&mut self, mut remaining: u16, radix: u16) {
        while remaining > 0 {
            if self.digits_le.len() < self.width {
                self.digits_le.push((remaining % radix) as u8);
                remaining /= radix;
            } else {
                self.prefix_nonzero = true;
                return;
            }
        }
    }

    fn truncate_to_width(&mut self) {
        if self.digits_le.len() > self.width {
            if self.digits_le[self.width..].iter().any(|&d| d != 0) {
                self.prefix_nonzero = true;
            }
            self.digits_le.truncate(self.width);
        }
    }

    /// Strips leading zeros unless the prefix-overflow flag is set, in which
    /// case the full width is preserved so callers see a stable tape length.
    pub(crate) fn msd_digits(&self) -> Vec<u8> {
        if self.digits_le.is_empty() {
            return vec![0];
        }
        let msd_iter = self.digits_le.iter().rev().copied();
        if self.prefix_nonzero {
            msd_iter.collect()
        } else {
            let result: Vec<u8> = msd_iter.skip_while(|&d| d == 0).collect();
            if result.is_empty() {
                vec![0]
            } else {
                result
            }
        }
    }

    fn trim_redundant_high_zeros(&mut self) {
        while self.digits_le.len() > 1 && self.digits_le.last() == Some(&0) {
            self.digits_le.pop();
        }
    }

    fn trim_most_significant_zeros_with_prefix(&mut self) {
        while self.digits_le.len() > self.width {
            self.digits_le.pop();
        }
        while self.digits_le.len() > 1 && self.digits_le.last() == Some(&0) {
            if self.digits_le.len() == self.width {
                break;
            }
            self.digits_le.pop();
        }
    }
}
