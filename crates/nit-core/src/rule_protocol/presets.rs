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

struct PhaseSpec {
    lookup: &'static RuleLookup,
    label: &'static str,
    steps: u32,
}

struct LoopedDuoSpec {
    id: &'static str,
    name: &'static str,
    description: &'static str,
    first: PhaseSpec,
    second: PhaseSpec,
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
        first: PhaseSpec {
            lookup: &HIGHLIFE,
            label: "Incubate",
            steps: 16,
        },
        second: PhaseSpec {
            lookup: &CONWAY,
            label: "Stabilize",
            steps: 256,
        },
    },
    LoopedDuoSpec {
        id: "anneal_set",
        name: "Anneal & Set",
        description: "Vote 1 then Conway 31 (loop)",
        first: PhaseSpec {
            lookup: &VOTE,
            label: "Anneal",
            steps: 1,
        },
        second: PhaseSpec {
            lookup: &CONWAY,
            label: "Set",
            steps: 31,
        },
    },
    LoopedDuoSpec {
        id: "coral_growth",
        name: "Coral Growth",
        description: "Coral 32 then Conway 128 (loop)",
        first: PhaseSpec {
            lookup: &CORAL,
            label: "Grow",
            steps: 32,
        },
        second: PhaseSpec {
            lookup: &CONWAY,
            label: "Settle",
            steps: 128,
        },
    },
    LoopedDuoSpec {
        id: "chaos_injection",
        name: "Chaos Injection",
        description: "Seeds 4 then Conway 128 (loop)",
        first: PhaseSpec {
            lookup: &SEEDS,
            label: "Inject",
            steps: 4,
        },
        second: PhaseSpec {
            lookup: &CONWAY,
            label: "Recover",
            steps: 128,
        },
    },
];

pub fn builtin_protocols(catalog: &RuleCatalog) -> Vec<ProtocolPreset> {
    let mut out = vec![ProtocolPreset {
        id: "fixed_conway".into(),
        name: "Fixed Conway".into(),
        description: "Single-rule Conway Life".into(),
        mode: RuleMode::Fixed(resolve_lookup(catalog, &CONWAY)),
    }];
    out.extend(
        LOOPED_DUOS
            .iter()
            .map(|spec| looped_two_phase(catalog, spec)),
    );
    out
}

fn looped_two_phase(catalog: &RuleCatalog, spec: &LoopedDuoSpec) -> ProtocolPreset {
    let phases = vec![
        RulePhase {
            rule: resolve_lookup(catalog, spec.first.lookup),
            steps: spec.first.steps,
            label: Some(spec.first.label.into()),
        },
        RulePhase {
            rule: resolve_lookup(catalog, spec.second.lookup),
            steps: spec.second.steps,
            label: Some(spec.second.label.into()),
        },
    ];
    ProtocolPreset {
        id: spec.id.into(),
        name: spec.name.into(),
        description: spec.description.into(),
        mode: RuleMode::Protocol(RuleProtocol::new(phases, true).expect("preset protocol")),
    }
}

fn resolve_lookup(catalog: &RuleCatalog, lookup: &RuleLookup) -> RuleRef {
    if let Some(named) = catalog.find_by_id(lookup.id) {
        return RuleRef::from_catalog(named);
    }
    RuleRef {
        id: None,
        rule: Rule::parse(lookup.fallback_rulestring).unwrap_or_else(|_| Rule::conway()),
        name: lookup.fallback_name.map(|s| s.into()),
    }
}
