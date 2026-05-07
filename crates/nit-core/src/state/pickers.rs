// Picker overlays are transient UI state — intentionally not Serialize/Deserialize.
// Each launch resets them; persisting an open picker would race with the rule
// catalog reload that happens after deserialization.

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
