use super::*;
use crate::theme::Theme;

fn line_text(line: &Line<'_>) -> String {
    line.spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect()
}

#[test]
fn last_run_table_shows_total_payoff_column_without_wld() {
    let config = nit_games::GamesConfig::from_toml(
        r#"
schema_version = 1
game = "ipd"
rounds = 2
repetitions = 1
self_play = true

[[strategy]]
id = "all_c"
type = "fsm"
index = 1
num_states = 1
k = 2
"#,
    )
    .expect("parse config");
    let requested_accelerator = config.engine.accelerator;

    let mut runtime = nit_games::RuntimeAcceleratorStats::new(requested_accelerator);
    runtime.note_metal_policy(
        131_072,
        4,
        nit_games::BatchPolicySource::Cached,
        Some("apple_m4_max_demo".into()),
        Some("/tmp/apple_m4_max_demo_v1.json".into()),
    );

    let run = nit_games::output::RunSummary {
        schema_version: nit_games::output::RUN_SUMMARY_SCHEMA_VERSION,
        timestamp: "2026-03-08T00:00:00Z".into(),
        run_id: "run".into(),
        seed: 42,
        config_text: String::new(),
        config,
        paths: nit_games::output::RunPaths {
            summary: None,
            events: None,
            history: None,
            definitions: None,
            results: None,
            config: None,
            analysis_dir: None,
        },
        strategies: Vec::new(),
        results: nit_games::output::TournamentResults {
            ranking: vec![nit_games::output::StrategyResult {
                id: "all_c".into(),
                name: None,
                total_payoff: -4,
                average_payoff: -1.0,
                adjusted_total_payoff: Some(-4.0),
                adjusted_average_payoff: Some(-1.0),
                matches: 2,
                wins: 0,
                losses: 0,
                draws: 2,
                crashed: false,
                crash_count: 0,
                tm_metrics: None,
            }],
            pairwise: Vec::new(),
            dominance: Vec::new(),
        },
        event_log: None,
        history_log: None,
        runtime,
        run_dir: None,
    };

    let lines = render_last_run_table(
        &run,
        120,
        Style::default(),
        Style::default(),
        Style::default(),
        Style::default(),
    );

    assert!(lines.iter().any(|line| line_text(line).contains("mean")));
    assert!(lines.iter().any(|line| line_text(line).contains("payoff")));
    assert!(lines.iter().any(|line| line_text(line).contains(" -2 ")));
    assert!(lines.iter().all(|line| !line_text(line).contains("W-L-D")));
}

#[test]
fn runtime_accelerator_formatter_includes_gpu_counts() {
    let mut runtime = nit_games::RuntimeAcceleratorStats::new(nit_games::AcceleratorMode::Metal);
    runtime.note_metal_batches(2, 32);
    runtime.note_cpu_matches(1);
    runtime.note_metal_policy(
        131_072,
        4,
        nit_games::BatchPolicySource::Cached,
        Some("apple_m4_max_demo".into()),
        Some("/tmp/apple_m4_max_demo_v1.json".into()),
    );

    let text = format_runtime_accelerator(&runtime);
    assert!(text.contains("metal"));
    assert!(text.contains("gpu 32"));
    assert!(text.contains("cpu 1"));
    assert!(text.contains("policy 131072x4 cached"));
}

#[test]
fn fsm_strategy_display_name_uses_symbol_count() {
    let opponent = strategy_display_name_from_kind(&StrategySpecKind::Fsm {
        outputs: vec![],
        start_state: 0,
        transitions: vec![],
        num_states: 1,
        input_mode: Some(InputMode::OpponentLastAction),
        index: None,
    });
    let joint = strategy_display_name_from_kind(&StrategySpecKind::Fsm {
        outputs: vec![],
        start_state: 0,
        transitions: vec![],
        num_states: 1,
        input_mode: Some(InputMode::JointLastAction),
        index: None,
    });

    assert_eq!(opponent, "FSM (s=1, k=2)");
    assert_eq!(joint, "FSM (s=1, k=4)");
}

#[test]
fn tm_strategy_display_name_uses_short_state_and_symbol_labels() {
    let tm = strategy_display_name_from_kind(&StrategySpecKind::OneSidedTm {
        states: 2,
        symbols: 3,
        start_state: 1,
        blank: 0,
        fallback_symbol: None,
        max_steps_per_round: 8,
        input_mode: InputMode::OpponentLastAction,
        output_map: vec![],
        transitions: vec![],
        rule_code: None,
    });

    assert_eq!(tm, "TM (s=2, k=3)");
}

#[test]
fn strategy_table_renders_bordered_summary_layout() {
    let strategies = vec![
        nit_games::config::StrategySpec {
            id: "fsm_allc".into(),
            name: None,
            kind: StrategySpecKind::Fsm {
                num_states: 1,
                start_state: 0,
                outputs: vec![],
                input_mode: Some(InputMode::OpponentLastAction),
                transitions: vec![],
                index: None,
            },
        },
        nit_games::config::StrategySpec {
            id: "tm_timeout".into(),
            name: None,
            kind: StrategySpecKind::OneSidedTm {
                states: 1,
                symbols: 2,
                start_state: 1,
                blank: 0,
                fallback_symbol: None,
                max_steps_per_round: 8,
                input_mode: InputMode::OpponentLastAction,
                output_map: vec![],
                transitions: vec![],
                rule_code: None,
            },
        },
    ];
    let refs: Vec<_> = strategies.iter().collect();

    let lines = render_strategy_table(
        &refs,
        54,
        Style::default(),
        Style::default(),
        Style::default(),
    );
    let rendered: Vec<String> = lines.iter().map(line_text).collect();

    assert!(rendered[0].starts_with('+'));
    assert!(rendered[0].chars().count() < 54);
    assert!(rendered[1].starts_with('|'));
    assert!(rendered[1].contains("id"));
    assert!(rendered[1].contains("summary"));
    assert!(rendered.iter().any(|line| line.contains("fsm_allc")));
    assert!(rendered.iter().any(|line| line.contains("FSM (s=1, k=2)")));
    assert!(rendered.iter().any(|line| line.contains("TM (s=1, k=2)")));
}

#[test]
fn payoff_lines_omit_payoff_legend_copy() {
    let payoff = nit_games::game::PayoffMatrix::default_pd();
    let lines = payoff_lines(
        &payoff,
        60,
        Style::default(),
        Style::default(),
        Style::default(),
    );
    let rendered: Vec<String> = lines.iter().map(line_text).collect();

    assert!(rendered.iter().any(|line| line.contains("payoff: R=")));
    assert!(rendered.iter().any(|line| line.contains("matrix:")));
    assert!(rendered.iter().all(|line| !line.contains("reward (C,C)")));
    assert!(rendered.iter().all(|line| !line.contains("sucker (C,D)")));
    assert!(rendered
        .iter()
        .all(|line| !line.contains("temptation (D,C)")));
    assert!(rendered
        .iter()
        .all(|line| !line.contains("punishment (D,D)")));
}

#[test]
fn layout_for_config_keeps_side_panel_width_stable_across_last_run_content() {
    let config = nit_games::GamesConfig::from_toml(
        r#"
schema_version = 1
game = "ipd"
rounds = 2
repetitions = 1
self_play = true

[[strategy]]
id = "all_c"
type = "fsm"
index = 1
num_states = 1
k = 2
"#,
    )
    .expect("parse config");
    let state = AppState::new(
        std::env::temp_dir(),
        nit_core::Buffer::empty("x", None),
        nit_core::Buffer::empty("n", None),
    );
    let empty_layout = layout_for_config(
        Rect {
            x: 0,
            y: 0,
            width: 90,
            height: 30,
        },
        &state,
        Some(&config),
    );
    let empty_side = empty_layout.side.expect("empty side panel");

    let mut state_with_run = AppState::new(
        std::env::temp_dir(),
        nit_core::Buffer::empty("x", None),
        nit_core::Buffer::empty("n", None),
    );
    state_with_run.games.last_run = Some(nit_games::output::RunSummary {
        schema_version: nit_games::output::RUN_SUMMARY_SCHEMA_VERSION,
        timestamp: "2026-03-11T23:00:28.511548Z".into(),
        run_id: "82d85202af92c04e".into(),
        seed: 42,
        config_text: String::new(),
        config: config.clone(),
        paths: nit_games::output::RunPaths {
            summary: None,
            events: None,
            history: None,
            definitions: None,
            results: None,
            config: None,
            analysis_dir: None,
        },
        strategies: Vec::new(),
        results: nit_games::output::TournamentResults {
            ranking: vec![nit_games::output::StrategyResult {
                id: "fsm_alld".into(),
                name: None,
                total_payoff: -1690,
                average_payoff: -0.884,
                adjusted_total_payoff: Some(-1690.0),
                adjusted_average_payoff: Some(-0.884),
                matches: 1,
                wins: 0,
                losses: 0,
                draws: 1,
                crashed: false,
                crash_count: 0,
                tm_metrics: None,
            }],
            pairwise: Vec::new(),
            dominance: Vec::new(),
        },
        event_log: None,
        history_log: None,
        runtime: nit_games::RuntimeAcceleratorStats::default(),
        run_dir: None,
    });

    let layout = layout_for_config(
        Rect {
            x: 0,
            y: 0,
            width: 90,
            height: 30,
        },
        &state_with_run,
        Some(&config),
    );

    let side = layout.side.expect("side panel");
    assert!(layout.show_payoff_side);
    assert_eq!(side.width, empty_side.width);
}

#[test]
fn layout_for_config_keeps_empty_last_run_panel_wide_enough() {
    let config = nit_games::GamesConfig::from_toml(
        r#"
schema_version = 1
game = "ipd"
rounds = 2
repetitions = 1
self_play = true

[[strategy]]
id = "all_c"
type = "fsm"
index = 1
num_states = 1
k = 2
"#,
    )
    .expect("parse config");
    let state = AppState::new(
        std::env::temp_dir(),
        nit_core::Buffer::empty("x", None),
        nit_core::Buffer::empty("n", None),
    );

    let layout = layout_for_config(
        Rect {
            x: 0,
            y: 0,
            width: 90,
            height: 30,
        },
        &state,
        Some(&config),
    );

    let side = layout.side.expect("side panel");
    assert!(layout.show_payoff_side);
    assert!(side.width as usize >= LAST_RUN_PANEL_TARGET_WIDTH + 2 + LAST_RUN_PANEL_EXTRA_WIDTH);
}

#[test]
fn build_side_lines_wrap_long_last_run_fields_without_overflow() {
    let config = nit_games::GamesConfig::from_toml(
        r#"
schema_version = 1
game = "ipd"
rounds = 2
repetitions = 1
self_play = true

[[strategy]]
id = "all_c"
type = "fsm"
index = 1
num_states = 1
k = 2
"#,
    )
    .expect("parse config");
    let mut state = AppState::new(
        std::env::temp_dir(),
        nit_core::Buffer::empty("x", None),
        nit_core::Buffer::empty("n", None),
    );
    let runtime = nit_games::RuntimeAcceleratorStats {
        backend: nit_games::RuntimeAcceleratorBackend::Metal,
        metal_matches: 913_936,
        metal_matches_per_batch: Some(262_144),
        metal_inflight_batches: Some(5),
        metal_policy_cache_path: Some(
            "/Users/nitrika/Library/Caches/dev.arcxlab.nit/games/metal-policy/apple_m4_max_1872106799188804901_v1.json"
                .into(),
        ),
        ..nit_games::RuntimeAcceleratorStats::default()
    };
    state.games.last_run = Some(nit_games::output::RunSummary {
        schema_version: nit_games::output::RUN_SUMMARY_SCHEMA_VERSION,
        timestamp: "2026-03-11T23:19:22.86116Z".into(),
        run_id: "c3ca2b14966fcff".into(),
        seed: 42,
        config_text: String::new(),
        config,
        paths: nit_games::output::RunPaths {
            summary: Some(
                "/Users/nitrika/Projects/Configs/nit/runs/games/2026-03-11T23-19-22.86116Z__seed-42/run_summary.json"
                    .into(),
            ),
            events: None,
            history: None,
            definitions: None,
            results: None,
            config: None,
            analysis_dir: None,
        },
        strategies: Vec::new(),
        results: nit_games::output::TournamentResults {
            ranking: vec![nit_games::output::StrategyResult {
                id: "fsm_3495".into(),
                name: None,
                total_payoff: -1690,
                average_payoff: -0.884,
                adjusted_total_payoff: Some(-1690.0),
                adjusted_average_payoff: Some(-0.884),
                matches: 1,
                wins: 0,
                losses: 0,
                draws: 1,
                crashed: false,
                crash_count: 0,
                tm_metrics: None,
            }],
            pairwise: Vec::new(),
            dominance: Vec::new(),
        },
        event_log: None,
        history_log: None,
        runtime,
        run_dir: None,
    });

    let width = 40;
    let rendered: Vec<String> = build_side_lines(&state, &Theme::default(), width)
        .iter()
        .map(line_text)
        .collect();

    let accel_cache_idx = rendered
        .iter()
        .position(|line| line.starts_with("accel_cache: "))
        .expect("accel_cache line");
    let summary_idx = rendered
        .iter()
        .position(|line| line.starts_with("summary: "))
        .expect("summary line");
    assert!(rendered[accel_cache_idx + 1].starts_with("             "));
    assert!(!rendered[accel_cache_idx + 1].trim().is_empty());
    assert!(rendered[summary_idx + 1].starts_with("         "));
    assert!(!rendered[summary_idx + 1].trim().is_empty());
    assert!(display_width(&rendered[accel_cache_idx]) <= width);
    assert!(display_width(&rendered[accel_cache_idx + 1]) <= width);
    assert!(display_width(&rendered[summary_idx]) <= width);
    assert!(display_width(&rendered[summary_idx + 1]) <= width);
}
