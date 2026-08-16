use serde::{Deserialize, Serialize};

use crate::node::NodeId;

/// How a node was reached from its parent. `Backtrack` is emitted only in Phase 2,
/// when the frontier re-selects a held node; `Turn`/`Fork`/`Merge` cover Phase 0/1.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum EdgeKind {
    Turn,
    Fork,
    Merge,
    Backtrack,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Edge {
    pub from: NodeId,
    pub to: NodeId,
    pub kind: EdgeKind,
}
