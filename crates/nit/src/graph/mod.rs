use nit_games::{InputMode, StrategyIntrospectionKind};
use serde::Serialize;

mod build;
mod render;

pub(crate) use build::build_strategy_graph;
pub(crate) use render::{render_strategy_graph_dot, write_strategy_graph_json};

#[derive(Serialize)]
pub(crate) struct GraphNode {
    id: String,
    label: String,
}

#[derive(Serialize)]
pub(crate) struct GraphEdge {
    from: String,
    to: String,
    label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    color: Option<String>,
}

#[derive(Serialize)]
pub(crate) struct StrategyGraph {
    directed: bool,
    pub(crate) strategy_id: String,
    kind: StrategyIntrospectionKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    input_mode: Option<InputMode>,
    #[serde(skip_serializing_if = "Option::is_none")]
    start_state: Option<String>,
    nodes: Vec<GraphNode>,
    edges: Vec<GraphEdge>,
    #[serde(skip_serializing_if = "Option::is_none")]
    notes: Option<Vec<String>>,
}
