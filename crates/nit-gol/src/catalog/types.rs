//! Public data types for the rule catalog.

use std::fmt;

use crate::{Rule, RuleParseError};

/// Optional per-rule parameters embedded in the catalog.
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct RuleDefaultParams {
    #[serde(default)]
    pub warning: Option<String>,
    #[serde(default)]
    pub wrap: Option<String>,
}

/// A user-supplied overlay that modifies or adds catalog entries.
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

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum RuleSource {
    /// Shipped with the binary.
    Builtin,
    /// Added or overridden by user configuration.
    User,
}

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

/// User's current rule selection plus optional catalog metadata.
///
/// Field names (`rule`, `id`, `name`) are part of the on-disk config
/// contract via serde — do not rename without a migration path.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct SelectedRule {
    pub rule: Rule,
    pub id: Option<String>,
    pub name: Option<String>,
}

impl SelectedRule {
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

    /// Most-specific selector string: the id when known, else the rulestring.
    pub fn selector(&self) -> String {
        self.id.clone().unwrap_or_else(|| self.rule.to_string())
    }

    /// Format as `rulestring (name)`.
    pub fn label(&self) -> String {
        self.format_label(false)
    }

    /// Format as `name (rulestring)`.
    pub fn name_first_label(&self) -> String {
        self.format_label(true)
    }

    fn format_label(&self, name_first: bool) -> String {
        match &self.name {
            Some(name) if name_first => format!("{} ({})", name, self.rule),
            Some(name) => format!("{} ({})", self.rule, name),
            None => self.rule.to_string(),
        }
    }
}

impl Default for SelectedRule {
    fn default() -> Self {
        Self::from_rule(Rule::conway())
    }
}

#[derive(Debug)]
pub enum RuleSelectError {
    /// Selector did not match any id or alias and was not a parseable rulestring.
    UnknownId(String),
    /// Selector parsed as a rulestring candidate but the grammar rejected it.
    Parse(RuleParseError),
}

impl fmt::Display for RuleSelectError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownId(value) => write!(f, "unknown rule id '{value}'"),
            Self::Parse(err) => write!(f, "{err}"),
        }
    }
}

impl std::error::Error for RuleSelectError {}
