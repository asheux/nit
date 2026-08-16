use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::{self, BufReader, Write};
use std::path::Path;

use nit_core::GenomeTier;
use serde::{Deserialize, Serialize};

use crate::edge::Edge;
use crate::node::{Node, NodeId, NodeStatus};

/// Persisted search history as a DAG. Nodes are keyed in a `BTreeMap` so serde
/// emits them in a deterministic order (the round-trip test compares re-serialized
/// bytes); `edges` is the source of truth for children, each node's own `parents`
/// the reverse direction.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Graph {
    nodes: BTreeMap<NodeId, Node>,
    edges: Vec<Edge>,
}

impl Graph {
    pub fn add_node(&mut self, id: NodeId, node: Node) {
        self.nodes.insert(id, node);
    }

    pub fn add_edge(&mut self, edge: Edge) {
        self.edges.push(edge);
    }

    pub fn node(&self, id: &NodeId) -> Option<&Node> {
        self.nodes.get(id)
    }

    pub fn node_mut(&mut self, id: &NodeId) -> Option<&mut Node> {
        self.nodes.get_mut(id)
    }

    pub fn children(&self, id: &NodeId) -> Vec<NodeId> {
        self.edges
            .iter()
            .filter(|edge| &edge.from == id)
            .map(|edge| edge.to.clone())
            .collect()
    }

    pub fn parents(&self, id: &NodeId) -> &[NodeId] {
        match self.nodes.get(id) {
            Some(node) => &node.parents,
            None => &[],
        }
    }

    /// Every transitive ancestor of `id`, walking the `parents` reverse-index. The
    /// search engine needs this to keep merges acyclic (a node must never merge with
    /// one of its own ancestors) and to choose a backtrack target along a held
    /// node's lineage. The visited set both deduplicates shared ancestors in a
    /// diamond and guards against following an accidental cycle forever.
    pub fn ancestors(&self, id: &NodeId) -> BTreeSet<NodeId> {
        let mut reached = BTreeSet::new();
        let mut pending: Vec<NodeId> = self.parents(id).to_vec();
        while let Some(ancestor) = pending.pop() {
            if reached.insert(ancestor.clone()) {
                pending.extend(self.parents(&ancestor).iter().cloned());
            }
        }
        reached
    }

    pub fn iter_nodes(&self) -> impl Iterator<Item = (&NodeId, &Node)> {
        self.nodes.iter()
    }

    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// Persist the DAG as JSON, creating any missing parent directories. The write
    /// is atomic (temp file + rename), so a crash mid-write leaves the previous DAG
    /// intact rather than a truncated one. A non-finite score makes `serde_json` fail
    /// here — surfaced as an `Err`, never a silent loss — but the valuer coerces NaN
    /// away, so real graphs serialize cleanly.
    pub fn save(&self, path: &Path) -> io::Result<()> {
        if let Some(parent) = path.parent().filter(|dir| !dir.as_os_str().is_empty()) {
            fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_vec_pretty(self).map_err(io::Error::other)?;
        nit_utils::fs::write_atomic(path, |file| file.write_all(&json))
    }

    pub fn load(path: &Path) -> io::Result<Self> {
        let reader = BufReader::new(File::open(path)?);
        serde_json::from_reader(reader).map_err(io::Error::other)
    }

    /// Emit the DAG as a self-contained Graphviz DOT document: one statement per
    /// node (a tier-ramp fill, a status shape, and a label of short-id · tier ·
    /// score · the turn's first summary line · file count) and one per edge (typed
    /// `Turn`/`Fork`/`Merge`/`Backtrack`), with the best-score lineage and every
    /// gate-failed node marked. Pure — no I/O and no process spawn; a caller pipes
    /// the string to `dot -Tpng` itself. Every attribute string is escaped because
    /// `summary` is untrusted agent text that could otherwise break the DOT.
    pub fn to_dot(&self) -> String {
        let kept = self.kept_path_nodes();
        let mut out =
            String::from("digraph multiway {\n  rankdir=TB;\n  node [fontname=\"monospace\"];\n");
        for (id, node) in &self.nodes {
            out.push_str(&dot_node_stmt(id, node, kept.contains(id)));
        }
        for edge in &self.edges {
            let on_path = kept.contains(&edge.from) && kept.contains(&edge.to);
            let mut attrs = format!("label=\"{:?}\"", edge.kind);
            if on_path {
                attrs.push_str(", color=\"#d4af37\", penwidth=2");
            }
            out.push_str(&format!(
                "  \"{}\" -> \"{}\" [{}];\n",
                dot_escape(edge.from.as_str()),
                dot_escape(edge.to.as_str()),
                attrs,
            ));
        }
        out.push_str("}\n");
        out
    }

    /// The "kept path": from the highest-scored viable node, follow first parents (a
    /// merge's `parents[0]`) back to the root. v1 emits no `EdgeKind::Backtrack`, so
    /// status + best-score lineage — not edge kind — is the only sound way to recover
    /// the path the search settled on. The visited guard stops a malformed cycle from
    /// looping forever.
    fn kept_path_nodes(&self) -> BTreeSet<NodeId> {
        let mut cursor = self
            .nodes
            .iter()
            .filter_map(|(id, node)| {
                node.value
                    .filter(|v| !v.gated)
                    .map(|v| (id.clone(), v.score))
            })
            .max_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(id, _)| id);
        let mut path = BTreeSet::new();
        while let Some(id) = cursor {
            if !path.insert(id.clone()) {
                break;
            }
            cursor = self.parents(&id).first().cloned();
        }
        path
    }
}

/// One node statement. The label collapses to just the short id and summary for an
/// unvalued node (the root), and grows tier/score/file-count as the node is valued
/// and its turn recorded; `on_path` nodes gain a gold border.
fn dot_node_stmt(id: &NodeId, node: &Node, on_path: bool) -> String {
    let short: String = id.as_str().chars().take(7).collect();
    let mut label = dot_escape(&short);
    if let Some(value) = node.value.as_ref() {
        label.push_str(&format!(" · {} · {:.2}", value.tier.numeral(), value.score));
    }
    if let Some(first) = node.summary.lines().next().filter(|line| !line.is_empty()) {
        label.push_str("\\n");
        label.push_str(&dot_escape(first));
    }
    if !node.changed_paths.is_empty() {
        label.push_str(&format!("\\n({} files)", node.changed_paths.len()));
    }

    let fill = node.value.as_ref().map_or("#cccccc", |v| tier_fill(v.tier));
    // Gate-failed nodes dash their border on top of the tier fill so a pruned
    // branch reads as pruned at a glance, not just by its red ramp.
    let style = if node.status == NodeStatus::GateFailed {
        "filled,dashed"
    } else {
        "filled"
    };
    let mut attrs = format!(
        "label=\"{}\", fillcolor=\"{}\", shape={}, style=\"{}\"",
        label,
        fill,
        status_shape(node.status),
        style,
    );
    if on_path {
        attrs.push_str(", color=\"#d4af37\", penwidth=2");
    }
    format!("  \"{}\" [{}];\n", dot_escape(id.as_str()), attrs)
}

/// Escape a string for a DOT double-quoted attribute: agent summaries are
/// untrusted, so an unescaped `"` or `\` would corrupt — or inject into — the
/// document. Control characters (including stray newlines) collapse to a space so
/// a label never spills across lines.
fn dot_escape(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for ch in raw.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if c.is_control() => out.push(' '),
            c => out.push(c),
        }
    }
    out
}

/// A red→green fill ramp over the genome tiers, so a viewer reads value by colour.
fn tier_fill(tier: GenomeTier) -> &'static str {
    match tier {
        GenomeTier::StillLife => "#d64545",
        GenomeTier::Oscillator => "#e08c3b",
        GenomeTier::Spaceship => "#e0c93b",
        GenomeTier::Methuselah => "#8cc63f",
        GenomeTier::Replicator => "#2ea043",
    }
}

/// A distinct node shape per search status, so frontier/held/pruned/solution nodes
/// are distinguishable without colour alone.
fn status_shape(status: NodeStatus) -> &'static str {
    match status {
        NodeStatus::Open => "ellipse",
        NodeStatus::Expanded | NodeStatus::Dominated => "box",
        NodeStatus::GateFailed => "octagon",
        NodeStatus::Held => "diamond",
        NodeStatus::Solution => "doublecircle",
    }
}
