use super::*;
use super::{decrease_steps_per_tick, increase_steps_per_tick};

fn sample_snapshot() -> MatchSnapshot {
    MatchSnapshot {
        match_index: 1,
        total_matches: 1,
        round: 3,
        rounds: 3,
        a: "a".into(),
        b: "b".into(),
        a_score: -4,
        b_score: -2,
        outcomes: "013".into(),
        payoffs: vec![[-1, -1], [-3, 0], [0, -1]],
        a_halted: "110".into(),
        b_halted: "111".into(),
    }
}

fn line_text(line: &Line<'_>) -> String {
    line.spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect()
}

fn sample_config() -> nit_games::NormalizedConfig {
    nit_games::GamesConfig::from_toml(
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
    .expect("parse config")
}

fn new_app_state() -> AppState {
    AppState::new(
        std::env::temp_dir(),
        nit_core::Buffer::empty("x", None),
        nit_core::Buffer::empty("n", None),
    )
}

fn new_game_session(finished_elapsed: Option<Duration>) -> GameSession {
    GameSession {
        config: sample_config(),
        progress: None,
        snapshot: None,
        results: TournamentResults::empty(),
        definitions: Vec::new(),
        started_at: Instant::now(),
        finished_elapsed,
    }
}

#[allow(unused_macros)]
macro_rules! assert_has_line {
    ($lines:expr, $text:expr) => {
        assert!(
            $lines.iter().any(|line| line_text(line).contains($text)),
            "expected line containing {:?}",
            $text
        );
    };
}

fn cache_test_runtime() -> (GamesPetriDishRuntime, AppState) {
    let mut state = new_app_state();
    let mut runtime = GamesPetriDishRuntime::new(&state);
    runtime.session = Some(new_game_session(None));
    runtime.view = PetriView::Cache;
    runtime.cache_snapshot = nit_metal::BatchPolicyCacheSnapshot {
        root: Some("/tmp/metal-policy".into()),
        entries: vec![nit_metal::BatchPolicyCacheEntryInfo {
            key: "apple_m4_max_a".into(),
            path: "/tmp/metal-policy/apple_m4_max_a_v1.json".into(),
            device_name: "Apple M4 Max".into(),
            payload_signature: "fsm_s4_a2_n51924_static1mib".into(),
            matches_per_batch: 262_144,
            inflight_batches: 4,
        }],
    };
    state.games.status = GamesStatus::Running;
    (runtime, state)
}

#[test]
fn render_cache_browser_shows_selected_entry_details() {
    let snapshot = nit_metal::BatchPolicyCacheSnapshot {
        root: Some("/tmp/metal-policy".into()),
        entries: vec![
            nit_metal::BatchPolicyCacheEntryInfo {
                key: "apple_m4_max_a".into(),
                path: "/tmp/metal-policy/apple_m4_max_a_v1.json".into(),
                device_name: "Apple M4 Max".into(),
                payload_signature: "fsm_s4_a2_n51924_static1mib".into(),
                matches_per_batch: 262_144,
                inflight_batches: 4,
            },
            nit_metal::BatchPolicyCacheEntryInfo {
                key: "apple_m4_max_b".into(),
                path: "/tmp/metal-policy/apple_m4_max_b_v1.json".into(),
                device_name: "Apple M4 Max".into(),
                payload_signature: "tm_s2_sym2_steps64_n128_static1mib".into(),
                matches_per_batch: 32_768,
                inflight_batches: 4,
            },
        ],
    };

    let lines = render_cache_browser(
        &snapshot,
        1,
        96,
        Style::default(),
        Style::default(),
        Style::default(),
        Style::default(),
        Style::default(),
    );
    assert!(line_text(&lines[0]).contains("Metal Cache"));
    assert!(line_text(&lines[1]).contains("/tmp/metal-policy"));
    assert!(lines
        .iter()
        .any(|line| line_text(line).contains("apple_m4_max_b 32768x4")));
    assert!(lines
        .iter()
        .any(|line| line_text(line).contains("payload: tm_s2_sym2_steps64_n128_static1mib")));
    assert!(lines
        .iter()
        .any(|line| line_text(line).contains("path: /tmp/metal-policy/apple_m4_max_b_v1.json")));
}

#[test]
fn cache_clear_all_requires_confirmation() {
    let (mut runtime, mut state) = cache_test_runtime();
    assert!(runtime.handle_key(
        &KeyEvent::new(KeyCode::Char('X'), KeyModifiers::NONE),
        &mut state
    ));
    assert!(runtime.confirm_clear_all_cache);
}

#[test]
fn cache_clear_all_confirmation_can_be_cancelled() {
    let (mut runtime, mut state) = cache_test_runtime();
    runtime.confirm_clear_all_cache = true;
    assert!(runtime.handle_key(
        &KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE),
        &mut state
    ));
    assert!(!runtime.confirm_clear_all_cache);
    assert_eq!(state.status.as_deref(), Some("Metal cache clear cancelled"));
}

#[test]
fn match_inspector_uses_progress_summary_when_snapshot_is_missing() {
    let runtime = nit_games::RuntimeAcceleratorStats {
        backend: nit_games::RuntimeAcceleratorBackend::Metal,
        metal_matches: 1024,
        ..nit_games::RuntimeAcceleratorStats::default()
    };
    let lines = render_match_inspector(
        None,
        Some(TournamentProgress {
            match_index: 345,
            total_matches: 1000,
            round: 200,
            rounds: 200,
            match_complete: true,
            a: "a".into(),
            b: "b".into(),
            total_payoff_a: 0,
            total_payoff_b: -600,
            last_action_a: Some(nit_games::Action::Defect),
            last_action_b: Some(nit_games::Action::Cooperate),
            last_payoff_a: Some(0),
            last_payoff_b: Some(-3),
            last_halted_a: Some(true),
            last_halted_b: Some(true),
            last_outcome: Some(nit_games::game::Outcome::DC),
            runtime,
        }),
        &[],
        GamesStatus::Running,
        50,
        120,
        Style::default(),
        Style::default(),
        Style::default(),
        Style::default(),
        Style::default(),
        Style::default(),
    );
    assert!(line_text(&lines[0]).contains("Last Completed Match"));
    assert!(lines
        .iter()
        .any(|line| line_text(line).contains("last complete")));
    assert!(lines
        .iter()
        .any(|line| line_text(line).contains("Total: 0 / -600")));
    assert!(lines
        .iter()
        .any(|line| line_text(line).contains("Last: D / C (0 / -3)")));
    assert!(lines.iter().any(|line| {
        line_text(line).contains("Detailed round history is unavailable during GPU batching")
    }));
}

#[test]
fn match_inspector_uses_live_title_for_snapshot_mode() {
    let lines = render_match_inspector(
        Some(sample_snapshot()),
        None,
        &[],
        GamesStatus::Running,
        50,
        120,
        Style::default(),
        Style::default(),
        Style::default(),
        Style::default(),
        Style::default(),
        Style::default(),
    );

    assert!(line_text(&lines[0]).contains("Live Match"));
    assert!(lines
        .iter()
        .any(|line| line_text(line).contains("Match: 1/1 (round 3/3)")));
}

#[test]
fn petri_rect_uses_taller_height_on_large_screens() {
    let rect = petri_rect(Rect {
        x: 0,
        y: 0,
        width: 160,
        height: 48,
    });

    assert_eq!(rect.height, 40);
}

#[test]
fn match_strip_renders_halt_row_and_timeout_markers() {
    let lines = render_match_strip(
        &sample_snapshot(),
        50,
        120,
        Style::default(),
        Style::default(),
        Style::default(),
        Style::default(),
        Style::default(),
    );
    assert_eq!(lines.len(), 7);
    let halt_line = line_text(&lines[3]);
    assert!(halt_line.contains("Hlt:"));
    assert!(halt_line.contains("1/1"));
    assert!(halt_line.contains("0/1"));
}

#[test]
fn match_strip_handles_missing_halt_history() {
    let mut snapshot = sample_snapshot();
    snapshot.a_halted.clear();
    snapshot.b_halted.clear();

    let lines = render_match_strip(
        &snapshot,
        50,
        120,
        Style::default(),
        Style::default(),
        Style::default(),
        Style::default(),
        Style::default(),
    );
    assert!(line_text(&lines[3]).contains("--"));
}

#[test]
fn progress_waiting_text_reflects_running_status() {
    assert_eq!(
        progress_waiting_text(GamesStatus::Running),
        "Starting tournament..."
    );
    assert_eq!(
        progress_pending_round_text(GamesStatus::Running),
        "Round pending..."
    );
}

#[test]
fn progress_waiting_text_reflects_done_status() {
    assert_eq!(
        progress_waiting_text(GamesStatus::Done),
        "Tournament complete"
    );
}

#[test]
fn tm_family_prep_summary_reports_cpu_fallback_reason() {
    let timings = FamilyRunBuildTimings {
        tm_filter: Some(nit_games::TmHaltingFilterDiagnostics {
            backend: nit_games::TmHaltingFilterBackend::NotebookCpuFallback,
            metal_decline_reason: Some("non-zero noise disables Metal batch evaluation".into()),
            ..nit_games::TmHaltingFilterDiagnostics::default()
        }),
        ..FamilyRunBuildTimings::default()
    };

    let summary = tm_family_prep_summary(&timings).expect("expected prep summary");
    assert_eq!(
        summary,
        "prep fell back to CPU: non-zero noise disables Metal batch evaluation"
    );
}

#[test]
fn family_build_result_loading_message_includes_tm_prep_summary() {
    let mut state = new_app_state();
    let mut runtime = GamesPetriDishRuntime::new(&state);
    state.games.family_building = true;

    let (tx, rx) = mpsc::channel();
    runtime.family_run_rx = Some(rx);
    let timings = FamilyRunBuildTimings {
        generation_elapsed: Duration::from_millis(1),
        estimate_elapsed: Duration::from_millis(2),
        normalize_elapsed: Duration::from_millis(3),
        tm_filter_elapsed: Some(Duration::from_millis(4)),
        tm_filter: Some(nit_games::TmHaltingFilterDiagnostics {
            backend: nit_games::TmHaltingFilterBackend::NotebookCpuFallback,
            metal_decline_reason: Some("non-zero noise disables Metal batch evaluation".into()),
            ..nit_games::TmHaltingFilterDiagnostics::default()
        }),
        ..FamilyRunBuildTimings::default()
    };
    let outcome = FamilyBuildOutcome {
        force: false,
        result: Ok(GamesRunOverride {
            config: sample_config(),
            config_text: "schema_version = 1".into(),
            label: "tm {1, 2}".into(),
            family_mode: true,
        }),
        timings: Some(timings),
    };
    tx.send(outcome).expect("send family build outcome");

    runtime.handle_family_build_result(&mut state);
    let message = runtime
        .loading_message
        .as_deref()
        .expect("expected loading message");
    assert!(message.contains("Queued tournament"));
    assert!(message.contains("tm-filter"));
    assert!(message.contains("prep fell back to CPU"));
    assert!(message.contains("non-zero noise"));
}

#[test]
fn format_tournament_elapsed_uses_readable_units() {
    assert_eq!(
        format_tournament_elapsed(Duration::from_millis(875)),
        "875ms"
    );
    assert_eq!(
        format_tournament_elapsed(Duration::from_millis(12_345)),
        "12.345s"
    );
    assert_eq!(
        format_tournament_elapsed(Duration::from_millis(125_678)),
        "2m05.678s"
    );
}

#[test]
fn finished_session_elapsed_stays_frozen() {
    let frozen = Duration::from_millis(12_345);
    let mut session = new_game_session(Some(frozen));
    session.started_at = Instant::now()
        .checked_sub(Duration::from_secs(90))
        .unwrap_or_else(Instant::now);

    assert_eq!(
        session.elapsed_at(Instant::now() + Duration::from_secs(30)),
        frozen
    );
}

#[test]
fn session_footer_line_shows_elapsed_runtime() {
    let line = session_footer_line(
        2_048,
        false,
        false,
        Duration::from_millis(12_345),
        Style::default(),
        Style::default(),
        Style::default(),
        Style::default(),
    );

    assert_eq!(
        line_text(&line),
        "steps/tick: 2048  paused: no  elapsed: 12.345s"
    );
}

#[test]
fn session_footer_line_can_show_match_units() {
    let line = session_footer_line(
        32,
        true,
        false,
        Duration::from_millis(12_345),
        Style::default(),
        Style::default(),
        Style::default(),
        Style::default(),
    );

    assert_eq!(
        line_text(&line),
        "matches/tick: 32  paused: no  elapsed: 12.345s"
    );
}

#[test]
fn increase_steps_per_tick_clamps_normal_round_mode() {
    assert_eq!(increase_steps_per_tick(199, false), 200);
    assert_eq!(increase_steps_per_tick(200, false), 200);
}

#[test]
fn increase_steps_per_tick_preserves_large_existing_values() {
    assert_eq!(increase_steps_per_tick(50_000, false), 50_001);
    assert_eq!(increase_steps_per_tick(4_096, true), 4_097);
}

#[test]
fn increase_steps_per_tick_does_not_overflow() {
    assert_eq!(increase_steps_per_tick(u32::MAX, false), u32::MAX);
    assert_eq!(increase_steps_per_tick(u32::MAX, true), u32::MAX);
}

#[test]
fn decrease_steps_per_tick_stops_at_one() {
    assert_eq!(decrease_steps_per_tick(2), 1);
    assert_eq!(decrease_steps_per_tick(1), 1);
}

#[test]
fn tournament_progress_percent_uses_overall_progress() {
    let progress = TournamentProgress {
        match_index: 100_001,
        total_matches: 456_490,
        round: 0,
        rounds: 20,
        match_complete: false,
        a: "a".into(),
        b: "b".into(),
        total_payoff_a: 0,
        total_payoff_b: 0,
        last_action_a: None,
        last_action_b: None,
        last_payoff_a: None,
        last_payoff_b: None,
        last_halted_a: None,
        last_halted_b: None,
        last_outcome: None,
        runtime: nit_games::RuntimeAcceleratorStats::default(),
    };
    let pct = tournament_progress_percent(&progress);
    assert!(pct > 20.0);
    assert!(pct < 22.5);
}

#[test]
fn render_progress_labels_completed_batch_snapshot_without_round_counter() {
    let mut state = new_app_state();
    state.games.status = GamesStatus::Running;
    let progress = TournamentProgress {
        match_index: 345,
        total_matches: 1000,
        round: 200,
        rounds: 200,
        match_complete: true,
        a: "a".into(),
        b: "b".into(),
        total_payoff_a: 0,
        total_payoff_b: -600,
        last_action_a: Some(nit_games::Action::Defect),
        last_action_b: Some(nit_games::Action::Cooperate),
        last_payoff_a: Some(0),
        last_payoff_b: Some(-3),
        last_halted_a: Some(true),
        last_halted_b: Some(true),
        last_outcome: Some(nit_games::game::Outcome::DC),
        runtime: {
            let mut runtime = nit_games::RuntimeAcceleratorStats::default();
            runtime.note_metal_policy(
                131_072,
                4,
                nit_games::BatchPolicySource::Cached,
                Some("apple_m4_max_demo".into()),
                Some("/tmp/apple_m4_max_demo_v1.json".into()),
            );
            runtime
        },
    };

    let lines = render_progress(
        Some(progress),
        &[],
        &state,
        Style::default(),
        Style::default(),
        Style::default(),
        Style::default(),
        Style::default(),
    );
    let match_line = line_text(&lines[1]);
    assert!(match_line.contains("last complete"));
    assert!(!match_line.contains("round 200/200"));
    let live_line = line_text(&lines[2]);
    assert!(live_line.contains("Showing last completed match snapshot"));
    assert!(!live_line.contains("GPU batching"));
    assert!(lines
        .iter()
        .any(|line| line_text(line).contains("policy 131072x4 cached")));
    assert!(lines
        .iter()
        .any(|line| line_text(line).contains("AccelCache: /tmp/apple_m4_max_demo_v1.json")));
}

#[test]
fn family_mode_disables_history_to_preserve_metal_batching() {
    let mut state = new_app_state();
    let mut runtime = GamesPetriDishRuntime::new(&state);
    state.games.pending_run_override = Some(nit_core::GamesRunOverride {
        config: sample_config(),
        config_text: "schema_version = 1".into(),
        label: "fsm family".into(),
        family_mode: true,
    });

    runtime.start_session(&mut state);

    let session = runtime.session.as_ref().expect("session started");
    assert!(matches!(
        session.config.engine.mode,
        nit_games::EngineMode::Batch
    ));
    assert!(session.config.engine.fast_eval);
    assert!(!session.config.event_log.enabled);
    assert!(!session.config.history.enabled);
    assert!(state.games.match_history.capture_disabled_for_run);
}

#[test]
fn finished_hidden_session_reopens_games_petri_dish() {
    let mut state = new_app_state();
    let mut runtime = GamesPetriDishRuntime::new(&state);
    runtime.session = Some(new_game_session(None));
    runtime.hidden = true;
    state.games.running = true;
    state.games.status = GamesStatus::Running;
    state.games.petri_hidden = true;

    runtime.finish_session(
        &mut state,
        nit_games::output::RunSummary {
            schema_version: nit_games::output::RUN_SUMMARY_SCHEMA_VERSION,
            timestamp: "2026-03-11T12:00:00Z".into(),
            run_id: "run".into(),
            seed: 7,
            config_text: String::new(),
            config: sample_config(),
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
            results: TournamentResults::empty(),
            event_log: None,
            history_log: None,
            runtime: nit_games::RuntimeAcceleratorStats::default(),
            run_dir: None,
        },
    );

    assert_eq!(state.games.status, GamesStatus::Done);
    assert!(!runtime.hidden);
    assert!(!state.games.petri_hidden);
    assert!(runtime.is_visible());
}

#[test]
fn top_table_shows_aggregate_payoff_column_in_mean_mode() {
    let config = sample_config();
    let kernel = nit_games::TournamentKernel::new(config.clone());
    let results = nit_games::output::TournamentResults {
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
    };
    let (rank_w, score_w, total_w, wld_w) = top_table_widths(&config);
    let lines = render_top_table(
        &results,
        &config,
        kernel.definitions(),
        120,
        Style::default(),
        Style::default(),
        Style::default(),
        Style::default(),
        Style::default(),
        Style::default(),
        Style::default(),
        Style::default(),
        rank_w,
        score_w,
        total_w,
        wld_w,
    );

    assert!(lines
        .iter()
        .any(|line| line_text(line).contains("AggPayoff")));
    assert!(lines.iter().any(|line| line_text(line).contains(" -2 ")));
}

#[test]
fn wolfram_preview_line_is_expression_friendly() {
    let preview = nit_games::MatchHistoryPreview {
        match_index: 53,
        total_matches: 913_936,
        a: "0".into(),
        b: "867".into(),
        rounds_total: 4,
        outcomes: "2323".into(),
    };

    let line = wolfram_preview_line(&preview);
    assert_eq!(
        line,
        "<|\"match_index\" -> 53, \"total_matches\" -> 913936, \"a\" -> \"0\", \"b\" -> \"867\", \"rounds_total\" -> 4, \"outcomes\" -> \"2323\"|>"
    );
}
