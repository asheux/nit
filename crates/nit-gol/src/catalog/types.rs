//! Public data types for the rule catalog.

use crate::{Rule, RuleParseError};

/// Optional per-rule parameters embedded in the catalog.
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct RuleDefaultParams {
    #[serde(default)]
    pub warning: Option<String>,
    #[serde(default)]
    pub wrap: Option<String>,
}

/// A user-supplied overlay that can modify or add catalog entries.
#[derive(Clone, Debug, serde::Deserialize)]
pub struct RuleOverlay {
    pub id: String,
    #[serde(default, alias = "name")]
    pub display_name: Option<String>,
    #[serde(default, alias = "rule")]
    pub rulestring: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub tags: Option<Vec<String>>,
    #[serde(default)]
    pub aliases: Option<Vec<String>>,
    #[serde(default)]
    pub default_params: Option<RuleDefaultParams>,
    #[serde(default)]
    pub provenance: Option<Vec<String>>,
    #[serde(default)]
    pub favorite: Option<bool>,
    #[serde(default)]
    pub hidden: Option<bool>,
}

/// Provenance of a catalog entry.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum RuleSource {
    /// Shipped with the binary.
    Builtin,
    /// Added or overridden by user configuration.
    User,
}

/// A fully resolved rule entry in the catalog.
#[derive(Clone, Debug)]
pub struct RuleEntry {
    pub id: String,
    pub name: String,
    pub rule: Rule,
    pub rulestring: String,
    pub rulestring_raw: String,
    pub description: String,
    pub tags: Vec<String>,
    pub aliases: Vec<String>,
    pub default_params: RuleDefaultParams,
    pub provenance: Vec<String>,
    pub favorite: bool,
    pub hidden: bool,
    pub source: RuleSource,
}

impl RuleEntry {
    pub fn warning(&self) -> Option<&str> {
        self.default_params.warning.as_deref()
    }
}

/// A user's current rule selection with optional catalog metadata.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct SelectedRule {
    pub rule: Rule,
    pub id: Option<String>,
    pub name: Option<String>,
}

impl SelectedRule {
    /// Bare rule selection with no catalog metadata attached.
    pub fn from_rule(rule: Rule) -> Self {
        Self {
            rule,
            id: None,
            name: None,
        }
    }

    pub fn from_named(entry: &RuleEntry) -> Self {
        Self {
            rule: entry.rule,
            id: Some(entry.id.clone()),
            name: Some(entry.name.clone()),
        }
    }

    /// Return the most specific selector string for this rule.
    pub fn selector(&self) -> String {
        self.id.clone().unwrap_or_else(|| self.rule.to_string())
    }

    /// Format as `rulestring (name)` for display.
    pub fn label(&self) -> String {
        match &self.name {
            Some(name) => format!("{} ({})", self.rule, name),
            None => self.rule.to_string(),
        }
    }

    /// Format as `name (rulestring)` for display.
    pub fn name_first_label(&self) -> String {
        match &self.name {
            Some(name) => format!("{} ({})", name, self.rule),
            None => self.rule.to_string(),
        }
    }
}

impl Default for SelectedRule {
    fn default() -> Self {
        SelectedRule::from_rule(Rule::conway())
    }
}

/// Error returned when a rule selector cannot be resolved.
#[derive(Debug)]
pub enum RuleSelectError {
    /// Selector did not match any id or alias and was not a parseable rulestring.
    UnknownId(String),
    /// Selector parsed as a rulestring candidate but the grammar rejected it.
    Parse(RuleParseError),
}

impl std::fmt::Display for RuleSelectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RuleSelectError::UnknownId(value) => write!(f, "unknown rule id '{value}'"),
            RuleSelectError::Parse(err) => write!(f, "{err}"),
        }
    }
}

impl std::error::Error for RuleSelectError {}
