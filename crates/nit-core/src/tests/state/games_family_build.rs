//! `:games run fsm` family-build path: queueing the override, persisting/
//! using the disk cache, and the CPU-cap force-bypass behaviour.

use super::*;

const FSM_BASE_CONFIG: &str = r#"
schema_version = 1
game = "ipd"
rounds = 10
repetitions = 1
noise = 0.0

[[strategy]]
id = "fsm_allc"
type = "fsm"
num_states = 1
outputs = ["C"]
transitions = [[0, 0]]
"#;

#[test]
fn command_games_run_fsm_family_queues_generated_override() {
    let root = temp_dir("cmd-games-run-fsm-family");
    let mut state = AppState::new(
        root.clone(),
        Buffer::from_str("x", FSM_BASE_CONFIG, None),
        Buffer::empty("n", None),
    );
    state.app_kind = AppKind::Games;
    assert!(!handle_command_line(&mut state, ":games run fsm {2, 2}"));
    assert!(!state.games.pending_run);
    assert!(state.games.family_building);
    let request = state
        .games
        .pending_family_run
        .as_ref()
        .expect("expected queued family request");
    let override_run = build_family_run_override_for_request(
        &state.workspace_root,
        &state.editor_buffer().content_as_string(),
        request,
    )
    .expect("expected generated override");
    let expected = nit_games::unique_fsm_behavior_representatives(2, 2)
        .expect("notebook behavior representatives")
        .len();
    assert_eq!(override_run.config.strategies.len(), expected);
    assert_eq!(override_run.config.strategies.len(), 22);
    assert!(override_run
        .config
        .strategies
        .iter()
        .all(|spec| matches!(spec.kind, nit_games::config::StrategySpecKind::Fsm { .. })));
}

#[test]
fn fsm_family_build_persists_disk_cache() {
    let root = temp_dir("fsm-family-disk-cache");
    let request = GamesFamilyRunRequest {
        family: "fsm".into(),
        input: "{2, 2}".into(),
        force: false,
    };
    let override_run = build_family_run_override_for_request(&root, FSM_BASE_CONFIG, &request)
        .expect("expected generated override");
    assert_eq!(override_run.config.strategies.len(), 22);

    let cache_path = fsm_family_cache_path(&root, 2, 2, nit_games::FsmGroupingMode::Wnbm);
    assert!(cache_path.exists(), "expected cache file at {cache_path:?}");

    let cached: FsmFamilyCacheEntry =
        serde_json::from_slice(&fs::read(&cache_path).expect("cache file"))
            .expect("valid cache json");
    assert_eq!(cached.states, 2);
    assert_eq!(cached.actions, 2);
    assert_eq!(cached.grouping_mode, nit_games::FsmGroupingMode::Wnbm);
    assert_eq!(cached.canonical_count, 49);
    assert_eq!(cached.representative_indices.len(), 22);
}

#[test]
fn fsm_family_build_uses_persisted_disk_cache_when_present() {
    let root = temp_dir("fsm-family-disk-cache-hit");
    let cache_path = fsm_family_cache_path(&root, 1, 2, nit_games::FsmGroupingMode::Wnbm);
    fs::create_dir_all(
        cache_path
            .parent()
            .expect("expected parent cache directory"),
    )
    .expect("create cache dir");
    fs::write(
        &cache_path,
        serde_json::to_vec(&FsmFamilyCacheEntry {
            schema_version: FSM_FAMILY_CACHE_SCHEMA_VERSION,
            states: 1,
            actions: 2,
            grouping_mode: nit_games::FsmGroupingMode::Wnbm,
            canonical_count: 2,
            representative_indices: vec![1, 0],
        })
        .expect("encode cache entry"),
    )
    .expect("write cache entry");

    let request = GamesFamilyRunRequest {
        family: "fsm".into(),
        input: "{1, 2}".into(),
        force: false,
    };
    let override_run = build_family_run_override_for_request(&root, FSM_BASE_CONFIG, &request)
        .expect("expected generated override from cache");
    let ids = override_run
        .config
        .strategies
        .iter()
        .map(|spec| spec.id.as_str())
        .collect::<Vec<_>>();
    assert_eq!(ids, vec!["fsm_1", "fsm_0"]);
}

#[test]
fn command_games_run_fsm_family_accepts_named_tuple_keys() {
    let root = temp_dir("cmd-games-run-fsm-family-named");
    let mut state = AppState::new(
        root.clone(),
        Buffer::from_str("x", FSM_BASE_CONFIG, None),
        Buffer::empty("n", None),
    );
    state.app_kind = AppKind::Games;
    assert!(!handle_command_line(
        &mut state,
        ":games run fsm {s=2, k=2}"
    ));
    assert!(!state.games.pending_run);
    assert!(state.games.pending_family_run.is_some());
}

#[test]
fn command_games_run_fsm_family_placeholder_tuple_shows_hint() {
    let root = temp_dir("cmd-games-run-fsm-family-placeholder");
    let mut state = AppState::new(
        root.clone(),
        Buffer::from_str("x", FSM_BASE_CONFIG, None),
        Buffer::empty("n", None),
    );
    state.app_kind = AppKind::Games;
    assert!(!handle_command_line(&mut state, ":games run fsm {s, k}"));
    assert!(!state.games.pending_run);
    let status = state.status.clone().unwrap_or_default();
    assert!(status.contains("placeholders need numeric values"));
}

#[test]
fn command_games_run_fsm_family_cap_blocks_without_force() {
    // CPU-only: caps are enforced. Metal/Auto would bypass them.
    let cpu_capped_config = r#"
schema_version = 1
game = "ipd"
rounds = 10
repetitions = 1
noise = 0.0

[engine]
fsm_grouping = "moorem"
accelerator = "cpu"

[[strategy]]
id = "fsm_allc"
type = "fsm"
num_states = 1
outputs = ["C"]
transitions = [[0, 0]]
"#;
    let root = temp_dir("cmd-games-run-fsm-family-cap");
    let mut state = AppState::new(
        root.clone(),
        Buffer::from_str("x", cpu_capped_config, None),
        Buffer::empty("n", None),
    );
    state.app_kind = AppKind::Games;
    assert!(!handle_command_line(&mut state, ":games run fsm {3, 2}"));
    assert!(!state.games.pending_run);
    assert!(state.games.pending_family_run.is_some());
    let request = state
        .games
        .pending_family_run
        .as_ref()
        .expect("expected queued family request");
    let err = build_family_run_override_for_request(
        &state.workspace_root,
        &state.editor_buffer().content_as_string(),
        request,
    )
    .expect_err("expected cap error");
    assert!(err.contains("too large for CPU"));
}

#[test]
fn command_games_run_fsm_family_cap_force_bypasses_limits() {
    let auto_grouping_config = r#"
schema_version = 1
game = "ipd"
rounds = 10
repetitions = 1
noise = 0.0

[engine]
fsm_grouping = "moorem"

[[strategy]]
id = "fsm_allc"
type = "fsm"
num_states = 1
outputs = ["C"]
transitions = [[0, 0]]
"#;
    let root = temp_dir("cmd-games-run-fsm-family-cap-force");
    let mut state = AppState::new(
        root.clone(),
        Buffer::from_str("x", auto_grouping_config, None),
        Buffer::empty("n", None),
    );
    state.app_kind = AppKind::Games;
    assert!(!handle_command_line(
        &mut state,
        ":games run force fsm {3, 2}"
    ));
    assert!(!state.games.pending_run);
    assert!(state.games.pending_family_run.is_some());
    let request = state
        .games
        .pending_family_run
        .as_ref()
        .expect("expected queued family request");
    let override_run = build_family_run_override_for_request(
        &state.workspace_root,
        &state.editor_buffer().content_as_string(),
        request,
    )
    .expect("expected generated override");
    assert!(!override_run.config.strategies.is_empty());
}
