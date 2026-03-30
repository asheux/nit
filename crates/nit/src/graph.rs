use std::io;

use nit_games::{
    Action, InputMode, StrategyIntrospection, StrategyIntrospectionKind,
    StrategyIntrospectionParameters, TmTransitionRecord,
};
use serde::Serialize;

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

pub(crate) fn build_strategy_graph(intro: &StrategyIntrospection) -> anyhow::Result<StrategyGraph> {
    match &intro.parameters {
        StrategyIntrospectionParameters::Fsm {
            states,
            start_state,
            outputs,
            transitions,
            index,
            ..
        } => Ok(build_fsm_graph(
            intro.id.clone(),
            intro.kind.clone(),
            *states,
            *start_state,
            outputs,
            transitions,
            index.map(|value| vec![format!("notebook_index={value}")]),
        )),
        StrategyIntrospectionParameters::Ca { n, k, r, t } => Ok(build_ca_graph(
            intro.id.clone(),
            intro.kind.clone(),
            *n,
            *k,
            *r,
            *t,
        )),
        StrategyIntrospectionParameters::OneSidedTm {
            states,
            start_state,
            transitions,
            ..
        } => Ok(build_tm_graph(
            intro.id.clone(),
            intro.kind.clone(),
            *states,
            *start_state,
            transitions,
        )),
    }
}

fn build_fsm_graph(
    strategy_id: String,
    kind: StrategyIntrospectionKind,
    states: usize,
    start_state: usize,
    outputs: &[Action],
    transitions: &[Vec<usize>],
    notes: Option<Vec<String>>,
) -> StrategyGraph {
    let mut nodes = Vec::new();
    for idx in 0..states {
        let output = outputs.get(idx).map(|a| a.as_char()).unwrap_or('?');
        nodes.push(GraphNode {
            id: (idx + 1).to_string(),
            label: format!("{}({output})", idx + 1),
        });
    }
    let mut edges = Vec::new();
    for (state_idx, row) in transitions.iter().enumerate() {
        for (input_idx, next) in row.iter().enumerate() {
            let label = input_idx.to_string();
            edges.push(GraphEdge {
                from: (state_idx + 1).to_string(),
                to: (next + 1).to_string(),
                color: edge_color_for_label(&label),
                label,
            });
        }
    }
    StrategyGraph {
        directed: true,
        strategy_id,
        kind,
        input_mode: Some(InputMode::OpponentLastAction),
        start_state: Some((start_state + 1).to_string()),
        nodes,
        edges,
        notes,
    }
}

fn build_tm_graph(
    strategy_id: String,
    kind: StrategyIntrospectionKind,
    states: u16,
    start_state: u16,
    transitions: &[TmTransitionRecord],
) -> StrategyGraph {
    let mut nodes = Vec::new();
    for state in 1..=states {
        nodes.push(GraphNode {
            id: state.to_string(),
            label: state.to_string(),
        });
    }
    let mut edges = Vec::new();
    let mut uses_halt = false;
    for trans in transitions {
        let label = trans.write.to_string();
        let to_id = if trans.next == 0 {
            uses_halt = true;
            "HALT".to_string()
        } else {
            trans.next.to_string()
        };
        edges.push(GraphEdge {
            from: trans.state.to_string(),
            to: to_id,
            color: edge_color_for_label(&label),
            label,
        });
    }
    if uses_halt {
        nodes.push(GraphNode {
            id: "HALT".to_string(),
            label: "HALT".to_string(),
        });
    }
    StrategyGraph {
        directed: true,
        strategy_id,
        kind,
        input_mode: None,
        start_state: Some(start_state.to_string()),
        nodes,
        edges,
        notes: Some(vec![
            "edges labeled by write symbol (ap)".to_string(),
            "read/move not shown".to_string(),
        ]),
    }
}

fn build_ca_graph(
    strategy_id: String,
    kind: StrategyIntrospectionKind,
    n: u64,
    k: u8,
    r: f32,
    t: u32,
) -> StrategyGraph {
    StrategyGraph {
        directed: true,
        strategy_id,
        kind,
        input_mode: None,
        start_state: None,
        nodes: Vec::new(),
        edges: Vec::new(),
        notes: Some(vec![format!(
            "CA rule tuple {{n={n}, k={k}, r={r}}}, steps={t}"
        )]),
    }
}

fn edge_color_for_label(label: &str) -> Option<String> {
    match label {
        "0" => Some("#e74c3c".to_string()),
        "1" => Some("#2ecc71".to_string()),
        "2" => Some("#3498db".to_string()),
        "3" => Some("#9b59b6".to_string()),
        _ => None,
    }
}

pub(crate) fn render_strategy_graph_dot(graph: &StrategyGraph) -> String {
    let mut dot = String::new();
    dot.push_str("digraph strategy {\n");
    dot.push_str("  rankdir=LR;\n");
    dot.push_str("  node [shape=box];\n");
    if let Some(start) = &graph.start_state {
        dot.push_str("  start [shape=point];\n");
        let start_id = dot_id(start);
        dot.push_str(&format!("  start -> {start_id};\n"));
    }
    for node in &graph.nodes {
        let label = node.label.replace('"', "\\\"");
        let node_id = dot_id(&node.id);
        dot.push_str(&format!("  {node_id} [label=\"{label}\"];\n"));
    }
    for edge in &graph.edges {
        let label = edge.label.replace('"', "\\\"");
        let mut attrs = vec![format!("label=\"{label}\"")];
        if let Some(color) = &edge.color {
            attrs.push(format!("color=\"{color}\""));
            attrs.push(format!("fontcolor=\"{color}\""));
        }
        let from = dot_id(&edge.from);
        let to = dot_id(&edge.to);
        let attrs_joined = attrs.join(", ");
        dot.push_str(&format!("  {from} -> {to} [{attrs_joined}];\n"));
    }
    dot.push_str("}\n");
    dot
}

fn dot_id(raw: &str) -> String {
    let escaped = raw.replace('"', "\\\"");
    format!("\"{escaped}\"")
}

pub(crate) fn write_strategy_graph_json(
    out_path: &std::path::Path,
    graph: &StrategyGraph,
) -> anyhow::Result<()> {
    use anyhow::Context;
    nit_utils::fs::write_atomic(out_path, |writer| {
        serde_json::to_writer_pretty(writer, graph).map_err(io::Error::other)
    })
    .with_context(|| format!("failed to write {}", out_path.display()))
}
