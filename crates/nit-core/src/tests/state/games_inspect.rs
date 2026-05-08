//! `:games inspect` and `:games history` command behaviours — strategy
//! definition generation, CA simulator wiring, and the match-history popup.

use super::*;

#[test]
fn command_games_inspect_numeric_generates_fsm_rule() {
    let root = temp_dir("cmd-games-inspect-fsm-numeric");
    let mut state = AppState::new(
        root.clone(),
        Buffer::empty("x", None),
        Buffer::empty("n", None),
    );
    state.app_kind = AppKind::Games;
    assert!(!handle_command_line(&mut state, ":games inspect 22"));
    let def = state
        .games
        .strategy_inspect
        .definition
        .as_ref()
        .expect("generated definition");
    match &def.kind {
        nit_games::config::StrategySpecKind::Fsm {
            num_states, index, ..
        } => {
            assert_eq!(*num_states, 2);
            assert_eq!(*index, Some(22));
        }
        other => panic!("expected FSM kind, got {other:?}"),
    }
}

#[test]
fn command_games_inspect_fsm_tuple_uses_fsm_override() {
    let root = temp_dir("cmd-games-inspect-fsm-tuple");
    let mut state = AppState::new(
        root.clone(),
        Buffer::empty("x", None),
        Buffer::empty("n", None),
    );
    state.app_kind = AppKind::Games;
    assert!(!handle_command_line(
        &mut state,
        ":games inspect fsm {22, 3, 2}"
    ));
    let def = state
        .games
        .strategy_inspect
        .definition
        .as_ref()
        .expect("generated definition");
    assert_eq!(def.id, "fsm");
    match &def.kind {
        nit_games::config::StrategySpecKind::Fsm {
            num_states,
            transitions,
            index,
            ..
        } => {
            assert_eq!(*num_states, 3);
            assert_eq!(*index, Some(22));
            assert_eq!(transitions.len(), 3);
            assert!(transitions.iter().all(|row| row.len() == 2));
        }
        other => panic!("expected FSM kind, got {other:?}"),
    }
}

#[test]
fn command_games_inspect_tm_tuple_still_generates_tm() {
    let root = temp_dir("cmd-games-inspect-tm-tuple");
    let mut state = AppState::new(
        root.clone(),
        Buffer::empty("x", None),
        Buffer::empty("n", None),
    );
    state.app_kind = AppKind::Games;
    assert!(!handle_command_line(
        &mut state,
        ":games inspect tm_rule {3111, 2, 2}"
    ));
    let def = state
        .games
        .strategy_inspect
        .definition
        .as_ref()
        .expect("generated definition");
    match &def.kind {
        nit_games::config::StrategySpecKind::OneSidedTm {
            states,
            symbols,
            rule_code,
            ..
        } => {
            assert_eq!(*states, 2);
            assert_eq!(*symbols, 2);
            assert_eq!(*rule_code, Some(3111));
        }
        other => panic!("expected TM kind, got {other:?}"),
    }
}

#[test]
fn command_games_ca_tuple_opens_ca_simulator() {
    let root = temp_dir("cmd-games-ca-tuple");
    let mut state = AppState::new(
        root.clone(),
        Buffer::empty("x", None),
        Buffer::empty("n", None),
    );
    state.app_kind = AppKind::Games;
    assert!(!handle_command_line(
        &mut state,
        ":games ca {30, 2, 1, 5} 13 7"
    ));
    assert!(state.games.ca_sim.open);
    assert_eq!(state.games.ca_sim.input, Some(13));
    assert_eq!(state.games.ca_sim.steps_override, Some(7));
    let def = state
        .games
        .ca_sim
        .definition
        .as_ref()
        .expect("generated definition");
    match &def.kind {
        nit_games::config::StrategySpecKind::Ca { n, k, r, t } => {
            assert_eq!(*n, 30);
            assert_eq!(*k, 2);
            assert!((*r - 1.0).abs() < f32::EPSILON);
            assert_eq!(*t, 5);
        }
        other => panic!("expected CA kind, got {other:?}"),
    }
}

#[test]
fn command_games_ca_tuple_without_t_uses_default_ten() {
    let root = temp_dir("cmd-games-ca-tuple-default-t");
    let mut state = AppState::new(
        root.clone(),
        Buffer::empty("x", None),
        Buffer::empty("n", None),
    );
    state.app_kind = AppKind::Games;
    assert!(!handle_command_line(&mut state, ":games ca {30, 2, 1} 13"));
    let def = state
        .games
        .ca_sim
        .definition
        .as_ref()
        .expect("generated definition");
    match &def.kind {
        nit_games::config::StrategySpecKind::Ca { t, .. } => {
            assert_eq!(*t, 10);
        }
        other => panic!("expected CA kind, got {other:?}"),
    }
}

#[test]
fn command_games_ca_config_selects_strategy_by_id() {
    let root = temp_dir("cmd-games-ca-config");
    let config = r#"
schema_version = 1
game = "ipd"
rounds = 10
repetitions = 1
noise = 0.0

[[strategy]]
id = "ca_rule"
type = "ca"
n = 30
k = 2
r = 1
t = 4
"#;
    let mut state = AppState::new(
        root.clone(),
        Buffer::from_str("x", config, None),
        Buffer::empty("n", None),
    );
    state.app_kind = AppKind::Games;
    assert!(!handle_command_line(
        &mut state,
        ":games ca config 9 3 ca_rule"
    ));
    assert!(state.games.ca_sim.open);
    assert_eq!(state.games.ca_sim.input, Some(9));
    assert_eq!(state.games.ca_sim.steps_override, Some(3));
    let def = state
        .games
        .ca_sim
        .definition
        .as_ref()
        .expect("selected definition");
    assert_eq!(def.id, "ca_rule");
    assert!(matches!(
        def.kind,
        nit_games::config::StrategySpecKind::Ca { .. }
    ));
}

#[test]
fn command_games_history_opens_popup_with_completed_matches() {
    let root = temp_dir("cmd-games-history");
    let mut state = AppState::new(
        root.clone(),
        Buffer::empty("x", None),
        Buffer::empty("n", None),
    );
    state.app_kind = AppKind::Games;
    state
        .games
        .match_history
        .entries
        .push(nit_games::MatchHistoryPreview {
            match_index: 1,
            total_matches: 1,
            a: "fsm_allc".into(),
            b: "fsm_alld".into(),
            rounds_total: 4,
            outcomes: "0123".into(),
        });
    assert!(!handle_command_line(&mut state, ":games history"));
    assert!(state.games.match_history.open);
    assert!(state.games.match_history.last_error.is_none());
}

#[test]
fn command_history_alias_opens_popup_with_completed_matches() {
    let root = temp_dir("cmd-history-alias");
    let mut state = AppState::new(
        root.clone(),
        Buffer::empty("x", None),
        Buffer::empty("n", None),
    );
    state.app_kind = AppKind::Games;
    state
        .games
        .match_history
        .entries
        .push(nit_games::MatchHistoryPreview {
            match_index: 1,
            total_matches: 1,
            a: "fsm_allc".into(),
            b: "fsm_alld".into(),
            rounds_total: 4,
            outcomes: "0123".into(),
        });
    assert!(!handle_command_line(&mut state, ":history"));
    assert!(state.games.match_history.open);
    assert!(state.games.match_history.last_error.is_none());
}

#[test]
fn command_games_history_avoids_empty_error_when_capture_is_disabled() {
    let root = temp_dir("cmd-games-history-disabled");
    let mut state = AppState::new(
        root.clone(),
        Buffer::empty("x", None),
        Buffer::empty("n", None),
    );
    state.app_kind = AppKind::Games;
    state.games.match_history.capture_disabled_for_run = true;

    assert!(!handle_command_line(&mut state, ":games history"));
    assert!(state.games.match_history.open);
    assert!(state.games.match_history.last_error.is_none());
}
