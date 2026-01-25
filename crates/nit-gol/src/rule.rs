use thiserror::Error;

#[derive(Copy, Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Rule {
    births: u16,
    survives: u16,
}

impl Rule {
    pub fn conway() -> Self {
        Self::new(mask_from_digits(&[3]), mask_from_digits(&[2, 3]))
    }

    pub fn new(births: u16, survives: u16) -> Self {
        Self {
            births: births & 0x01ff,
            survives: survives & 0x01ff,
        }
    }

    pub fn births_mask(&self) -> u16 {
        self.births
    }

    pub fn survives_mask(&self) -> u16 {
        self.survives
    }

    pub fn is_birth(&self, neighbors: u8) -> bool {
        neighbors <= 8 && (self.births & (1 << neighbors)) != 0
    }

    pub fn is_survive(&self, neighbors: u8) -> bool {
        neighbors <= 8 && (self.survives & (1 << neighbors)) != 0
    }

    pub fn parse(text: &str) -> Result<Self, RuleParseError> {
        let mut births = 0u16;
        let mut survives = 0u16;
        let mut mode: Option<char> = None;
        let cleaned = text.trim().replace(' ', "");
        if cleaned.is_empty() {
            return Err(RuleParseError::Empty);
        }
        for ch in cleaned.chars() {
            match ch {
                'B' | 'b' => mode = Some('B'),
                'S' | 's' => mode = Some('S'),
                '/' => {}
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

    pub fn to_string(self) -> String {
        let births = digits_from_mask(self.births);
        let survives = digits_from_mask(self.survives);
        format!("B{}/S{}", births, survives)
    }
}

impl std::fmt::Display for Rule {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let births = digits_from_mask(self.births);
        let survives = digits_from_mask(self.survives);
        write!(f, "B{}/S{}", births, survives)
    }
}

fn digits_from_mask(mask: u16) -> String {
    let mut s = String::new();
    for i in 0..=8u8 {
        if (mask & (1 << i)) != 0 {
            s.push(char::from(b'0' + i));
        }
    }
    s
}

fn mask_from_digits(digits: &[u8]) -> u16 {
    let mut mask = 0u16;
    for d in digits {
        if *d <= 8 {
            mask |= 1 << d;
        }
    }
    mask
}

#[derive(Debug, Error)]
pub enum RuleParseError {
    #[error("empty rule")]
    Empty,
    #[error("missing rule section")]
    MissingSection,
    #[error("invalid character {0}")]
    InvalidChar(char),
}
