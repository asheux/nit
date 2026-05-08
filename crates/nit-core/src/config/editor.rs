/// Editor preferences. Currently only the tab width — kept as its own struct
/// so future editor knobs (line-ending, soft-wrap, ruler columns) land here
/// without churning the top-level `Settings` shape.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct EditorConfig {
    pub tab_width: u8,
}

impl Default for EditorConfig {
    fn default() -> Self {
        Self { tab_width: 4 }
    }
}
