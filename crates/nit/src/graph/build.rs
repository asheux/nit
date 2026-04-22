use nit_games::{
    Action, InputMode, StrategyIntrospection, StrategyIntrospectionKind,
    StrategyIntrospectionParameters, TmTransitionRecord,
};

use super::{GraphEdge, GraphNode, StrategyGraph};

const EDGE_COLORS: [&str; 4] = ["#e74c3c", "#2ecc71", "#3498db", "#9b59b6"];

pub(crate) fn build_strategy_graph(intro: &StrategyIntrospection) -> StrategyGraph {
    let id = intro.id.clone();
    let kind = intro.kind.clone();
    match &intro.parameters {
        StrategyIntrospectionParameters::Fsm {
            states,
            start_state,
            outputs,
            transitions,
            index,
            ..
        } => build_fsm_graph(
            id,
            kind,
            *states,
            *start_state,
            outputs,
            transitions,
            index.map(|value| vec![format!("notebook_index={value}")]),
        ),
        StrategyIntrospectionParameters::Ca { n, k, r, t } => {
            build_ca_graph(id, kind, *n, *k, *r, *t)
        }
        StrategyIntrospectionParameters::OneSidedTm {
            states,
            start_state,
            transitions,
            ..
        } => build_tm_graph(id, kind, *states, *start_state, transitions),
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
    let nodes: Vec<GraphNode> = (0..states)
        .map(|idx| {
            let output = outputs.get(idx).map(|a| a.as_char()).unwrap_or('?');
            GraphNode {
                id: (idx + 1).to_string(),
                label: format!("{}({output})", idx + 1),
            }
        })
        .collect();
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
    let mut nodes: Vec<GraphNode> = (1..=states)
        .map(|state| GraphNode {
            id: state.to_string(),
            label: state.to_string(),
        })
        .collect();
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
    let idx = label.parse::<usize>().ok()?;
    EDGE_COLORS.get(idx).map(|c| c.to_string())
}
