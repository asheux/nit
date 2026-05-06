use crate::rule_config::RulePersistence;

#[test]
fn rule_persistence_default_workspace_override_is_true() {
    let p = RulePersistence::default();
    assert!(
        p.workspace_override,
        "workspace_override defaults to true so per-workspace selections take precedence",
    );
    assert!(p.global_path.is_none());
    assert!(p.workspace_path.is_none());
}
