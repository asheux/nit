use std::collections::HashSet;

use nit_gol::{Rule, RuleParseError};

use crate::config::GolUserRule;

#[derive(Clone, Debug)]
pub struct NamedRule {
    pub id: String,
    pub name: String,
    pub rule: Rule,
    pub description: String,
}

impl NamedRule {
    fn new(id: &str, name: &str, rule: Rule, description: &str) -> Self {
        Self {
            id: id.to_string(),
            name: name.to_string(),
            rule,
            description: description.to_string(),
        }
    }
}

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

    pub fn from_named(named: &NamedRule) -> Self {
        Self {
            rule: named.rule,
            id: Some(named.id.clone()),
            name: Some(named.name.clone()),
        }
    }

    pub fn selector(&self) -> String {
        self.id
            .clone()
            .unwrap_or_else(|| self.rule.to_string())
    }

    pub fn label(&self) -> String {
        match &self.name {
            Some(name) => format!("{} ({})", self.rule, name),
            None => self.rule.to_string(),
        }
    }

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

#[derive(Clone, Debug)]
pub struct RuleCatalog {
    builtins: Vec<NamedRule>,
    user_rules: Vec<NamedRule>,
}

impl RuleCatalog {
    pub fn new(user_rules: &[GolUserRule]) -> (Self, Vec<String>) {
        let builtins = builtin_rules();
        let mut warnings = Vec::new();
        let mut ids: HashSet<String> = builtins
            .iter()
            .map(|rule| rule.id.to_ascii_lowercase())
            .collect();
        let mut users = Vec::new();
        for entry in user_rules {
            let id_lower = entry.id.to_ascii_lowercase();
            if ids.contains(&id_lower) {
                warnings.push(format!("Duplicate rule id '{}'; skipping", entry.id));
                continue;
            }
            let rule = match Rule::parse(&entry.rule) {
                Ok(rule) => rule,
                Err(err) => {
                    warnings.push(format!(
                        "Invalid rule string '{}' for id '{}': {}",
                        entry.rule, entry.id, err
                    ));
                    continue;
                }
            };
            ids.insert(id_lower);
            users.push(NamedRule {
                id: entry.id.clone(),
                name: entry.name.clone(),
                rule,
                description: entry.description.clone(),
            });
        }
        (
            Self {
                builtins,
                user_rules: users,
            },
            warnings,
        )
    }

    pub fn len(&self) -> usize {
        self.builtins.len() + self.user_rules.len()
    }

    pub fn builtins(&self) -> &[NamedRule] {
        &self.builtins
    }

    pub fn iter(&self) -> impl Iterator<Item = &NamedRule> {
        self.builtins.iter().chain(self.user_rules.iter())
    }

    pub fn get(&self, idx: usize) -> Option<&NamedRule> {
        if idx < self.builtins.len() {
            self.builtins.get(idx)
        } else {
            self.user_rules.get(idx - self.builtins.len())
        }
    }

    pub fn find_by_id(&self, id: &str) -> Option<&NamedRule> {
        self.iter().find(|rule| rule.id.eq_ignore_ascii_case(id))
    }

    pub fn find_by_rule(&self, rule: Rule) -> Option<&NamedRule> {
        self.iter().find(|entry| entry.rule == rule)
    }

    pub fn index_of_selected(&self, selected: &SelectedRule) -> Option<usize> {
        if let Some(id) = &selected.id {
            return self
                .iter()
                .position(|rule| rule.id.eq_ignore_ascii_case(id));
        }
        self.iter().position(|rule| rule.rule == selected.rule)
    }

    pub fn filter_indices(&self, query: &str) -> Vec<usize> {
        let needle = query.trim().to_ascii_lowercase();
        if needle.is_empty() {
            return (0..self.len()).collect();
        }
        self.iter()
            .enumerate()
            .filter_map(|(idx, rule)| {
                let rule_text = rule.rule.to_string();
                let hay = format!(
                    "{} {} {} {}",
                    rule.id, rule.name, rule_text, rule.description
                )
                .to_ascii_lowercase();
                if hay.contains(&needle) {
                    Some(idx)
                } else {
                    None
                }
            })
            .collect()
    }

    pub fn select(&self, selector: &str) -> Result<SelectedRule, RuleSelectError> {
        let trimmed = selector.trim();
        if trimmed.is_empty() {
            return Err(RuleSelectError::UnknownId(selector.to_string()));
        }
        if let Some(named) = self.find_by_id(trimmed) {
            return Ok(SelectedRule::from_named(named));
        }
        let rule = Rule::parse(trimmed).map_err(RuleSelectError::Parse)?;
        let mut selected = SelectedRule::from_rule(rule);
        if let Some(named) = self.find_by_rule(rule) {
            selected.id = Some(named.id.clone());
            selected.name = Some(named.name.clone());
        }
        Ok(selected)
    }

    pub fn label_for_rule(&self, rule: Rule) -> String {
        match self.find_by_rule(rule) {
            Some(named) => format!("{} ({})", rule, named.name),
            None => rule.to_string(),
        }
    }
}

impl Default for RuleCatalog {
    fn default() -> Self {
        RuleCatalog::new(&[]).0
    }
}

#[derive(Debug)]
pub enum RuleSelectError {
    UnknownId(String),
    Parse(RuleParseError),
}

impl std::fmt::Display for RuleSelectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RuleSelectError::UnknownId(value) => {
                write!(f, "unknown rule id '{}'", value)
            }
            RuleSelectError::Parse(err) => write!(f, "{err}"),
        }
    }
}

impl std::error::Error for RuleSelectError {}

fn builtin_rules() -> Vec<NamedRule> {
    const BUILTINS: &[(&str, &str, &str, &str)] = &[
        (
            "conway",
            "Conway's Life",
            "B3/S23",
            "Classic Life rule.",
        ),
        ("highlife", "HighLife", "B36/S23", "Replicators and explosive growth."),
        ("seeds", "Seeds", "B2/S", "No survival; only births."),
        ("life34", "34 Life", "B34/S34", "Life with 3/4 births and survives."),
        (
            "diamoeba",
            "Diamoeba",
            "B35678/S5678",
            "Blobby, amoeba-like growth.",
        ),
        (
            "daynight",
            "Day & Night",
            "B3678/S34678",
            "Symmetric day/night behavior.",
        ),
        ("morley", "Morley", "B368/S245", "High complexity patterns."),
        (
            "replicator",
            "Replicator",
            "B1357/S1357",
            "Self-replicating patterns.",
        ),
        (
            "labyrinth",
            "Labyrinth",
            "B3/S12345",
            "Maze-like corridors.",
        ),
        (
            "anneal",
            "Anneal",
            "B4678/S35678",
            "Annealing behavior and stability.",
        ),
        ("serviettes", "Serviettes", "B234/S", "Expanding lace patterns."),
    ];
    BUILTINS
        .iter()
        .map(|(id, name, rule_text, desc)| {
            let rule = Rule::parse(rule_text).expect("builtin rule parse");
            NamedRule::new(id, name, rule, desc)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtins_parse_and_unique_ids() {
        let (catalog, warnings) = RuleCatalog::new(&[]);
        assert!(warnings.is_empty());
        let mut ids = HashSet::new();
        for rule in catalog.iter() {
            assert!(ids.insert(rule.id.to_ascii_lowercase()));
            let text = rule.rule.to_string();
            let parsed = Rule::parse(&text).expect("builtin rule parse");
            assert_eq!(parsed, rule.rule);
        }
    }

    #[test]
    fn select_by_id_and_rule() {
        let (catalog, _) = RuleCatalog::new(&[]);
        let by_id = catalog.select("highlife").expect("select id");
        assert_eq!(by_id.rule.to_string(), "B36/S23");
        assert_eq!(by_id.name.as_deref(), Some("HighLife"));

        let by_rule = catalog.select("B3/S23").expect("select rule");
        assert_eq!(by_rule.id.as_deref(), Some("conway"));
    }
}
