use nit_gol::Rule;
use nit_utils::hashing::stable_hash_bytes;

use crate::gol_rules::SelectedRule;

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
