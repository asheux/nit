use super::*;
use nit_core::substrate::{Signal, SignalKind, SignalTarget, SubstrateState};

fn mk_state_with_signals(signals: Vec<Signal>) -> AppState {
    use nit_core::buffer::Buffer;
    let root = std::env::temp_dir().join(format!(
        "nit-signals-view-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).unwrap();
    let mut state = AppState::new(root, Buffer::empty("x", None), Buffer::empty("n", None));
    let mut substrate = SubstrateState::default();
    for s in signals {
        substrate.emit_signal(s);
    }
    state.substrate = substrate;
    state
}

fn mk_signal(id: &str, kind: SignalKind, initial: f32, posted_at: u64) -> Signal {
    Signal {
        id: id.into(),
        kind,
        posted_by: "agent-a".into(),
        posted_at_gen: posted_at,
        target: SignalTarget::Global,
        initial_strength: initial,
        payload: serde_json::Value::Null,
    }
}

#[test]
fn build_lines_empty_has_header_and_hint() {
    let state = mk_state_with_signals(vec![]);
    let theme = Theme::default();
    let lines = build_lines(&state, &theme, 100);
    // summary + blank + column header + blank + empty hint = 5 lines
    assert_eq!(lines.len(), 5);
}

#[test]
fn build_lines_with_two_signals_emits_rows() {
    let signals = vec![
        mk_signal("s1", SignalKind::Warning, 0.9, 0),
        mk_signal("s2", SignalKind::Lead, 0.4, 0),
    ];
    let state = mk_state_with_signals(signals);
    let theme = Theme::default();
    let lines = build_lines(&state, &theme, 100);
    // summary + blank + column header + 2 rows = 5 lines
    assert_eq!(lines.len(), 5);
}
