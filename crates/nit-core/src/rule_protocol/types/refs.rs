//! Catalogue references stored inside a `RuleProtocol` phase.
//!
//! `selector()` is the round-trip key for protocol specs: when a rule has a
//! catalogue id we use that (so user-renamed rules still resolve), and we
//! fall back to the rulestring for ad-hoc rules without an id.

use nit_gol::Rule;

use crate::gol_rules::{NamedRule, SelectedRule};

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct RuleRef {
    pub id: Option<String>,
    pub rule: Rule,
    pub name: Option<String>,
}

impl RuleRef {
    pub fn from_selected(selected: &SelectedRule) -> Self {
        Self {
            id: selected.id.clone(),
            rule: selected.rule,
            name: selected.name.clone(),
        }
    }

    pub fn from_catalog(rule: &NamedRule) -> Self {
        Self {
            id: Some(rule.id.clone()),
            rule: rule.rule,
            name: Some(rule.name.clone()),
        }
    }

    pub fn label(&self) -> String {
        match &self.name {
            Some(name) => format!("{} ({})", self.rule, name),
            None => self.rule.to_string(),
        }
    }

    pub fn selector(&self) -> String {
        self.id.clone().unwrap_or_else(|| self.rule.to_string())
    }
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct RulePhase {
    pub rule: RuleRef,
    pub steps: u32,
    pub label: Option<String>,
}
