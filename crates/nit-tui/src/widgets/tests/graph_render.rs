use super::*;

#[test]
fn port_offset_pattern() {
    assert_eq!(port_offset(0), 0);
    assert_eq!(port_offset(1), -1);
    assert_eq!(port_offset(2), 1);
    assert_eq!(port_offset(3), -2);
    assert_eq!(port_offset(4), 2);
}

#[test]
fn wire_char_basic() {
    assert_eq!(wire_char_from_mask(BIT_L | BIT_R), '-');
    assert_eq!(wire_char_from_mask(BIT_U | BIT_D), '|');
    assert_eq!(wire_char_from_mask(BIT_U | BIT_R), '+');
}

#[test]
fn layers_are_deterministic() {
    let g = GraphSpec {
        nodes: vec![
            GraphNode {
                id: 0,
                label: "0".into(),
                output: None,
            },
            GraphNode {
                id: 1,
                label: "1".into(),
                output: None,
            },
            GraphNode {
                id: 2,
                label: "2".into(),
                output: None,
            },
        ],
        edges: vec![
            GraphEdge {
                from: 0,
                to: 1,
                label: "a".into(),
            },
            GraphEdge {
                from: 1,
                to: 2,
                label: "b".into(),
            },
        ],
        start: Some(0),
        flow: GraphFlow::TopToBottom,
        show_edge_labels: true,
    };
    let a = assign_layers(&g);
    let b = assign_layers(&g);
    assert_eq!(a, b);
    assert_eq!(a.get(&0), Some(&0));
    assert_eq!(a.get(&1), Some(&1));
    assert_eq!(a.get(&2), Some(&2));
}

#[test]
fn route_path_prefers_straight_line() {
    let width = 20;
    let height = 7;
    let blocked = vec![false; (width as usize) * (height as usize)];
    let used = vec![0u8; (width as usize) * (height as usize)];
    let path = route_path(
        (0, 3),
        (19, 3),
        width,
        height,
        &blocked,
        &used,
        FlowDir::LeftToRight,
    )
    .unwrap();
    assert_eq!(path.first().copied(), Some((0, 3)));
    assert_eq!(path.last().copied(), Some((19, 3)));
    // Expect a direct run.
    assert_eq!(path.len() as u16, 20);
}

#[test]
fn arrow_char_uses_directional_glyphs() {
    assert_eq!(arrow_char((1, 1), (2, 1)), '→');
    assert_eq!(arrow_char((2, 1), (1, 1)), '←');
    assert_eq!(arrow_char((1, 1), (1, 2)), '↓');
    assert_eq!(arrow_char((1, 2), (1, 1)), '↑');
}

#[test]
fn fsm_flow_renders_rightward_arrowheads() {
    let graph = GraphSpec {
        nodes: vec![
            GraphNode {
                id: 0,
                label: "1".into(),
                output: Some('C'),
            },
            GraphNode {
                id: 1,
                label: "2".into(),
                output: Some('D'),
            },
        ],
        edges: vec![GraphEdge {
            from: 0,
            to: 1,
            label: "0".into(),
        }],
        start: Some(0),
        flow: GraphFlow::LeftToRight,
        show_edge_labels: false,
    };
    let cached = compute_circuit_graph(
        &graph,
        Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 16,
        },
        0,
    );
    assert_eq!(cached.message, None);
    assert_eq!(cached.arrows.len(), 1);
    assert_eq!(cached.arrows[0].ch, '→');
}
