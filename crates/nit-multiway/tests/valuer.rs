//! Phase 1 value-function acceptance: a better-structured candidate edit scores
//! strictly higher than a worse one, and a gate-failing (`gated`) node is
//! non-viable and excluded from expansion regardless of its genome score.
//!
//! The fixtures are run through the real `nit_core::compute_genome_report`; the
//! ordering is deterministic and was chosen so the gap is wide (Spaceship vs
//! Still Life), not marginal. Both exceed the 20-significant-line floor below
//! which the genome auto-passes and the ordering would be undefined.

use std::path::Path;

use nit_core::{compute_genome_report, GenomeTier, SubstrateState};
use nit_multiway::node::{CommitRef, Node, NodeStatus};
use nit_multiway::search::is_expandable;
use nit_multiway::valuer::node_value;
use nit_multiway::Value;

/// Idiomatic enum + `match` state machine: varied AST, shallow nesting.
const BETTER: &str = r#"
pub enum State {
    Idle,
    Running,
    Paused,
    Stopped,
}

pub struct Machine {
    state: State,
    ticks: u32,
}

impl Machine {
    pub fn new() -> Self {
        Self { state: State::Idle, ticks: 0 }
    }

    pub fn step(&mut self) {
        self.ticks += 1;
        self.state = match self.state {
            State::Idle => State::Running,
            State::Running if self.ticks > 5 => State::Paused,
            State::Running => State::Running,
            State::Paused => State::Stopped,
            State::Stopped => State::Stopped,
        };
    }

    pub fn is_active(&self) -> bool {
        matches!(self.state, State::Running | State::Paused)
    }

    pub fn label(&self) -> &'static str {
        match self.state {
            State::Idle => "idle",
            State::Running => "running",
            State::Paused => "paused",
            State::Stopped => "stopped",
        }
    }
}
"#;

/// The opposite: one function buried under a deeply nested if/else pyramid.
const WORSE: &str = r#"
fn process(a: i64, b: i64, c: i64, d: i64) -> i64 {
    let mut x = 0;
    if a > 0 {
        if b > 0 {
            if c > 0 {
                if d > 0 {
                    x = a + b + c + d;
                } else {
                    x = a + b + c;
                }
            } else {
                if d > 0 {
                    x = a + b + d;
                } else {
                    x = a + b;
                }
            }
        } else {
            if c > 0 {
                x = a + c;
            } else {
                x = a;
            }
        }
    } else {
        x = 0;
    }
    x
}
"#;

#[test]
fn better_edit_scores_strictly_higher() {
    let path = Path::new("candidate.rs");
    let (better, better_tier) = node_value(&[compute_genome_report(BETTER, path)]);
    let (worse, worse_tier) = node_value(&[compute_genome_report(WORSE, path)]);

    assert!(
        better > worse,
        "better edit must score higher: better={better} ({better_tier:?}), worse={worse} ({worse_tier:?})"
    );
    assert!(better_tier >= worse_tier);
}

#[test]
fn gate_failing_node_is_non_viable_and_excluded() {
    // A failed gate marks the node non-viable even when its genome score is the
    // top of the range — gates are hard, not weighed into the score.
    let broken = Value {
        gated: true,
        score: 0.95,
        tier: GenomeTier::Replicator,
    };
    assert!(!broken.is_viable());
    assert!(!is_expandable(&open_node(broken)));

    // A viable node with a far lower score is still expandable.
    let clean = Value {
        gated: false,
        score: 0.20,
        tier: GenomeTier::Spaceship,
    };
    assert!(clean.is_viable());
    assert!(is_expandable(&open_node(clean)));
}

fn open_node(value: Value) -> Node {
    Node {
        commit: CommitRef("seed".into()),
        value: Some(value),
        status: NodeStatus::Open,
        parents: Vec::new(),
        substrate: SubstrateState::default(),
        summary: String::new(),
        changed_paths: Vec::new(),
    }
}
