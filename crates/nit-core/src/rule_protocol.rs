use nit_gol::Rule;
use nit_utils::hashing::stable_hash_bytes;

use crate::gol_rules::{RuleCatalog, SelectedRule};

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

    pub fn from_catalog(rule: &crate::gol_rules::NamedRule) -> Self {
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

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct RuleProtocol {
    pub phases: Vec<RulePhase>,
    pub looped: bool,
    pub phase_idx: usize,
    pub step_in_phase: u32,
}

impl RuleProtocol {
    pub fn new(phases: Vec<RulePhase>, looped: bool) -> Result<Self, String> {
        if phases.is_empty() {
            return Err("protocol must have at least one phase".into());
        }
        if let Some(pos) = phases.iter().position(|phase| phase.steps == 0) {
            return Err(format!("phase {} has invalid steps=0", pos + 1));
        }
        Ok(Self {
            phases,
            looped,
            phase_idx: 0,
            step_in_phase: 0,
        })
    }

    pub fn current_rule(&self) -> &RuleRef {
        &self.phases[self.phase_idx].rule
    }

    pub fn current_phase(&self) -> &RulePhase {
        &self.phases[self.phase_idx]
    }

    pub fn phase_count(&self) -> usize {
        self.phases.len()
    }

    pub fn advance_one_gen(&mut self) {
        if self.phases.is_empty() {
            return;
        }
        let steps = self.current_phase().steps.max(1);
        let next_step = self.step_in_phase.saturating_add(1);
        if next_step < steps {
            self.step_in_phase = next_step;
            return;
        }
        if self.phase_idx + 1 < self.phases.len() {
            self.phase_idx += 1;
            self.step_in_phase = 0;
            return;
        }
        if self.looped {
            self.phase_idx = 0;
            self.step_in_phase = 0;
        } else {
            self.step_in_phase = steps.saturating_sub(1);
        }
    }

    pub fn reset(&mut self) {
        self.phase_idx = 0;
        self.step_in_phase = 0;
    }

    pub fn canonical_string(&self) -> String {
        let mut out = String::new();
        for (idx, phase) in self.phases.iter().enumerate() {
            if idx > 0 {
                out.push('>');
            }
            out.push_str(&phase.rule.selector());
            out.push('*');
            out.push_str(&phase.steps.to_string());
        }
        if self.looped {
            out.push_str("(loop)");
        }
        out
    }

    pub fn hash(&self) -> u64 {
        stable_hash_bytes(self.canonical_string().as_bytes())
    }

    pub fn cycle_steps(&self) -> u32 {
        self.phases.iter().map(|phase| phase.steps).sum()
    }
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub enum RuleMode {
    Fixed(RuleRef),
    Protocol(RuleProtocol),
}

impl RuleMode {
    pub fn current_rule(&self) -> &RuleRef {
        match self {
            RuleMode::Fixed(rule) => rule,
            RuleMode::Protocol(protocol) => protocol.current_rule(),
        }
    }

    pub fn advance_one_gen(&mut self) {
        if let RuleMode::Protocol(protocol) = self {
            protocol.advance_one_gen();
        }
    }

    pub fn reset(&mut self) {
        if let RuleMode::Protocol(protocol) = self {
            protocol.reset();
        }
    }

    pub fn canonical_string(&self) -> String {
        match self {
            RuleMode::Fixed(rule) => rule.selector(),
            RuleMode::Protocol(protocol) => protocol.canonical_string(),
        }
    }

    pub fn protocol(&self) -> Option<&RuleProtocol> {
        match self {
            RuleMode::Protocol(protocol) => Some(protocol),
            _ => None,
        }
    }

    pub fn protocol_mut(&mut self) -> Option<&mut RuleProtocol> {
        match self {
            RuleMode::Protocol(protocol) => Some(protocol),
            _ => None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct ProtocolPreset {
    pub id: String,
    pub name: String,
    pub description: String,
    pub mode: RuleMode,
}

pub fn builtin_protocols(catalog: &RuleCatalog) -> Vec<ProtocolPreset> {
    let conway = rule_ref_from_id(catalog, "conway").unwrap_or_else(|| RuleRef {
        id: None,
        rule: Rule::conway(),
        name: None,
    });
    let highlife = rule_ref_from_id(catalog, "highlife").unwrap_or_else(|| RuleRef {
        id: None,
        rule: Rule::parse("B36/S23").unwrap_or_else(|_| Rule::conway()),
        name: Some("HighLife".into()),
    });
    let vote = rule_ref_from_id(catalog, "vote").unwrap_or_else(|| RuleRef {
        id: None,
        rule: Rule::parse("B5678/S45678").unwrap_or_else(|_| Rule::conway()),
        name: Some("Vote".into()),
    });
    let coral = rule_ref_from_id(catalog, "coral").unwrap_or_else(|| RuleRef {
        id: None,
        rule: Rule::parse("B3/S45678").unwrap_or_else(|_| Rule::conway()),
        name: Some("Coral".into()),
    });
    let seeds = rule_ref_from_id(catalog, "seeds").unwrap_or_else(|| RuleRef {
        id: None,
        rule: Rule::parse("B2/S").unwrap_or_else(|_| Rule::conway()),
        name: Some("Seeds".into()),
    });

    vec![
        ProtocolPreset {
            id: "fixed_conway".into(),
            name: "Fixed Conway".into(),
            description: "Single-rule Conway Life".into(),
            mode: RuleMode::Fixed(conway.clone()),
        },
        ProtocolPreset {
            id: "incubate_replicators".into(),
            name: "Incubate Replicators".into(),
            description: "HighLife 16 then Conway 256 (loop)".into(),
            mode: RuleMode::Protocol(
                RuleProtocol::new(
                    vec![
                        RulePhase {
                            rule: highlife.clone(),
                            steps: 16,
                            label: Some("Incubate".into()),
                        },
                        RulePhase {
                            rule: conway.clone(),
                            steps: 256,
                            label: Some("Stabilize".into()),
                        },
                    ],
                    true,
                )
                .expect("preset protocol"),
            ),
        },
        ProtocolPreset {
            id: "anneal_set".into(),
            name: "Anneal & Set".into(),
            description: "Vote 1 then Conway 31 (loop)".into(),
            mode: RuleMode::Protocol(
                RuleProtocol::new(
                    vec![
                        RulePhase {
                            rule: vote.clone(),
                            steps: 1,
                            label: Some("Anneal".into()),
                        },
                        RulePhase {
                            rule: conway.clone(),
                            steps: 31,
                            label: Some("Set".into()),
                        },
                    ],
                    true,
                )
                .expect("preset protocol"),
            ),
        },
        ProtocolPreset {
            id: "coral_growth".into(),
            name: "Coral Growth".into(),
            description: "Coral 32 then Conway 128 (loop)".into(),
            mode: RuleMode::Protocol(
                RuleProtocol::new(
                    vec![
                        RulePhase {
                            rule: coral.clone(),
                            steps: 32,
                            label: Some("Grow".into()),
                        },
                        RulePhase {
                            rule: conway.clone(),
                            steps: 128,
                            label: Some("Settle".into()),
                        },
                    ],
                    true,
                )
                .expect("preset protocol"),
            ),
        },
        ProtocolPreset {
            id: "chaos_injection".into(),
            name: "Chaos Injection".into(),
            description: "Seeds 4 then Conway 128 (loop)".into(),
            mode: RuleMode::Protocol(
                RuleProtocol::new(
                    vec![
                        RulePhase {
                            rule: seeds.clone(),
                            steps: 4,
                            label: Some("Inject".into()),
                        },
                        RulePhase {
                            rule: conway,
                            steps: 128,
                            label: Some("Recover".into()),
                        },
                    ],
                    true,
                )
                .expect("preset protocol"),
            ),
        },
    ]
}

pub fn parse_protocol_spec(spec: &str, catalog: &RuleCatalog) -> Result<RuleProtocol, String> {
    let trimmed = spec.trim();
    if trimmed.is_empty() {
        return Err("protocol spec is empty".into());
    }
    let mut looped = false;
    let mut cleaned = trimmed.to_string();
    let lowered = cleaned.to_ascii_lowercase();
    if lowered.ends_with("(loop)") {
        looped = true;
        let idx = cleaned.len().saturating_sub(6);
        cleaned.truncate(idx);
    } else if lowered.ends_with(" loop") {
        looped = true;
        let idx = cleaned.len().saturating_sub(5);
        cleaned.truncate(idx);
    } else if lowered.ends_with("loop") && !lowered.ends_with("/sloop") {
        looped = true;
        let idx = cleaned.len().saturating_sub(4);
        cleaned.truncate(idx);
    }
    let mut phases = Vec::new();
    for (idx, raw) in cleaned.split('>').enumerate() {
        let part = raw.trim();
        if part.is_empty() {
            return Err(format!("phase {} is empty", idx + 1));
        }
        let (rule_text, steps) = if let Some((left, right)) = part.split_once('*') {
            let steps_text = right.trim();
            let steps = steps_text
                .parse::<u32>()
                .map_err(|_| format!("invalid steps '{}' in phase {}", steps_text, idx + 1))?;
            (left.trim(), steps)
        } else {
            (part, 1)
        };
        if steps == 0 {
            return Err(format!("phase {} has steps=0", idx + 1));
        }
        let selected = catalog
            .select(rule_text)
            .map_err(|err| format!("invalid rule '{}' in phase {}: {}", rule_text, idx + 1, err))?;
        let rule_ref = RuleRef::from_selected(&selected);
        phases.push(RulePhase {
            rule: rule_ref,
            steps,
            label: None,
        });
    }
    RuleProtocol::new(phases, looped)
}

fn rule_ref_from_id(catalog: &RuleCatalog, id: &str) -> Option<RuleRef> {
    catalog.find_by_id(id).map(RuleRef::from_catalog)
}

#[cfg(test)]
#[path = "tests/rule_protocol.rs"]
mod tests;
