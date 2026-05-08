// Picker overlays are transient UI state — not Serialize/Deserialize because
// persisting an open picker would race with the rule-catalog reload that
// runs after AppState deserialization.

#[derive(Clone, Debug, Default)]
pub struct RulePickerState {
    pub open: bool,
    pub query: String,
    pub selected: usize,
}

#[derive(Clone, Debug, Default)]
pub struct ProtocolPickerState {
    pub open: bool,
    pub selected: usize,
    pub custom_input: String,
    pub custom_error: Option<String>,
    pub custom_preview: Option<String>,
}
