use super::*;

#[test]
fn fsm_graph_spec_builds_edges() {
    let def = StrategyDefinition {
        id: "t".to_string(),
        name: None,
        kind: StrategySpecKind::Fsm {
            num_states: 2,
            start_state: 0,
            outputs: vec![Action::Cooperate, Action::Defect],
            input_mode: Some(InputMode::OpponentLastAction),
            transitions: vec![vec![0, 1], vec![1, 0]],
            index: None,
        },
        rng_seed_a: None,
        rng_seed_b: None,
    };

    let graph = graph_from_definition(&def).expect("graph");
    assert_eq!(graph.nodes.len(), 2);
    assert_eq!(graph.edges.len(), 4);
    assert_eq!(graph.start, Some(0));
    assert_eq!(graph.flow, GraphFlow::LeftToRight);
    assert!(!graph.show_edge_labels);
}

#[test]
fn fsm_graph_spec_filters_unreachable_states() {
    let def = StrategyDefinition {
        id: "t".to_string(),
        name: None,
        kind: StrategySpecKind::Fsm {
            num_states: 3,
            start_state: 0,
            outputs: vec![Action::Cooperate, Action::Defect, Action::Cooperate],
            input_mode: Some(InputMode::OpponentLastAction),
            transitions: vec![vec![0, 1], vec![1, 0], vec![2, 2]],
            index: None,
        },
        rng_seed_a: None,
        rng_seed_b: None,
    };

    let graph = graph_from_definition(&def).expect("graph");
    assert_eq!(graph.nodes.len(), 2);
    assert!(graph.nodes.iter().all(|node| node.id != 2));
    assert!(graph
        .edges
        .iter()
        .all(|edge| edge.from != 2 && edge.to != 2));
}

#[test]
fn scrolled_strategy_detail_matches_window_of_full_document() {
    let mut state = AppState::new(
        std::env::temp_dir(),
        nit_core::Buffer::empty("x", None),
        nit_core::Buffer::empty("n", None),
    );
    state.games.strategy_inspect.lines = (0..120).map(|i| format!("step {i}")).collect();
    let theme = Theme::default();
    let full = build_lines(&state, &theme, 60);
    let (details, _) = split_graph_sections(&state.games.strategy_inspect.lines);
    for start in [0, 1, 47, 116] {
        let window = build_lines_window(&state, &theme, 60, start, 8, &details);
        assert_eq!(
            window,
            full.iter().skip(start).take(8).cloned().collect::<Vec<_>>()
        );
    }
}
