use std::time::{Duration, Instant};

use crate::genome_worker::GenomeWorker;
use crate::workspace_scan::WorkspaceScanRuntime;

use super::{cleanup, drain_until_idle, make_state, temp_workspace, write_file};

#[test]
fn worker_result_carries_workspace_scan_flag() {
    // A workspace_scan result must carry `workspace_scan: true` and route
    // through `evaluate_from_disk_workspace_scan` end-to-end.
    let root = temp_workspace();
    let file = write_file(&root, "src/lib.rs", "fn main() {}\n");

    let worker = GenomeWorker::new();
    assert!(worker.evaluate_from_disk_workspace_scan(file.clone()));

    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Ok(result) = worker.rx.try_recv() {
            assert!(result.workspace_scan);
            assert!(!result.shadow);
            assert!(!result.save_eval);
            assert_eq!(result.path, file);
            assert!(result.report.is_some());
            break;
        }
        if Instant::now() >= deadline {
            panic!("worker did not produce a result within 10s");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    cleanup(&root);
}

#[test]
fn persist_then_delete_round_trip() {
    let root = temp_workspace();
    let file = write_file(&root, "src/lib.rs", "fn main() {}\n");
    let report = nit_core::compute_genome_report("fn main() {}\n", &file);
    nit_core::agent_bus::persist_genome_report(&root, &report);

    let map = nit_core::agent_bus::load_genome_reports(&root);
    assert!(map.contains_key(&file));

    nit_core::agent_bus::delete_genome_report(&root, &file);
    let map_after = nit_core::agent_bus::load_genome_reports(&root);
    assert!(!map_after.contains_key(&file));
    cleanup(&root);
}

#[test]
fn backfill_completions_drain_from_live_and_do_not_persist() {
    // Hydrate queues files needing re-eval. Each result removes the path
    // from pending/dispatched; LIVE drains to empty, confirming entries
    // live only as long as work is in-flight.
    let root = temp_workspace();
    for idx in 0..4 {
        write_file(
            &root,
            &format!("src/f{idx}.rs"),
            &format!("fn f{idx}() {{}}\n"),
        );
    }

    let mut state = make_state(root.clone());
    let mut scan = WorkspaceScanRuntime::new();
    let worker = GenomeWorker::new();

    scan.hydrate(&mut state);
    assert_eq!(scan.pending_count() + scan.dispatched_count(), 4);

    drain_until_idle(&mut state, &mut scan, &worker, Duration::from_secs(30));
    assert!(scan.in_flight_snapshot().is_empty());
    cleanup(&root);
}

#[test]
fn fresh_runtime_starts_empty_after_prior_activity() {
    // A brand-new WorkspaceScanRuntime (simulates nit relaunch) must not
    // inherit any state from a prior instance. The runtime owns no
    // serialized fields, so the invariant holds even if a struct field
    // gets added later.
    let root = temp_workspace();
    for idx in 0..3 {
        write_file(
            &root,
            &format!("src/f{idx}.rs"),
            &format!("fn f{idx}() {{}}\n"),
        );
    }

    let mut state = make_state(root.clone());
    let worker = GenomeWorker::new();

    {
        let mut scan = WorkspaceScanRuntime::new();
        scan.hydrate(&mut state);
        drain_until_idle(&mut state, &mut scan, &worker, Duration::from_secs(30));
        let (done_a, total_a) = scan.progress();
        assert_eq!(done_a, total_a);
        assert!(done_a > 0);
    }

    let scan_b = WorkspaceScanRuntime::new();
    assert_eq!(scan_b.pending_count(), 0);
    assert_eq!(scan_b.dispatched_count(), 0);
    let (done_b, total_b) = scan_b.progress();
    assert_eq!(done_b, 0);
    assert_eq!(total_b, 0);
    assert!(!scan_b.is_scanning());
    cleanup(&root);
}

#[test]
fn gate_monitor_tab_clicks_set_target_sub_view_directly() {
    // Regression: STATS / FILESCORES / LIVE tabs once shared a single cycle
    // action, so clicking a non-adjacent tab from the current view stepped
    // through the cycle rather than jumping to the target. Each button now
    // returns the correct direct-set action, and applying it from any
    // starting state lands on the requested tab.
    use crate::widgets::gate_monitor_view::footer_hit;
    use nit_core::{Action, GateMonitorSubView};

    let root = temp_workspace();
    let mut state = make_state(root.clone());
    for width in [40, 60, 100] {
        for (col, tab) in [
            (3, GateMonitorSubView::Stats),
            (10, GateMonitorSubView::FileScores),
            (17, GateMonitorSubView::Live),
            (24, GateMonitorSubView::Skills),
        ] {
            assert_eq!(
                footer_hit(col, width, &state),
                Some(Action::GateMonitorSetSubView(tab))
            );
        }
        assert_eq!(
            footer_hit(31, width, &state),
            Some(Action::WorkspaceScanStart)
        );
        assert_eq!(footer_hit(width - 1, width, &state), None);
    }

    state.gate_monitor_sub_view = GateMonitorSubView::Stats;
    nit_core::apply_action(
        &mut state,
        Action::GateMonitorSetSubView(GateMonitorSubView::Live),
    );
    assert_eq!(state.gate_monitor_sub_view, GateMonitorSubView::Live);

    state.gate_monitor_sub_view = GateMonitorSubView::Live;
    nit_core::apply_action(
        &mut state,
        Action::GateMonitorSetSubView(GateMonitorSubView::Stats),
    );
    assert_eq!(state.gate_monitor_sub_view, GateMonitorSubView::Stats);

    state.gate_monitor_sub_view = GateMonitorSubView::Live;
    nit_core::apply_action(
        &mut state,
        Action::GateMonitorSetSubView(GateMonitorSubView::FileScores),
    );
    assert_eq!(state.gate_monitor_sub_view, GateMonitorSubView::FileScores);
    cleanup(&root);
}

#[test]
fn genome_footer_only_draws_complete_clickable_controls_at_compact_widths() {
    use crate::{theme::Theme, widgets::gate_monitor_view};
    use nit_core::{Action, GateMonitorSubView};
    use ratatui::{backend::TestBackend, Terminal};

    let root = temp_workspace();
    let mut state = make_state(root.clone());
    state.gate_monitor_sub_view = GateMonitorSubView::Skills;
    let scan = WorkspaceScanRuntime::new();
    for width in [12, 24, 30, 31, 32, 33, 34, 35, 40, 80] {
        let mut terminal = Terminal::new(TestBackend::new(width, 12)).unwrap();
        terminal
            .draw(|f| gate_monitor_view::render(f, f.size(), &mut state, &scan, &Theme::default()))
            .unwrap();
        let buf = terminal.backend().buffer();
        let text_for = |action: Action| -> String {
            (0..width)
                .filter(|&x| {
                    gate_monitor_view::footer_hit(x, width, &state).as_ref() == Some(&action)
                })
                .map(|x| buf.get(x, 11).symbol())
                .collect()
        };
        for (tab, title) in [
            (GateMonitorSubView::Stats, "STATS"),
            (GateMonitorSubView::FileScores, "FILES"),
            (GateMonitorSubView::Live, "LIVE"),
            (GateMonitorSubView::Skills, "SKILLS"),
        ] {
            let actual = text_for(Action::GateMonitorSetSubView(tab));
            if !actual.is_empty() || tab == GateMonitorSubView::Skills || width >= 30 {
                assert_eq!(actual.trim(), title, "width {width}");
            }
        }
        if width == 24 {
            assert_eq!(text_for(Action::GateMonitorToggleSubView).trim(), "Next ›");
        }
        let eval = text_for(Action::WorkspaceScanStart);
        assert!(
            matches!(eval.trim(), "EVAL" | "▶ EVAL" | "▶ EVALUATE GENOME"),
            "width {width}: {eval:?}"
        );
        assert_eq!(
            gate_monitor_view::footer_hit(width - 1, width, &state),
            None
        );
    }
    cleanup(&root);
}
