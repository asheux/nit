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
        writeln!(
            dot,
            "  {} [label=\"{}\"];",
            dot_id(&node.id),
            escape_dot_str(&node.label)
        )
        .unwrap();
    }
    for edge in &graph.edges {
        let color_attrs = edge
            .color
            .as_deref()
            .map(|color| format!(", color=\"{color}\", fontcolor=\"{color}\""))
            .unwrap_or_default();
        writeln!(
            dot,
            "  {} -> {} [label=\"{}\"{color_attrs}];",
            dot_id(&edge.from),
            dot_id(&edge.to),
            escape_dot_str(&edge.label),
        )
        .unwrap();
    }
    writeln!(dot, "}}").unwrap();
    dot
}

fn escape_dot_str(s: &str) -> String {
    s.replace('"', "\\\"")
}

fn dot_id(raw: &str) -> String {
    format!("\"{}\"", escape_dot_str(raw))
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
