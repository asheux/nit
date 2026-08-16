//! Search parameters for one engine run, plus the mood-driven decisions the search
//! loop makes against them: how wide to keep the frontier
//! ([`frontier_admission`]), when a node is good enough to stop early
//! ([`is_solution`]), and — once Phase 4 merging is enabled — which sibling pair to
//! recombine ([`merge_candidates`]).

use nit_core::GenomeTier;
use serde::{Deserialize, Serialize};

use crate::node::NodeId;
use crate::value::Value;

/// How the Phase 2 frontier trades breadth for depth: `Explore` keeps more
/// candidates alive, `Exploit` greedily follows the current best, `Balanced` sits
/// between. nit-multiway owns this enum rather than reusing any `nit_core` mood so
/// the search policy stays decoupled from unrelated runner semantics.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Mood {
    Explore,
    Balanced,
    Exploit,
}

/// Hard ceilings that stop a search. `max_tokens` is optional because not every
/// backend exposes a usable token meter.
#[derive(Copy, Clone, Debug, Serialize, Deserialize)]
pub struct Budget {
    pub max_nodes: usize,
    pub max_turns: usize,
    pub max_tokens: Option<u64>,
}

/// Tunable parameters threaded through a single engine run. `k` is the fork
/// fan-out attempted at each expansion step.
#[derive(Copy, Clone, Debug, Serialize, Deserialize)]
pub struct SearchPolicy {
    pub mood: Mood,
    pub k: usize,
    pub budget: Budget,
}

impl Default for SearchPolicy {
    fn default() -> Self {
        Self {
            mood: Mood::Balanced,
            k: 3,
            budget: Budget {
                max_nodes: 64,
                max_turns: 32,
                max_tokens: None,
            },
        }
    }
}

/// How many of an expansion's best-first-sorted viable children to admit to the
/// frontier; the rest are recorded as `Held` rather than pushed. This is the mood
/// knob: `Exploit` follows a single line, `Explore` keeps every viable child, and
/// `Balanced` admits the upper half. The result is clamped to at least one
/// whenever there is a viable child, so a productive expansion never strands all
/// of its children off the frontier.
pub fn frontier_admission(mood: Mood, viable: usize) -> usize {
    if viable == 0 {
        return 0;
    }
    let admit = match mood {
        Mood::Exploit => 1,
        Mood::Balanced => viable.div_ceil(2),
        Mood::Explore => viable,
    };
    admit.max(1)
}

/// Whether a node is an accepted terminal that ends the search early. v1 has no
/// configurable goal — run-to-budget is the norm — so only an un-gated node at the
/// top genome tier ([`GenomeTier::Replicator`]) short-circuits the loop.
pub fn is_solution(value: &Value) -> bool {
    !value.gated && value.tier >= GenomeTier::Replicator
}

/// Pick the two strongest sibling candidates to merge from one expansion's viable
/// children (sorted best-first), or `None` to skip — the common case. `Exploit`
/// never merges; `Balanced`/`Explore` recombine the top two. The pair are siblings
/// of one parent, so neither can be the other's ancestor: acyclicity holds with no
/// graph walk, which is why this takes only the viable slice, not the graph.
pub fn merge_candidates(mood: Mood, viable: &[(NodeId, f32)]) -> Option<(NodeId, NodeId)> {
    if mood == Mood::Exploit {
        return None;
    }
    match viable {
        [(a, _), (b, _), ..] => Some((a.clone(), b.clone())),
        _ => None,
    }
}
