//! Rule selection paths: by id, by string, and via the rule picker.

use super::*;
use crate::rule_config::RulePersistence;

#[test]
fn set_rule_by_id_updates_and_persists() {
    let root = temp_dir("rule-id");
    let config_path = root.join("config.toml");
    let mut state = AppState::new(
        root.clone(),
        Buffer::empty("x", None),
        Buffer::empty("n", None),
    );
    state.rule_persistence = RulePersistence {
        global_path: Some(config_path.clone()),
        workspace_path: None,
        workspace_override: false,
    };
    let named = state.rule_catalog.find_by_id("highlife").unwrap();
    let selected = SelectedRule::from_named(named);
    state.set_gol_rule(selected, true).unwrap();
    assert_eq!(state.visualizer.rule, "B36/S23");
    let contents = fs::read_to_string(config_path).unwrap();
    assert!(contents.contains("default = \"B36/S23\""));
}

#[test]
fn set_rule_by_string_updates_state() {
    let (_root, mut state) = empty_state("rule-str");
    let rule = Rule::parse("B36/S23").unwrap();
    let selected = SelectedRule::from_rule(rule);
    state.set_gol_rule(selected, false).unwrap();
    assert_eq!(state.visualizer.rule, "B36/S23");
}

#[test]
fn rule_picker_apply_sets_rule() {
    let (_root, mut state) = empty_state("rule-picker");
    state.rule_picker.open = true;
    state.rule_picker.query = "highlife".into();
    state.rule_picker.selected = 0;
    let _ = apply_action(&mut state, Action::ApplySelectedRuleFromPicker);
    assert_eq!(state.visualizer.rule, "B36/S23");
}
