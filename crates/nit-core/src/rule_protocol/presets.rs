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

struct LoopedDuo {
    id: &'static str,
    name: &'static str,
    description: &'static str,
    first: PhaseSpec,
    second: PhaseSpec,
}

struct PhaseSpec {
    rule: RuleRef,
    label: &'static str,
    steps: u32,
}

pub fn builtin_protocols(catalog: &RuleCatalog) -> Vec<ProtocolPreset> {
    let conway = fallback_rule_ref(catalog, "conway", "B3/S23", None);
    let highlife = fallback_rule_ref(catalog, "highlife", "B36/S23", Some("HighLife"));
    let vote = fallback_rule_ref(catalog, "vote", "B5678/S45678", Some("Vote"));
    let coral = fallback_rule_ref(catalog, "coral", "B3/S45678", Some("Coral"));
    let seeds = fallback_rule_ref(catalog, "seeds", "B2/S", Some("Seeds"));

    let duos = [
        LoopedDuo {
            id: "incubate_replicators",
            name: "Incubate Replicators",
            description: "HighLife 16 then Conway 256 (loop)",
            first: PhaseSpec {
                rule: highlife,
                label: "Incubate",
                steps: 16,
            },
            second: PhaseSpec {
                rule: conway.clone(),
                label: "Stabilize",
                steps: 256,
            },
        },
        LoopedDuo {
            id: "anneal_set",
            name: "Anneal & Set",
            description: "Vote 1 then Conway 31 (loop)",
            first: PhaseSpec {
                rule: vote,
                label: "Anneal",
                steps: 1,
            },
            second: PhaseSpec {
                rule: conway.clone(),
                label: "Set",
                steps: 31,
            },
        },
        LoopedDuo {
            id: "coral_growth",
            name: "Coral Growth",
            description: "Coral 32 then Conway 128 (loop)",
            first: PhaseSpec {
                rule: coral,
                label: "Grow",
                steps: 32,
            },
            second: PhaseSpec {
                rule: conway.clone(),
                label: "Settle",
                steps: 128,
            },
        },
        LoopedDuo {
            id: "chaos_injection",
            name: "Chaos Injection",
            description: "Seeds 4 then Conway 128 (loop)",
            first: PhaseSpec {
                rule: seeds,
                label: "Inject",
                steps: 4,
            },
            second: PhaseSpec {
                rule: conway.clone(),
                label: "Recover",
                steps: 128,
            },
        },
    ];

    let mut out = vec![ProtocolPreset {
        id: "fixed_conway".into(),
        name: "Fixed Conway".into(),
        description: "Single-rule Conway Life".into(),
        mode: RuleMode::Fixed(conway),
    }];
    out.extend(duos.into_iter().map(looped_two_phase));
    out
}

fn looped_two_phase(spec: LoopedDuo) -> ProtocolPreset {
    let phases = vec![
        RulePhase {
            rule: spec.first.rule,
            steps: spec.first.steps,
            label: Some(spec.first.label.into()),
        },
        RulePhase {
            rule: spec.second.rule,
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

fn fallback_rule_ref(
    catalog: &RuleCatalog,
    lookup_id: &str,
    fallback_rulestring: &str,
    fallback_name: Option<&str>,
) -> RuleRef {
    if let Some(named) = catalog.find_by_id(lookup_id) {
        return RuleRef::from_catalog(named);
    }
    RuleRef {
        id: None,
        rule: Rule::parse(fallback_rulestring).unwrap_or_else(|_| Rule::conway()),
        name: fallback_name.map(|s| s.into()),
    }
}
