//! Birth/survival rule representation and parsing.
//!
//! Rules are stored as a pair of 9-bit masks where bit `n` indicates the
//! condition applies to a cell with exactly `n` live neighbors. The
//! standard `B.../S...` notation is supported for parsing and display.

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
    #[must_use]
    pub fn new(births: u16, survives: u16) -> Self {
        debug_assert!(births & !MASK_9_BITS == 0 && survives & !MASK_9_BITS == 0);
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
    #[must_use]
    pub fn is_birth(&self, neighbors: u8) -> bool {
        neighbors <= 8 && (self.births_mask & (1 << neighbors)) != 0
    }

    /// Returns `true` if a live cell with `neighbors` live neighbors survives.
    #[must_use]
    pub fn is_survive(&self, neighbors: u8) -> bool {
        neighbors <= 8 && (self.survives_mask & (1 << neighbors)) != 0
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
        let mut births = 0u16;
        let mut survives = 0u16;
        let mut section: Option<Section> = None;
        let mut seen_section = false;
        let mut seen_slash = false;
        for ch in cleaned.chars() {
            match ch {
                'B' | 'b' => {
                    section = Some(Section::Births);
                    seen_section = true;
                }
                'S' | 's' => {
                    section = Some(Section::Survives);
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
                    section = None;
                }
                '0'..='8' => {
                    let bit = 1u16 << (ch as u8 - b'0');
                    match section {
                        Some(Section::Births) => births |= bit,
                        Some(Section::Survives) => survives |= bit,
                        None => return Err(RuleParseError::MissingSection),
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
        f.write_str("B")?;
        for i in 0..=8u8 {
            if (self.births_mask & (1 << i)) != 0 {
                f.write_str(DIGITS[i as usize])?;
            }
        }
        f.write_str("/S")?;
        for i in 0..=8u8 {
            if (self.survives_mask & (1 << i)) != 0 {
                f.write_str(DIGITS[i as usize])?;
            }
        }
        Ok(())
    }
}

const DIGITS: [&str; 9] = ["0", "1", "2", "3", "4", "5", "6", "7", "8"];

#[derive(Copy, Clone)]
enum Section {
    Births,
    Survives,
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
