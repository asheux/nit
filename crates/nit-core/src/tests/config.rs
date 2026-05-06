use crate::config::Settings;

#[test]
fn settings_default_round_trips_through_toml() {
    let original = Settings::default();
    let serialized = toml::to_string(&original).expect("serialize defaults");
    let restored: Settings = toml::from_str(&serialized).expect("deserialize defaults");
    assert_eq!(original.intake_enabled, restored.intake_enabled);
    assert_eq!(
        original.editor.tab_width, restored.editor.tab_width,
        "editor tab_width must round-trip",
    );
    assert_eq!(
        original.swarm.gate_retry_limit,
        restored.swarm.gate_retry_limit
    );
}

#[test]
fn settings_tolerates_missing_optional_keys() {
    // Older config files predating `swarm`, `genome`, and `intake_enabled`
    // must keep deserializing — the `#[serde(default)]` attributes provide
    // the back-compat hooks.
    let baseline = toml::to_string(&Settings::default()).expect("serialize defaults");
    let mut trimmed: toml::Table = toml::from_str(&baseline).expect("re-parse defaults");
    trimmed.remove("swarm");
    trimmed.remove("genome");
    trimmed.remove("intake_enabled");
    let serialized = toml::to_string(&trimmed).expect("serialize trimmed");
    let parsed: Settings = toml::from_str(&serialized).expect("deserialize trimmed config");
    assert!(parsed.intake_enabled, "intake_enabled defaults to true");
}
