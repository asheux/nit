//! Built-in protocol catalogue.
//!
//! Preset ids and shapes are part of the public surface — saved selections
//! from older runs use these ids to round-trip back into a `RuleMode`, so
//! renaming an entry breaks restore. Each looped duo pairs a "disturbance"
//! phase with a "recovery" Conway phase to demonstrate phase sequencing.

use nit_gol::Rule;

use crate::gol_rules::RuleCatalog;

use super::types::{RuleMode, RulePhase, RuleProtocol, RuleRef};

#[derive(Clone, Debug)]
pub struct ProtocolPreset {
    pub id: String,
    pub name: String,
    pub description: String,
    pub mode: RuleMode,
}

struct RuleLookup {
    id: &'static str,
    fallback_rulestring: &'static str,
    fallback_name: Option<&'static str>,
}

impl RuleLookup {
    fn resolve(&self, catalog: &RuleCatalog) -> RuleRef {
        if let Some(named) = catalog.find_by_id(self.id) {
            return RuleRef::from_catalog(named);
        }
        RuleRef {
            id: None,
            rule: Rule::parse(self.fallback_rulestring).unwrap_or_else(|_| Rule::conway()),
            name: self.fallback_name.map(|s| s.into()),
        }
    }
}

struct PhaseSpec {
    lookup: &'static RuleLookup,
    label: &'static str,
    steps: u32,
}

struct LoopedDuoSpec {
    id: &'static str,
    name: &'static str,
    description: &'static str,
    phases: [PhaseSpec; 2],
}

const CONWAY: RuleLookup = RuleLookup {
    id: "conway",
    fallback_rulestring: "B3/S23",
    fallback_name: None,
};
const HIGHLIFE: RuleLookup = RuleLookup {
    id: "highlife",
    fallback_rulestring: "B36/S23",
    fallback_name: Some("HighLife"),
};
const VOTE: RuleLookup = RuleLookup {
    id: "vote",
    fallback_rulestring: "B5678/S45678",
    fallback_name: Some("Vote"),
};
const CORAL: RuleLookup = RuleLookup {
    id: "coral",
    fallback_rulestring: "B3/S45678",
    fallback_name: Some("Coral"),
};
const SEEDS: RuleLookup = RuleLookup {
    id: "seeds",
    fallback_rulestring: "B2/S",
    fallback_name: Some("Seeds"),
};

const LOOPED_DUOS: &[LoopedDuoSpec] = &[
    LoopedDuoSpec {
        id: "incubate_replicators",
        name: "Incubate Replicators",
        description: "HighLife 16 then Conway 256 (loop)",
        phases: [
            PhaseSpec {
                lookup: &HIGHLIFE,
                label: "Incubate",
                steps: 16,
            },
            PhaseSpec {
                lookup: &CONWAY,
                label: "Stabilize",
                steps: 256,
            },
        ],
    },
    LoopedDuoSpec {
        id: "anneal_set",
        name: "Anneal & Set",
        description: "Vote 1 then Conway 31 (loop)",
        phases: [
            PhaseSpec {
                lookup: &VOTE,
                label: "Anneal",
                steps: 1,
            },
            PhaseSpec {
                lookup: &CONWAY,
                label: "Set",
                steps: 31,
            },
        ],
    },
    LoopedDuoSpec {
        id: "coral_growth",
        name: "Coral Growth",
        description: "Coral 32 then Conway 128 (loop)",
        phases: [
            PhaseSpec {
                lookup: &CORAL,
                label: "Grow",
                steps: 32,
            },
            PhaseSpec {
                lookup: &CONWAY,
                label: "Settle",
                steps: 128,
            },
        ],
    },
    LoopedDuoSpec {
        id: "chaos_injection",
        name: "Chaos Injection",
        description: "Seeds 4 then Conway 128 (loop)",
        phases: [
            PhaseSpec {
                lookup: &SEEDS,
                label: "Inject",
                steps: 4,
            },
            PhaseSpec {
                lookup: &CONWAY,
                label: "Recover",
                steps: 128,
            },
        ],
    },
];

pub fn builtin_protocols(catalog: &RuleCatalog) -> Vec<ProtocolPreset> {
    let mut out = vec![ProtocolPreset {
        id: "fixed_conway".into(),
        name: "Fixed Conway".into(),
        description: "Single-rule Conway Life".into(),
        mode: RuleMode::Fixed(CONWAY.resolve(catalog)),
    }];
    out.extend(
        LOOPED_DUOS
            .iter()
            .map(|spec| build_looped_duo(catalog, spec)),
    );
    out
}

fn build_looped_duo(catalog: &RuleCatalog, spec: &LoopedDuoSpec) -> ProtocolPreset {
    let phases: Vec<RulePhase> = spec
        .phases
        .iter()
        .map(|p| RulePhase {
            rule: p.lookup.resolve(catalog),
            steps: p.steps,
            label: Some(p.label.into()),
        })
        .collect();
    ProtocolPreset {
        id: spec.id.into(),
        name: spec.name.into(),
        description: spec.description.into(),
        mode: RuleMode::Protocol(RuleProtocol::new(phases, true).expect("preset protocol")),
    }
}
