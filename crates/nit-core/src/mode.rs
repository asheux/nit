#[derive(Copy, Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Mode {
    Insert,
    Normal,
    Visual,
}

impl Mode {
    pub fn toggle(self) -> Self {
        match self {
            Mode::Insert => Mode::Normal,
            Mode::Normal => Mode::Insert,
            Mode::Visual => Mode::Normal,
        }
    }
}
