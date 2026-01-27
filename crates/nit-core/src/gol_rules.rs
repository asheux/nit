use crate::config::GolUserRule;

pub use nit_gol::{
    RuleCatalog, RuleDefaultParams, RuleEntry as NamedRule, RuleOverlay, RuleSelectError,
    SelectedRule,
};

pub fn load_rule_catalog(user_rules: &[GolUserRule]) -> (RuleCatalog, Vec<String>) {
    let overlays: Vec<RuleOverlay> = user_rules
        .iter()
        .map(|rule| RuleOverlay {
            id: rule.id.clone(),
            display_name: Some(rule.name.clone()),
            rulestring: Some(rule.rule.clone()),
            description: Some(rule.description.clone()),
            tags: None,
            aliases: None,
            default_params: None,
            provenance: None,
            favorite: None,
            hidden: None,
        })
        .collect();
    RuleCatalog::load_with_overlays(&overlays)
}
