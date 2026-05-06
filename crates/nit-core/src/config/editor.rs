#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct EditorConfig {
    pub tab_width: u8,
}

impl Default for EditorConfig {
    fn default() -> Self {
        Self { tab_width: 4 }
    }
}
