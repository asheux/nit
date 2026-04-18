use super::*;
use crate::buffer::Buffer;
use crate::rule_config::RulePersistence;
use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

fn temp_dir(label: &str) -> PathBuf {
    let mut dir = std::env::temp_dir();
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    dir.push(format!("nit-test-{label}-{now}-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn set_rule_by_id_updates_and_persists() {
    let root = temp_dir("rule-id");
    let config_path = root.join("config.toml");
    let mut state = AppState::new(
        root.clone(),
        Buffer::empty("x", None),
        Buffer::empty("n", None),
    );
    state.rule_persistence = RulePersistence {
        global_path: Some(config_path.clone()),
        workspace_path: None,
        workspace_override: false,
    };
    let named = state.rule_catalog.find_by_id("highlife").unwrap();
    let selected = SelectedRule::from_named(named);
    state.set_gol_rule(selected, true).unwrap();
    assert_eq!(state.visualizer.rule, "B36/S23");
    let contents = fs::read_to_string(config_path).unwrap();
    assert!(contents.contains("default = \"B36/S23\""));
}

#[test]
fn set_rule_by_string_updates_state() {
    let root = temp_dir("rule-str");
    let mut state = AppState::new(
        root.clone(),
        Buffer::empty("x", None),
        Buffer::empty("n", None),
    );
    let rule = Rule::parse("B36/S23").unwrap();
    let selected = SelectedRule::from_rule(rule);
    state.set_gol_rule(selected, false).unwrap();
    assert_eq!(state.visualizer.rule, "B36/S23");
}

#[test]
fn rule_picker_apply_sets_rule() {
    let root = temp_dir("rule-picker");
    let mut state = AppState::new(
        root.clone(),
        Buffer::empty("x", None),
        Buffer::empty("n", None),
    );
    state.rule_picker.open = true;
    state.rule_picker.query = "highlife".into();
    state.rule_picker.selected = 0;
    let _ = apply_action(&mut state, Action::ApplySelectedRuleFromPicker);
    assert_eq!(state.visualizer.rule, "B36/S23");
}

#[test]
fn command_q_quits_when_clean_and_prompts_when_dirty() {
    let root = temp_dir("cmd-q");
    let mut state = AppState::new(
        root.clone(),
        Buffer::empty("x", None),
        Buffer::empty("n", None),
    );
    assert!(!state.editor_buffer().is_dirty());
    assert!(handle_command_line(&mut state, "q"));

    // Mark dirty and ensure :q requests confirmation instead of immediate exit.
    state.editor_buffer_mut().insert_char('x');
    assert!(state.editor_buffer().is_dirty());
    assert!(!handle_command_line(&mut state, "q"));
    assert!(matches!(state.prompt, Some(Prompt::ConfirmQuit)));
}

#[test]
fn open_file_creates_new_editor_buffer_when_current_buffer_is_dirty() {
    let root = temp_dir("open-file-dirty");
    let file_a = root.join("a.txt");
    let file_b = root.join("b.txt");
    fs::write(&file_a, "alpha").unwrap();
    fs::write(&file_b, "beta").unwrap();

    let mut state = AppState::new(
        root.clone(),
        Buffer::from_str("a.txt", "alpha", Some(file_a.clone())),
        Buffer::empty("n", None),
    );
    state.editor_buffer_mut().insert_char('!');

    let outcome = apply_action(&mut state, Action::OpenFile(file_b.clone()));

    assert!(!outcome.should_exit);
    assert_eq!(state.buffers.len(), 3);
    assert_eq!(state.active_editor_buffer_id, 2);
    assert_eq!(state.editor_buffer().path(), Some(&file_b));
    assert_eq!(state.editor_buffer().content_as_string(), "beta");

    let original = state.buffer(0).expect("original editor buffer");
    assert_eq!(original.path(), Some(&file_a));
    assert!(original.is_dirty());
    assert_eq!(original.content_as_string(), "!alpha");
}

#[test]
fn open_file_switches_to_existing_dirty_buffer_instead_of_reloading() {
    let root = temp_dir("open-file-existing");
    let file_a = root.join("a.txt");
    let file_b = root.join("b.txt");
    fs::write(&file_a, "alpha").unwrap();
    fs::write(&file_b, "beta").unwrap();

    let mut state = AppState::new(
        root.clone(),
        Buffer::from_str("a.txt", "alpha", Some(file_a.clone())),
        Buffer::empty("n", None),
    );
    state.editor_buffer_mut().insert_char('!');
    let _ = apply_action(&mut state, Action::OpenFile(file_b.clone()));

    fs::write(&file_a, "disk copy changed").unwrap();
    let outcome = apply_action(&mut state, Action::OpenFile(file_a.clone()));

    assert!(!outcome.should_exit);
    assert_eq!(state.buffers.len(), 3);
    assert_eq!(state.active_editor_buffer_id, 0);
    assert_eq!(state.editor_buffer().path(), Some(&file_a));
    assert!(state.editor_buffer().is_dirty());
    assert_eq!(state.editor_buffer().content_as_string(), "!alpha");
}

#[test]
fn quit_prompts_when_hidden_editor_buffer_is_dirty() {
    let root = temp_dir("quit-hidden-dirty");
    let file_a = root.join("a.txt");
    let file_b = root.join("b.txt");
    fs::write(&file_a, "alpha").unwrap();
    fs::write(&file_b, "beta").unwrap();

    let mut state = AppState::new(
        root.clone(),
        Buffer::from_str("a.txt", "alpha", Some(file_a.clone())),
        Buffer::empty("n", None),
    );
    state.editor_buffer_mut().insert_char('!');
    let _ = apply_action(&mut state, Action::OpenFile(file_b));

    let outcome = apply_action(&mut state, Action::Quit);

    assert!(!outcome.should_exit);
    assert!(matches!(state.prompt, Some(Prompt::ConfirmQuit)));
}

#[test]
fn command_q_prompts_when_hidden_editor_buffer_is_dirty() {
    let root = temp_dir("cmd-q-hidden-dirty");
    let file_a = root.join("a.txt");
    let file_b = root.join("b.txt");
    fs::write(&file_a, "alpha").unwrap();
    fs::write(&file_b, "beta").unwrap();

    let mut state = AppState::new(
        root.clone(),
        Buffer::from_str("a.txt", "alpha", Some(file_a.clone())),
        Buffer::empty("n", None),
    );
    state.editor_buffer_mut().insert_char('!');
    let _ = apply_action(&mut state, Action::OpenFile(file_b));

    assert!(!handle_command_line(&mut state, "q"));
    assert!(matches!(state.prompt, Some(Prompt::ConfirmQuit)));
}

#[test]
fn command_help_dash_question_opens_help_popup() {
    let root = temp_dir("cmd-help-dash");
    let mut state = AppState::new(
        root.clone(),
        Buffer::empty("x", None),
        Buffer::empty("n", None),
    );
    assert!(!state.show_help);
    assert!(!handle_command_line(&mut state, "help - ?"));
    assert!(state.show_help);
    assert_eq!(state.help_scroll, 0);
}

#[test]
fn command_question_mark_opens_help_popup() {
    let root = temp_dir("cmd-help-qmark");
    let mut state = AppState::new(
        root.clone(),
        Buffer::empty("x", None),
        Buffer::empty("n", None),
    );
    assert!(!state.show_help);
    assert!(!handle_command_line(&mut state, "?"));
    assert!(state.show_help);
    assert_eq!(state.help_scroll, 0);
}

#[test]
fn command_colon_help_dash_question_opens_help_with_file_tree_open() {
    let root = temp_dir("cmd-help-colon-tree");
    let mut state = AppState::new(
        root.clone(),
        Buffer::empty("x", None),
        Buffer::empty("n", None),
    );
    state.file_tree.open = true;
    assert!(!state.show_help);
    assert!(!handle_command_line(&mut state, ":help - ?"));
    assert!(state.show_help);
    assert_eq!(state.help_scroll, 0);
}

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
fn command_games_run_fsm_family_queues_generated_override() {
    let root = temp_dir("cmd-games-run-fsm-family");
    let config = r#"
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
    let mut state = AppState::new(
        root.clone(),
        Buffer::from_str("x", config, None),
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
    let config = r#"
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
    let request = GamesFamilyRunRequest {
        family: "fsm".into(),
        input: "{2, 2}".into(),
        force: false,
    };
    let override_run = build_family_run_override_for_request(&root, config, &request)
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
    let config = r#"
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
    let override_run = build_family_run_override_for_request(&root, config, &request)
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
    let config = r#"
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
    let mut state = AppState::new(
        root.clone(),
        Buffer::from_str("x", config, None),
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
    let config = r#"
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
    let mut state = AppState::new(
        root.clone(),
        Buffer::from_str("x", config, None),
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
    let root = temp_dir("cmd-games-run-fsm-family-cap");
    // CPU-only: caps are enforced. Metal/Auto would bypass them.
    let config = r#"
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
    let mut state = AppState::new(
        root.clone(),
        Buffer::from_str("x", config, None),
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
    let status = err;
    assert!(status.contains("too large for CPU"));
}

#[test]
fn command_games_run_fsm_family_cap_force_bypasses_limits() {
    let root = temp_dir("cmd-games-run-fsm-family-cap-force");
    let config = r#"
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
    let mut state = AppState::new(
        root.clone(),
        Buffer::from_str("x", config, None),
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

#[test]
fn fsm_family_raw_count_can_exceed_legacy_machine_cap() {
    let raw_count = nit_games::fsm_count(4, 2).expect("expected finite raw FSM count");
    assert!(raw_count > MAX_FAMILY_RUN_MACHINES_CPU as u128);
}

#[test]
fn estimate_total_matches_matches_runtime_schedule_math() {
    for strategy_count in [1usize, 2, 3, 7] {
        for repetitions in [1u32, 2, 5] {
            for self_play in [false, true] {
                let estimated = estimate_total_matches(strategy_count, repetitions, self_play)
                    .expect("estimate should not overflow");

                let mut src = format!(
                    "schema_version = 1\ngame = \"ipd\"\nrounds = 2\nrepetitions = {repetitions}\nself_play = {self_play}\n\n"
                );
                for idx in 0..strategy_count {
                    src.push_str(&format!(
                        "[[strategy]]\nid = \"fsm_{idx}\"\ntype = \"fsm\"\nnum_states = 1\nstart_state = 0\noutputs = [\"C\"]\ntransitions = [[0, 0]]\n\n"
                    ));
                }

                let cfg = nit_games::config::GamesConfig::from_toml(&src)
                    .expect("runtime config should parse");
                let runtime = nit_games::TournamentKernel::new(cfg).total_matches() as u128;
                assert_eq!(
                    estimated, runtime,
                    "mismatch for strategy_count={strategy_count}, repetitions={repetitions}, self_play={self_play}"
                );
            }
        }
    }
}

#[test]
fn command_games_run_tm_family_queues_generated_override() {
    let root = temp_dir("cmd-games-run-tm-family");
    let config = r#"
schema_version = 1
game = "ipd"
rounds = 5
repetitions = 1
noise = 0.0

[[strategy]]
id = "tm_rule"
type = "tm"
states = 1
symbols = 2
start_state = 1
blank = 0
max_steps_per_round = 32
rule_code = 0
"#;
    let mut state = AppState::new(
        root.clone(),
        Buffer::from_str("x", config, None),
        Buffer::empty("n", None),
    );
    state.app_kind = AppKind::Games;
    assert!(!handle_command_line(&mut state, ":games run tm {1, 2}"));
    assert!(!state.games.pending_run);
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
    assert!(override_run.config.strategies.iter().all(|spec| matches!(
        spec.kind,
        nit_games::config::StrategySpecKind::OneSidedTm { .. }
    )));
    assert!(override_run.config.strategies.iter().all(|spec| matches!(
        spec.kind,
        nit_games::config::StrategySpecKind::OneSidedTm {
            max_steps_per_round: DEFAULT_FAMILY_TM_MAX_STEPS,
            ..
        }
    )));
    assert!(override_run.config.tm_filter_applied);
    assert!(matches!(
        override_run.config.engine.mode,
        nit_games::EngineMode::Batch
    ));
    assert!(override_run.config.engine.fast_eval);
    assert!(!override_run.config.event_log.enabled);
    assert!(!override_run.config.history.enabled);
}

#[test]
fn command_games_run_tm_family_reports_build_stage_timings() {
    let root = temp_dir("cmd-games-run-tm-family-timings");
    let config = r#"
schema_version = 1
game = "ipd"
rounds = 200
repetitions = 1
self_play = true
noise = 0.0

[[strategy]]
id = "tm_rule"
type = "tm"
states = 1
symbols = 2
start_state = 1
blank = 0
max_steps_per_round = 32
rule_code = 0
"#;
    let request = GamesFamilyRunRequest {
        family: "tm".into(),
        input: "{1, 2}".into(),
        force: false,
    };
    let (_override_run, timings) =
        build_family_run_override_for_request_with_timings(&root, config, &request)
            .expect("expected generated override with timings");
    println!(
        "tm_family_build timings: generation={:?} estimate={:?} normalize={:?} total={:?}",
        timings.generation_elapsed,
        timings.estimate_elapsed,
        timings.normalize_elapsed,
        timings.total_elapsed
    );
    if let Some(diagnostics) = timings.tm_filter.as_ref() {
        println!(
            "tm_family_build filter: backend={} decline={:?} error={:?}",
            diagnostics.backend.label(),
            diagnostics.metal_decline_reason,
            diagnostics.metal_error
        );
    }
    assert_eq!(timings.generated_strategies, 16);
    assert!(timings.tm_filter.is_some());
    assert!(timings.total_elapsed >= timings.generation_elapsed);
}

#[test]
fn command_games_run_tm_family_forces_fast_eval_in_auto_mode_before_tm_prep() {
    let root = temp_dir("cmd-games-run-tm-family-fast-eval-auto");
    let config = r#"
schema_version = 1
game = "ipd"
rounds = 30
repetitions = 1
noise = 0.0

[engine]
accelerator = "auto"
fast_eval = false

[[strategy]]
id = "tm_rule"
type = "tm"
states = 1
symbols = 2
start_state = 1
blank = 0
max_steps_per_round = 32
rule_code = 0
"#;
    let request = GamesFamilyRunRequest {
        family: "tm".into(),
        input: "{1, 2}".into(),
        force: false,
    };
    let (override_run, timings) =
        build_family_run_override_for_request_with_timings(&root, config, &request)
            .expect("family build should succeed");
    assert!(override_run.config.engine.fast_eval);
    let diagnostics = timings.tm_filter.expect("expected TM prep diagnostics");
    assert_eq!(
        diagnostics.requested_accelerator,
        nit_games::AcceleratorMode::Auto
    );
    assert!(diagnostics
        .metal_decline_reason
        .as_deref()
        .map(|reason| !reason.contains("fast_eval = false"))
        .unwrap_or(true));
}

#[test]
fn command_games_run_tm_family_reports_noise_fallback_reason() {
    let root = temp_dir("cmd-games-run-tm-family-noise-reason");
    let config = r#"
schema_version = 1
game = "ipd"
rounds = 30
repetitions = 1
noise = 0.1

[engine]
accelerator = "auto"
fast_eval = true

[[strategy]]
id = "tm_rule"
type = "tm"
states = 1
symbols = 2
start_state = 1
blank = 0
max_steps_per_round = 32
rule_code = 0
"#;
    let request = GamesFamilyRunRequest {
        family: "tm".into(),
        input: "{1, 2}".into(),
        force: false,
    };
    let (_override_run, timings) =
        build_family_run_override_for_request_with_timings(&root, config, &request)
            .expect("family build should succeed with CPU fallback");
    let diagnostics = timings.tm_filter.expect("expected TM prep diagnostics");
    assert!(matches!(
        diagnostics.backend,
        nit_games::TmHaltingFilterBackend::NotebookCpuFallback
    ));
    let reason = diagnostics.metal_decline_reason.unwrap_or_default();
    assert!(
        reason.contains("non-zero noise"),
        "unexpected fallback reason: {reason}"
    );
}

#[test]
fn command_games_run_tm_family_strict_metal_fails_loudly_on_noise() {
    let root = temp_dir("cmd-games-run-tm-family-strict-metal-noise");
    let config = r#"
schema_version = 1
game = "ipd"
rounds = 30
repetitions = 1
noise = 0.1

[engine]
accelerator = "metal"
fast_eval = true

[[strategy]]
id = "tm_rule"
type = "tm"
states = 1
symbols = 2
start_state = 1
blank = 0
max_steps_per_round = 32
rule_code = 0
"#;
    let request = GamesFamilyRunRequest {
        family: "tm".into(),
        input: "{1, 2}".into(),
        force: false,
    };
    let err = build_family_run_override_for_request(&root, config, &request)
        .expect_err("strict metal TM family prep should fail on unsupported noise");
    assert!(err.contains("Metal accelerator"), "unexpected error: {err}");
    assert!(err.contains("noise"), "unexpected error: {err}");
}

#[cfg(target_os = "macos")]
#[test]
fn command_games_run_tm_family_reports_metal_backend_when_available() {
    let root = temp_dir("cmd-games-run-tm-family-metal-diagnostics");
    let config = r#"
schema_version = 1
game = "ipd"
rounds = 30
repetitions = 1
noise = 0.0

[engine]
accelerator = "metal"
fast_eval = false

[[strategy]]
id = "tm_rule"
type = "tm"
states = 1
symbols = 2
start_state = 1
blank = 0
max_steps_per_round = 32
rule_code = 0
"#;
    let request = GamesFamilyRunRequest {
        family: "tm".into(),
        input: "{1, 2}".into(),
        force: false,
    };
    let (_override_run, timings) =
        match build_family_run_override_for_request_with_timings(&root, config, &request) {
            Ok(result) => result,
            Err(err)
                if err.contains("Metal accelerator unavailable")
                    || err.contains("active Metal backend")
                    || err.contains("Metal device unavailable") =>
            {
                return;
            }
            Err(err) => panic!("unexpected strict metal error: {err}"),
        };
    let diagnostics = timings.tm_filter.expect("expected TM prep diagnostics");
    assert!(matches!(
        diagnostics.backend,
        nit_games::TmHaltingFilterBackend::Metal
    ));
}

#[test]
fn command_games_run_tm_family_accepts_post_tuple_max_steps() {
    let root = temp_dir("cmd-games-run-tm-family-max-steps");
    let config = r#"
schema_version = 1
game = "ipd"
rounds = 5
repetitions = 1
noise = 0.0

[[strategy]]
id = "tm_rule"
type = "tm"
states = 1
symbols = 2
start_state = 1
blank = 0
max_steps_per_round = 32
rule_code = 0
"#;
    let mut state = AppState::new(
        root.clone(),
        Buffer::from_str("x", config, None),
        Buffer::empty("n", None),
    );
    state.app_kind = AppKind::Games;
    assert!(!handle_command_line(&mut state, ":games run tm {1, 2} 7"));
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
    assert!(override_run.config.strategies.iter().all(|spec| matches!(
        spec.kind,
        nit_games::config::StrategySpecKind::OneSidedTm {
            max_steps_per_round: 7,
            ..
        }
    )));
}

#[test]
fn command_games_run_tm_family_build_keeps_strict_metal_behavior() {
    let root = temp_dir("cmd-games-run-tm-family-metal-build");
    let config = r#"
schema_version = 1
game = "ipd"
rounds = 5
repetitions = 1
noise = 0.0

[engine]
accelerator = "metal"
fast_eval = false

[[strategy]]
id = "tm_rule"
type = "tm"
states = 1
symbols = 2
start_state = 1
blank = 0
max_steps_per_round = 32
rule_code = 0
"#;
    let mut state = AppState::new(
        root.clone(),
        Buffer::from_str("x", config, None),
        Buffer::empty("n", None),
    );
    state.app_kind = AppKind::Games;
    assert!(!handle_command_line(&mut state, ":games run tm {1, 2}"));
    let request = state
        .games
        .pending_family_run
        .as_ref()
        .expect("expected queued family request");
    let result = build_family_run_override_for_request_with_timings(
        &state.workspace_root,
        &state.editor_buffer().content_as_string(),
        request,
    );
    match result {
        Ok((_override_run, timings)) => {
            let diagnostics = timings.tm_filter.expect("expected TM prep diagnostics");
            assert!(matches!(
                diagnostics.backend,
                nit_games::TmHaltingFilterBackend::Metal
            ));
        }
        Err(err) => {
            assert!(
                err.contains("Metal accelerator"),
                "unexpected strict metal error: {err}"
            );
        }
    }
}

#[test]
fn command_games_run_tm_family_ignores_existing_generated_strategy_sources() {
    let root = temp_dir("cmd-games-run-tm-family-generated-base");
    let config = r#"
schema_version = 1
game = "ipd"
rounds = 5
repetitions = 1
noise = 0.0

[[strategy]]
id = "generated_tm"
type = "generated"
source = "missing-strategies.wl"
"#;
    let mut state = AppState::new(
        root.clone(),
        Buffer::from_str("x", config, None),
        Buffer::empty("n", None),
    );
    state.app_kind = AppKind::Games;
    assert!(!handle_command_line(&mut state, ":games run tm {1, 2}"));
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
    .expect("family build should ignore unrelated generated sources");
    assert!(!override_run.config.strategies.is_empty());
    assert!(override_run.config.tm_filter_applied);
}

#[test]
fn command_games_run_tm_family_preserves_inline_blank_hint() {
    let root = temp_dir("cmd-games-run-tm-family-blank-hint");
    let config = r#"
schema_version = 1
game = "ipd"
rounds = 5
repetitions = 1
noise = 0.0

[[strategy]]
id = "tm_rule"
type = "tm"
states = 1
symbols = 2
start_state = 1
blank = 1
max_steps_per_round = 32
rule_code = 0
"#;
    let mut state = AppState::new(
        root.clone(),
        Buffer::from_str("x", config, None),
        Buffer::empty("n", None),
    );
    state.app_kind = AppKind::Games;
    assert!(!handle_command_line(&mut state, ":games run tm {1, 2}"));
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
    .expect("family build should preserve explicit TM blank hint");
    assert!(override_run.config.strategies.iter().all(|spec| matches!(
        spec.kind,
        nit_games::config::StrategySpecKind::OneSidedTm { blank: 1, .. }
    )));
}

#[test]
fn command_games_run_tm_family_rejects_invalid_post_tuple_max_steps() {
    let root = temp_dir("cmd-games-run-tm-family-bad-max-steps");
    let config = r#"
schema_version = 1
game = "ipd"
rounds = 5
repetitions = 1
noise = 0.0
"#;
    let mut state = AppState::new(
        root.clone(),
        Buffer::from_str("x", config, None),
        Buffer::empty("n", None),
    );
    state.app_kind = AppKind::Games;
    assert!(!handle_command_line(
        &mut state,
        ":games run tm {1, 2} nope"
    ));
    assert!(state.games.pending_family_run.is_none());
    let status = state.status.clone().unwrap_or_default();
    assert!(status.contains("max_steps"));
}

#[test]
fn command_games_run_ca_family_queues_generated_override() {
    let root = temp_dir("cmd-games-run-ca-family");
    let config = r#"
schema_version = 1
game = "ipd"
rounds = 6
repetitions = 1
noise = 0.0

[[strategy]]
id = "ca_rule"
type = "ca"
n = 30
k = 2
r = 1
t = 2
"#;
    let mut state = AppState::new(
        root.clone(),
        Buffer::from_str("x", config, None),
        Buffer::empty("n", None),
    );
    state.app_kind = AppKind::Games;
    assert!(!handle_command_line(&mut state, ":games run ca {2, 1}"));
    assert!(!state.games.pending_run);
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
    assert_eq!(override_run.config.strategies.len(), 256);
    assert!(override_run
        .config
        .strategies
        .iter()
        .all(|spec| matches!(spec.kind, nit_games::config::StrategySpecKind::Ca { .. })));
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

#[test]
fn substrate_overlay_toggle_cycles_through_three_tabs() {
    let root = temp_dir("substrate-overlay-toggle");
    let mut state = AppState::new(
        root.clone(),
        Buffer::empty("x", None),
        Buffer::empty("n", None),
    );
    assert_eq!(state.substrate_overlay_tab, SubstrateOverlayTab::Signals);
    let _ = apply_action(&mut state, Action::SubstrateOverlayToggleTab);
    assert_eq!(state.substrate_overlay_tab, SubstrateOverlayTab::Claims);
    let _ = apply_action(&mut state, Action::SubstrateOverlayToggleTab);
    assert_eq!(state.substrate_overlay_tab, SubstrateOverlayTab::Assumptions);
    let _ = apply_action(&mut state, Action::SubstrateOverlayToggleTab);
    assert_eq!(state.substrate_overlay_tab, SubstrateOverlayTab::Signals);
}

#[test]
fn show_substrate_opens_overlay_and_resets_scroll() {
    let root = temp_dir("substrate-overlay-show");
    let mut state = AppState::new(
        root.clone(),
        Buffer::empty("x", None),
        Buffer::empty("n", None),
    );
    state.substrate_overlay_scroll = 42;
    let _ = apply_action(&mut state, Action::ShowSubstrate);
    assert!(state.show_substrate_overlay);
    assert_eq!(state.substrate_overlay_scroll, 0);
}

#[test]
fn hide_substrate_closes_overlay() {
    let root = temp_dir("substrate-overlay-hide");
    let mut state = AppState::new(
        root.clone(),
        Buffer::empty("x", None),
        Buffer::empty("n", None),
    );
    state.show_substrate_overlay = true;
    let _ = apply_action(&mut state, Action::HideSubstrate);
    assert!(!state.show_substrate_overlay);
}
