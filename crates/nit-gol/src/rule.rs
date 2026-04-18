//! Birth/survival rule representation and parsing.
//!
//! Rules are stored as a pair of 9-bit masks where bit `n` indicates the
//! condition applies to a cell with exactly `n` live neighbors. The
//! standard `B.../S...` notation is supported for parsing and display.

use std::fmt::{self, Write};

use thiserror::Error;

const MASK_9_BITS: u16 = 0x01ff;

/// A Life-like cellular automaton rule.
#[derive(Copy, Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Rule {
    births_mask: u16,
    survives_mask: u16,
}

impl Rule {
    /// Conway's Game of Life (`B3/S23`).
    #[must_use]
    pub fn conway() -> Self {
        Self::new(1 << 3, (1 << 2) | (1 << 3))
    }

    /// Construct a rule from pre-computed birth and survival masks.
    ///
    /// Bits above bit 8 are silently trimmed; in debug builds this trim
    /// triggers a `debug_assert` so accidentally-set high bits surface at
    /// construction rather than inside the hot neighbor-count loop.
    #[must_use]
    pub fn new(births: u16, survives: u16) -> Self {
        debug_assert!(
            births & !MASK_9_BITS == 0 && survives & !MASK_9_BITS == 0,
            "rule masks must fit in 9 bits",
        );
        Self {
            births_mask: births & MASK_9_BITS,
            survives_mask: survives & MASK_9_BITS,
        }
    }

    #[must_use]
    pub fn births_mask(&self) -> u16 {
        self.births_mask
    }

    #[must_use]
    pub fn survives_mask(&self) -> u16 {
        self.survives_mask
    }

    /// Returns `true` if a dead cell with `neighbors` live neighbors is born.
    ///
    /// The `<= 8` guard defends against callers using non-Moore
    /// neighborhoods; inside this crate `step` always stays in range.
    #[must_use]
    pub fn is_birth(&self, neighbors: u8) -> bool {
        neighbors <= 8 && self.births_mask & (1u16 << neighbors) != 0
    }

    /// Returns `true` if a live cell with `neighbors` live neighbors survives.
    #[must_use]
    pub fn is_survive(&self, neighbors: u8) -> bool {
        neighbors <= 8 && self.survives_mask & (1u16 << neighbors) != 0
    }

    /// Parse a rule from `B.../S...` notation.
    ///
    /// Accepts mixed case, optional spaces, and a single `/` separator.
    /// Examples: `"B3/S23"`, `"b36 / s23"`, `"B2/S"`.
    pub fn parse(text: &str) -> Result<Self, RuleParseError> {
        let cleaned = text.trim().replace(' ', "");
        if cleaned.is_empty() {
            return Err(RuleParseError::Empty);
        }
        let mut segments = cleaned.split('/');
        let first_seg = segments.next().unwrap_or("");
        let second_seg = segments.next().unwrap_or("");
        if segments.next().is_some() {
            return Err(RuleParseError::InvalidSeparator);
        }
        if first_seg.is_empty() {
            return Err(RuleParseError::MissingSection);
        }

        let mut births = 0u16;
        let mut survives = 0u16;
        let mut saw_prefix = false;
        for segment in [first_seg, second_seg] {
            if segment.is_empty() {
                continue;
            }
            let mut chars = segment.chars();
            let prefix = chars.next().unwrap();
            let target = match prefix {
                'B' | 'b' => &mut births,
                'S' | 's' => &mut survives,
                other => return Err(RuleParseError::InvalidChar(other)),
            };
            saw_prefix = true;
            for symbol in chars {
                match symbol {
                    '0'..='8' => *target |= 1u16 << (symbol as u8 - b'0'),
                    other => return Err(RuleParseError::InvalidChar(other)),
                }
            }
        }

        if !saw_prefix || (births == 0 && survives == 0) {
            return Err(RuleParseError::MissingSection);
        }
        Ok(Self::new(births, survives))
    }
}

impl fmt::Display for Rule {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_char('B')?;
        for birth_digit in active_digits(self.births_mask) {
            f.write_char((b'0' + birth_digit) as char)?;
        }
        f.write_str("/S")?;
        for survival_digit in active_digits(self.survives_mask) {
            f.write_char((b'0' + survival_digit) as char)?;
        }
        Ok(())
    }
}

fn active_digits(mask: u16) -> impl Iterator<Item = u8> {
    (0..=8u8).filter(move |bit| mask & (1u16 << bit) != 0)
}

/// Errors that can occur when parsing a rule string.
#[derive(Debug, Error)]
pub enum RuleParseError {
    #[error("empty rule")]
    Empty,
    #[error("missing rule section")]
    MissingSection,
    #[error("invalid rule separator")]
    InvalidSeparator,
    #[error("invalid character {0}")]
    InvalidChar(char),
}
