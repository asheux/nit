//! Birth/survival rule representation and parsing.
//!
//! Rules are encoded as a pair of 9-bit masks (one for births, one for
//! survival) where bit `n` means the condition applies when a cell has
//! `n` live neighbors. The standard `B.../S...` notation is supported
//! for parsing and display.

use thiserror::Error;

/// A Life-like cellular automaton rule.
///
/// Internally stores two 9-bit masks: one for birth conditions and
/// one for survival conditions. Bit `n` of each mask indicates that
/// the condition applies when a cell has exactly `n` live neighbors.
#[derive(Copy, Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Rule {
    births_mask: u16,
    survives_mask: u16,
}

impl Rule {
    /// The classic Conway's Game of Life rule (B3/S23).
    pub fn conway() -> Self {
        Self::new(mask_from_digits(&[3]), mask_from_digits(&[2, 3]))
    }

    /// Construct a rule from pre-computed birth and survival masks.
    pub fn new(births: u16, survives: u16) -> Self {
        debug_assert!(is_valid_mask(births) && is_valid_mask(survives));
        Self {
            births_mask: births & 0x01ff,
            survives_mask: survives & 0x01ff,
        }
    }

    /// Raw 9-bit birth condition mask.
    pub fn births_mask(&self) -> u16 {
        self.births_mask
    }

    /// Raw 9-bit survival condition mask.
    pub fn survives_mask(&self) -> u16 {
        self.survives_mask
    }

    /// Returns `true` if a dead cell with `neighbors` alive neighbors is born.
    pub fn is_birth(&self, neighbors: u8) -> bool {
        neighbors <= 8 && (self.births_mask & (1 << neighbors)) != 0
    }

    /// Returns `true` if a living cell with `neighbors` alive neighbors survives.
    pub fn is_survive(&self, neighbors: u8) -> bool {
        neighbors <= 8 && (self.survives_mask & (1 << neighbors)) != 0
    }

    /// Parse a rule from `B.../S...` notation.
    ///
    /// Accepts mixed case, optional spaces, and a single `/` separator.
    /// Examples: `"B3/S23"`, `"b36 / s23"`, `"B2/S"`.
    pub fn parse(text: &str) -> Result<Self, RuleParseError> {
        let mut births = 0u16;
        let mut survives = 0u16;
        let mut mode: Option<char> = None;
        let mut seen_section = false;
        let mut seen_slash = false;
        let cleaned = text.trim().replace(' ', "");
        if cleaned.is_empty() {
            return Err(RuleParseError::Empty);
        }
        for ch in cleaned.chars() {
            match ch {
                'B' | 'b' => {
                    mode = Some('B');
                    seen_section = true;
                }
                'S' | 's' => {
                    mode = Some('S');
                    seen_section = true;
                }
                '/' => {
                    if seen_slash {
                        return Err(RuleParseError::InvalidSeparator);
                    }
                    if !seen_section {
                        return Err(RuleParseError::MissingSection);
                    }
                    seen_slash = true;
                    mode = None;
                }
                '0'..='8' => {
                    let val = ch.to_digit(10).unwrap() as u8;
                    match mode {
                        Some('B') => births |= 1 << val,
                        Some('S') => survives |= 1 << val,
                        _ => return Err(RuleParseError::MissingSection),
                    }
                }
                _ => return Err(RuleParseError::InvalidChar(ch)),
            }
        }
        if births == 0 && survives == 0 {
            return Err(RuleParseError::MissingSection);
        }
        Ok(Self::new(births, survives))
    }
}

impl std::fmt::Display for Rule {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let births = digits_from_mask(self.births_mask);
        let survives = digits_from_mask(self.survives_mask);
        write!(f, "B{births}/S{survives}")
    }
}

/// Convert a 9-bit mask to its digit string representation.
fn digits_from_mask(mask: u16) -> String {
    let mut s = String::new();
    for i in 0..=8u8 {
        if (mask & (1 << i)) != 0 {
            s.push(char::from(b'0' + i));
        }
    }
    s
}

/// Convert a slice of neighbor counts to a 9-bit mask.
fn mask_from_digits(digits: &[u8]) -> u16 {
    let mut mask = 0u16;
    for d in digits {
        if *d <= 8 {
            mask |= 1 << d;
        }
    }
    mask
}

/// Returns `true` if the mask only uses bits 0–8.
fn is_valid_mask(mask: u16) -> bool {
    mask & !0x01ff == 0
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
