use std::fmt::Write as _;
use std::io;
use std::path::Path;

use anyhow::Context;

use super::StrategyGraph;

pub(crate) fn render_strategy_graph_dot(graph: &StrategyGraph) -> String {
    let mut dot = String::new();
    writeln!(dot, "digraph strategy {{").unwrap();
    writeln!(dot, "  rankdir=LR;").unwrap();
    writeln!(dot, "  node [shape=box];").unwrap();
    if let Some(start) = &graph.start_state {
        writeln!(dot, "  start [shape=point];").unwrap();
        writeln!(dot, "  start -> {};", dot_id(start)).unwrap();
    }
    for node in &graph.nodes {
        let label = node.label.replace('"', "\\\"");
        writeln!(dot, "  {} [label=\"{label}\"];", dot_id(&node.id)).unwrap();
    }
    for edge in &graph.edges {
        let label = edge.label.replace('"', "\\\"");
        let mut attrs = format!("label=\"{label}\"");
        if let Some(color) = &edge.color {
            write!(attrs, ", color=\"{color}\", fontcolor=\"{color}\"").unwrap();
        }
        writeln!(
            dot,
            "  {} -> {} [{attrs}];",
            dot_id(&edge.from),
            dot_id(&edge.to)
        )
        .unwrap();
    }
    writeln!(dot, "}}").unwrap();
    dot
}

fn dot_id(raw: &str) -> String {
    let escaped = raw.replace('"', "\\\"");
    format!("\"{escaped}\"")
}

pub(crate) fn write_strategy_graph_json(
    out_path: &Path,
    graph: &StrategyGraph,
) -> anyhow::Result<()> {
    nit_utils::fs::write_atomic(out_path, |writer| {
        serde_json::to_writer_pretty(writer, graph).map_err(io::Error::other)
    })
    .with_context(|| format!("failed to write {}", out_path.display()))
}
