//! Tests for the agent ops view: DAG dashboard lines, artifacts history,
//! roster styling, and saved-run metadata. Tests use `SwarmDashboardView`/
//! `SwarmPersistenceView` fixtures wired up through `AppState` and assert
//! against the rendered lines and spans emitted by the ops view helpers.

use super::{
    append_swarm_artifact_lines, arrow_glyph, artifacts_history_entries,
    artifacts_history_visible_entries, current_lines_for_width, cursor_glyph,
    dag_lines_for_dashboard, diagnostics_lines, format_saved_run_relative_label_from_micros,
    mission_visible_agent_lines, ops_styled_line, parse_roster_truncation_disabled,
    roster_backend_total_counts, roster_column_widths, roster_grouped_agent_indices,
    roster_inventory_backend_accent, roster_lane_backend_accent, roster_running_priority,
    roster_styled_line, roster_swarm_mission_hit, roster_swarm_mission_line_idx,
    roster_swarm_template_hit, roster_swarm_template_line_idx, saved_run_detail_label,
    swarm_clone_display_label, table_role_label, tree_closed_glyph, tree_open_glyph,
    BackendInventoryBackend, MISSION_VISIBLE_AGENTS_MAX, ROSTER_VISIBLE_AGENTS_PER_BACKEND,
};
use crate::swarm::{
    GateReport, GateReportGate, SwarmDashboardView, SwarmGateDashboardRow, SwarmPersistenceView,
    SwarmTaskDashboardRow, SwarmTaskPersistenceView,
};
use crate::theme::Theme;
use nit_core::{
    AgentAlertSeverity, AgentChannel, AgentLane, AgentLaneKind, AgentMessage, AgentOpsTab,
    AgentStatus, AppKind, AppState, Buffer, MissionPhase, MissionRecord, PatchProposal,
    PatchStatus, SavedRunHistoryFilter,
};
use ratatui::style::Modifier;
use std::{
    fs,
    time::{SystemTime, UNIX_EPOCH},
};

/// Baseline `AgentLane` for roster-rendering tests: idle, zeroed counters,
/// no mission. All idle-roster assertions are insensitive to those fields —
/// only id/role/lane/kind vary between rows.
fn idle_lane(id: &str, lane: &str, kind: AgentLaneKind) -> AgentLane {
    AgentLane {
        id: id.into(),
        role: id.into(),
        lane: lane.into(),
        kind,
        status: AgentStatus::Idle,
        heartbeat_age_secs: 0,
        queue_len: 0,
        current_mission: None,
        shadow: false,
        last_message: String::new(),
    }
}

fn unique_test_workspace(label: &str) -> std::path::PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let path = std::env::temp_dir().join(format!(
        "nit-artifacts-{label}-{}-{nanos}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&path);
    fs::create_dir_all(&path).expect("create workspace");
    path
}

#[test]
fn dag_lines_include_tasks_and_gates() {
    let dashboard = SwarmDashboardView {
        mission_id: "mis-009".into(),
        template: "plan-v2".into(),
        phase: "EXEC".into(),
        done: 1,
        failed: 0,
        skipped: 0,
        running: 1,
        queued: 0,
        pending: 0,
        tasks: vec![SwarmTaskDashboardRow {
            id: "t1".into(),
            title: "Integrate dashboard changes".into(),
            role: Some("integrator".into()),
            agent_id: "agent-1".into(),
            state: "Running".into(),
            deps: vec!["t0".into()],
            blocked_on: Vec::new(),
            writes: true,
            done_when: Some("UI matches spec".into()),
            output_present: false,
        }],
        gate_bundle: Some("rust-ci".into()),
        gates: vec![SwarmGateDashboardRow {
            name: "fmt".into(),
            command: "cargo fmt --all -- --check".into(),
            status: "PENDING".into(),
            notes: None,
        }],
    };

    let lines = dag_lines_for_dashboard(&dashboard, 80);
    assert!(lines.iter().any(|line| line.contains("Status:")));
    assert!(lines.iter().any(|line| line.contains("t1")));
    assert!(lines.iter().any(|line| line.contains("fmt")));
}

#[test]
fn dag_lines_wrap_instead_of_ellipsis() {
    let dashboard = SwarmDashboardView {
        mission_id: "mis-010".into(),
        template: "plan-v2".into(),
        phase: "EXEC".into(),
        done: 0,
        failed: 0,
        skipped: 0,
        running: 1,
        queued: 0,
        pending: 0,
        tasks: vec![SwarmTaskDashboardRow {
            id: "t1".into(),
            title: "This is a very long title that should wrap across multiple lines".into(),
            role: Some("integrator".into()),
            agent_id: "agent-1".into(),
            state: "Running".into(),
            deps: vec!["t0".into(), "t2".into(), "t3".into(), "t4".into()],
            blocked_on: vec!["gate-fmt".into(), "gate-clippy".into()],
            writes: true,
            done_when: Some(
                "Ensure the DAG view never truncates with ellipsis; wrap instead.".into(),
            ),
            output_present: false,
        }],
        gate_bundle: Some(
            "bundle-with-a-very-long-name-that-must-wrap-instead-of-truncating".into(),
        ),
        gates: vec![SwarmGateDashboardRow {
            name: "fmt".into(),
            command: "cargo fmt --all -- --check && echo \"hello world\" && echo \"more\"".into(),
            status: "PENDING".into(),
            notes: None,
        }],
    };

    let lines = dag_lines_for_dashboard(&dashboard, 48);
    assert!(
        !lines.iter().any(|line| line.contains('…')),
        "expected DAG output to wrap without ellipsis"
    );
    assert!(
        lines.iter().any(|line| line.trim_end().ends_with('\\')),
        "expected wrapped commands to use backslash continuation"
    );
}

#[test]
fn artifact_lines_include_task_and_verify_paths() {
    let view = SwarmPersistenceView {
        mission_id: "mis-011".into(),
        template: "bulk".into(),
        phase: "REPORT".into(),
        gate_bundle: Some("rust-ci".into()),
        gate_selection: "auto".into(),
        gate_report: Some(GateReport {
            overall_ok: false,
            gates: vec![GateReportGate {
                name: "fmt".into(),
                command: "cargo fmt --all -- --check".into(),
                ok: false,
                status: Some("fail".into()),
                notes: Some("formatting drift".into()),
            }],
        }),
        gate_output: Some("fmt failed".into()),
        report_status: Some("DONE".into()),
        report_agent_id: Some("planner".into()),
        report_output: Some("# Final Report\n\nShip it.\n".into()),
        tasks: vec![
            SwarmTaskPersistenceView {
                id: "integrate".into(),
                title: "Integrate artifacts tab".into(),
                role: Some("integrate".into()),
                agent_id: "agent-1".into(),
                state: "DONE".into(),
                deps: vec!["judge".into()],
                blocked_on: Vec::new(),
                writes: true,
                done_when: Some("Artifacts visible in Agent Ops".into()),
                expected_artifacts: vec!["files".into(), "commands".into()],
                expected_artifacts_missing: false,
                output_present: true,
                output: Some("done".into()),
                artifacts: Some(crate::swarm::SwarmTaskArtifacts {
                    summary: Some("Surfaced mission artifacts in the TUI".into()),
                    files: vec![crate::swarm::SwarmArtifactFile {
                        path: "crates/nit-tui/src/widgets/agent_ops_view.rs".into(),
                        notes: Some("new Artifacts tab".into()),
                    }],
                    diffs: Vec::new(),
                    commands: vec![crate::swarm::SwarmArtifactCommand {
                        cmd: "cargo test -p nit-tui agent_ops_view::tests".into(),
                        purpose: Some("validate rendering".into()),
                    }],
                    risks: Vec::new(),
                    notes: vec!["needs follow-up for artifact comments".into()],
                }),
            },
            SwarmTaskPersistenceView {
                id: "review".into(),
                title: "Review artifacts output".into(),
                role: Some("review".into()),
                agent_id: "agent-2".into(),
                state: "DONE".into(),
                deps: vec!["integrate".into()],
                blocked_on: Vec::new(),
                writes: false,
                done_when: None,
                expected_artifacts: vec!["risks".into()],
                expected_artifacts_missing: true,
                output_present: false,
                output: None,
                artifacts: None,
            },
        ],
    };

    let mut lines = vec![" ARTIFACTS".into(), "─".repeat(80)];
    append_swarm_artifact_lines(&mut lines, &view, 80);

    assert!(lines
        .iter()
        .any(|line| line.contains(".nit/swarm/mis-011/tasks/integrate/artifacts.json")));
    assert!(lines
        .iter()
        .any(|line| line.contains(".nit/swarm/mis-011/gates/verify.md")));
    assert!(lines
        .iter()
        .any(|line| line.contains(".nit/swarm/mis-011/report/final.md")));
    assert!(lines
        .iter()
        .any(|line| line.contains("no parseable swarm_artifacts JSON block")));
    assert!(lines.iter().any(|line| line.contains("fmt [FAIL]")));
}

#[test]
fn artifacts_tab_shows_non_swarm_mission_provenance() {
    let mut state = AppState::new(
        std::env::temp_dir(),
        Buffer::empty("x", None),
        Buffer::empty("n", None),
    );
    state.agents.dock_tab = AgentOpsTab::Evidence;
    state.agents.missions = vec![MissionRecord {
        id: "mis-201".into(),
        title: "Repo review".into(),
        phase: MissionPhase::Report,
        swarm: false,
        assigned_agents: vec!["gpt-5.4".into()],
        status: "DONE".into(),
        updated_at: "now".into(),
    }];
    state.agents.mission_selected = 0;
    state.agents.selected_mission = Some("mis-201".into());
    state.agents.messages = vec![
        AgentMessage {
            at: "10:00".into(),
            channel: AgentChannel::Agent,
            agent_id: None,
            mission_id: Some("mis-201".into()),
            text: "Review the repo carefully.".into(),
            prompt_msg_idx: None,
            kind: None,
        },
        AgentMessage {
            at: "10:01".into(),
            channel: AgentChannel::Agent,
            agent_id: Some("gpt-5.4".into()),
            mission_id: Some("mis-201".into()),
            text: "Bulk orchestration looks mostly correct; docs need follow-up.".into(),
            prompt_msg_idx: None,
            kind: None,
        },
    ];
    state.agents.patches = vec![PatchProposal {
        id: "patch-201".into(),
        mission_id: Some("mis-201".into()),
        agent_id: "gpt-5.4".into(),
        title: "Review notes".into(),
        summary: "No code changes; notes only.".into(),
        diff: "diff --git a/docs/SWARM.md b/docs/SWARM.md".into(),
        status: PatchStatus::Reviewed,
    }];

    let lines = current_lines_for_width(&state, 96);
    assert!(lines
        .iter()
        .any(|line| line.contains(".nit/agents/runs/mis-201/thread.md")));
    assert!(lines
        .iter()
        .any(|line| line.contains(".nit/agents/runs/mis-201/run.json")));
    assert!(lines.iter().any(|line| line.contains("single-agent")));
    assert!(lines
        .iter()
        .any(|line| line.contains("Bulk orchestration looks mostly correct")));
    assert!(lines
        .iter()
        .any(|line| line.contains("PATCH") && line.contains("Review notes")));
}

#[test]
fn artifacts_tab_shows_ad_hoc_selected_agent_output_without_mission() {
    let mut state = AppState::new(
        std::env::temp_dir(),
        Buffer::empty("x", None),
        Buffer::empty("n", None),
    );
    state.agents.dock_tab = AgentOpsTab::Evidence;
    state.agents.missions.clear();
    state.agents.selected_mission = None;
    state.agents.selected_agent = Some("gpt-5.4".into());
    state.agents.messages = vec![
        AgentMessage {
            at: "11:00".into(),
            channel: AgentChannel::Agent,
            agent_id: None,
            mission_id: None,
            text: "Review the repo and report only.".into(),
            prompt_msg_idx: None,
            kind: None,
        },
        AgentMessage {
            at: "11:01".into(),
            channel: AgentChannel::Agent,
            agent_id: Some("gpt-5.4".into()),
            mission_id: None,
            text: "I checked the repo health; no flickering issue was reproduced.".into(),
            prompt_msg_idx: None,
            kind: None,
        },
    ];
    state
        .agents
        .codex_thread_ids
        .insert("gpt-5.4".into(), "thread-123".into());

    let lines = current_lines_for_width(&state, 96);
    assert!(lines.iter().any(|line| line.contains("Context: ad-hoc")));
    assert!(lines.iter().any(|line| line.contains("thread-123")));
    assert!(lines
        .iter()
        .any(|line| line.contains("no flickering issue was reproduced")));
}

#[test]
fn artifacts_tab_falls_back_to_saved_mission_run_when_live_context_is_empty() {
    let workspace = unique_test_workspace("mission");
    let run_dir = workspace.join(".nit/agents/runs/mis-301");
    fs::create_dir_all(&run_dir).expect("create run dir");
    fs::write(
        run_dir.join("run.json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "messages": [
                {
                    "at": "12:00",
                    "channel": "Agent",
                    "agent_id": null,
                    "mission_id": "mis-301",
                    "text": "Review this subsystem."
                },
                {
                    "at": "12:01",
                    "channel": "Agent",
                    "agent_id": "gpt-5.4",
                    "mission_id": "mis-301",
                    "text": "Saved mission reply from disk."
                }
            ],
            "patches": [
                {
                    "id": "patch-301",
                    "mission_id": "mis-301",
                    "agent_id": "gpt-5.4",
                    "title": "Persisted patch",
                    "summary": "loaded from disk",
                    "diff": "diff --git a/a b/a",
                    "status": "Reviewed"
                }
            ],
            "evidence": [
                {
                    "id": "evidence-301",
                    "mission_id": "mis-301",
                    "agent_id": "gpt-5.4",
                    "title": "Persisted evidence",
                    "detail": "saved detail",
                    "link": null
                }
            ],
            "codex_thread_ids": {
                "gpt-5.4": "thread-301"
            }
        }))
        .expect("serialize run"),
    )
    .expect("write run");

    let mut state = AppState::new(
        workspace,
        Buffer::empty("x", None),
        Buffer::empty("n", None),
    );
    state.agents.dock_tab = AgentOpsTab::Evidence;
    state.agents.missions = vec![MissionRecord {
        id: "mis-301".into(),
        title: "Saved mission".into(),
        phase: MissionPhase::Report,
        swarm: false,
        assigned_agents: vec!["gpt-5.4".into()],
        status: "DONE".into(),
        updated_at: "now".into(),
    }];
    state.agents.mission_selected = 0;
    state.agents.selected_mission = Some("mis-301".into());
    state.agents.selected_agent = Some("gpt-5.4".into());
    state.agents.messages.clear();
    state.agents.patches.clear();
    state.agents.evidence.clear();

    let lines = current_lines_for_width(&state, 96);
    assert!(lines.iter().any(|line| line.contains("thread-301")));
    assert!(lines
        .iter()
        .any(|line| line.contains("Saved mission reply from disk")));
    assert!(lines
        .iter()
        .any(|line| line.contains("PATCH") && line.contains("Persisted patch")));
}

#[test]
fn artifacts_tab_falls_back_to_saved_ad_hoc_run_when_live_context_is_empty() {
    let workspace = unique_test_workspace("adhoc");
    let run_dir = workspace.join(".nit/agents/ad-hoc/gpt-5_4");
    fs::create_dir_all(&run_dir).expect("create ad-hoc dir");
    fs::write(
        run_dir.join("run.json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "agent_id": "gpt-5.4",
            "codex_thread_id": "thread-adhoc",
            "messages": [
                {
                    "at": "13:00",
                    "channel": "Agent",
                    "agent_id": null,
                    "mission_id": null,
                    "text": "Explain the failure."
                },
                {
                    "at": "13:01",
                    "channel": "Agent",
                    "agent_id": "gpt-5.4",
                    "mission_id": null,
                    "text": "Saved ad-hoc reply from disk."
                }
            ],
            "patches": [],
            "evidence": []
        }))
        .expect("serialize ad-hoc run"),
    )
    .expect("write ad-hoc run");

    let mut state = AppState::new(
        workspace,
        Buffer::empty("x", None),
        Buffer::empty("n", None),
    );
    state.agents.dock_tab = AgentOpsTab::Evidence;
    state.agents.selected_mission = None;
    state.agents.selected_agent = Some("gpt-5.4".into());
    state.agents.messages.clear();
    state.agents.patches.clear();
    state.agents.evidence.clear();

    let lines = current_lines_for_width(&state, 96);
    assert!(lines.iter().any(|line| line.contains("thread-adhoc")));
    assert!(lines
        .iter()
        .any(|line| line.contains("Saved ad-hoc reply from disk")));
    assert!(lines
        .iter()
        .any(|line| line.contains(".nit/agents/ad-hoc/gpt-5_4/run.json")));
}

#[test]
fn role_label_canonicalizes_computational_research_display_only() {
    assert_eq!(
        table_role_label("computational research"),
        "computational-research"
    );
    assert_eq!(
        table_role_label("Computational Research"),
        "computational-research"
    );
    assert_eq!(
        table_role_label("computational-research"),
        "computational-research"
    );
    assert_eq!(table_role_label("Planner"), "Planner");
}

#[test]
fn swarm_clone_display_label_omits_mission_id_and_compacts_suffix() {
    let label = swarm_clone_display_label("planner#swarm-mis-001-clone-01", Some("propose"))
        .expect("clone label");
    assert_eq!(label, "clone 01 [propose]");
    assert!(!label.contains("mis-001"));
}

#[test]
fn roster_header_uses_compact_template_and_backend_labels() {
    let mut state = AppState::new(
        std::env::temp_dir(),
        Buffer::empty("x", None),
        Buffer::empty("n", None),
    );
    state.agents.dock_tab = AgentOpsTab::Roster;

    let lines = current_lines_for_width(&state, 72);
    assert_eq!(lines[1], "  Codex    not found  idle");
    assert_eq!(lines[2], "  Claude   not found  idle");
    assert_eq!(lines[3], "  Gemini   not found  idle");
    assert_eq!(lines[4], "  Local    built-in  idle");
    assert!(lines[roster_swarm_template_line_idx(&state)].starts_with(" Template:"));
    assert!(lines[roster_swarm_mission_line_idx(&state)].starts_with(" Mission:"));
}

#[test]
fn roster_header_marks_detected_backends_active() {
    let mut state = AppState::new(
        std::env::temp_dir(),
        Buffer::empty("x", None),
        Buffer::empty("n", None),
    );
    state.agents.dock_tab = AgentOpsTab::Roster;
    state.agents.claude_cli_available = true;
    state.agents.gemini_cli_available = true;

    let lines = current_lines_for_width(&state, 72);
    assert_eq!(lines[2], "  Claude   available  active");
    assert_eq!(lines[3], "  Gemini   available  active");
}

#[test]
fn roster_backend_group_rows_render_plain_backend_labels() {
    let mut state = AppState::new(
        std::env::temp_dir(),
        Buffer::empty("x", None),
        Buffer::empty("n", None),
    );
    state.agents.dock_tab = AgentOpsTab::Roster;
    state.agents.codex_cli_available = true;
    state
        .agents
        .agents
        .push(idle_lane("gpt-5.4", "Codex", AgentLaneKind::Codex));

    let lines = current_lines_for_width(&state, 96);

    assert!(lines
        .iter()
        .any(|line| line.contains(&format!("{} Codex", tree_closed_glyph()))));
    assert!(!lines
        .iter()
        .any(|line| line.contains(&format!("{} Codex", arrow_glyph()))));
    assert!(!lines.iter().any(|line| line.contains("gpt-5.4")));
}

#[test]
fn roster_backend_rows_expand_and_collapse_model_lists() {
    let mut state = AppState::new(
        std::env::temp_dir(),
        Buffer::empty("x", None),
        Buffer::empty("n", None),
    );
    state.agents.dock_tab = AgentOpsTab::Roster;
    state
        .agents
        .agents
        .push(idle_lane("gpt-5.4", "Codex", AgentLaneKind::Codex));

    let collapsed = current_lines_for_width(&state, 96);
    assert!(collapsed
        .iter()
        .any(|line| line.contains(&format!("{} Codex", tree_closed_glyph()))));
    assert!(!collapsed.iter().any(|line| line.contains("gpt-5.4")));

    state
        .agents
        .roster_expanded_backend_kinds
        .insert(nit_core::AgentLaneKind::Codex);

    let expanded = current_lines_for_width(&state, 96);
    assert!(expanded
        .iter()
        .any(|line| line.contains(&format!("{} Codex", tree_open_glyph()))));
    assert!(expanded.iter().any(|line| line.contains("gpt-5.4")));
}

#[test]
fn roster_selected_backend_row_shows_cursor_marker() {
    let mut state = AppState::new(
        std::env::temp_dir(),
        Buffer::empty("x", None),
        Buffer::empty("n", None),
    );
    state.agents.dock_tab = AgentOpsTab::Roster;
    state
        .agents
        .agents
        .push(idle_lane("gpt-5.4", "Codex", AgentLaneKind::Codex));

    let lines = current_lines_for_width(&state, 96);

    assert!(lines.iter().any(|line| line.contains(&format!(
        "{}{} Codex",
        cursor_glyph(),
        tree_closed_glyph()
    ))));
}

#[test]
fn roster_lists_discovered_claude_and_gemini_models_as_rows() {
    let mut state = AppState::new(
        std::env::temp_dir(),
        Buffer::empty("x", None),
        Buffer::empty("n", None),
    );
    state.agents.dock_tab = AgentOpsTab::Roster;
    state.agents.claude_cli_available = true;
    state.agents.gemini_cli_available = true;
    state
        .agents
        .roster_expanded_backend_kinds
        .insert(nit_core::AgentLaneKind::Claude);
    state
        .agents
        .roster_expanded_backend_kinds
        .insert(nit_core::AgentLaneKind::Gemini);
    state.agents.agents.push(idle_lane(
        "claude-sonnet-4",
        "Claude",
        AgentLaneKind::Claude,
    ));
    state
        .agents
        .agents
        .push(idle_lane("gemini-2.5-pro", "Gemini", AgentLaneKind::Gemini));

    let lines = current_lines_for_width(&state, 96);

    assert!(lines
        .iter()
        .any(|line| line.contains("  Claude   available  active")));
    assert!(lines
        .iter()
        .any(|line| line.contains("  Gemini   available  active")));
    assert!(lines.iter().any(|line| line.contains("claude-sonnet-4")));
    assert!(lines.iter().any(|line| line.contains("gemini-2.5-pro")));
}

#[test]
fn roster_shows_priority_checkbox_for_supported_backend_models() {
    let mut state = AppState::new(
        std::env::temp_dir(),
        Buffer::empty("x", None),
        Buffer::empty("n", None),
    );
    state.agents.dock_tab = AgentOpsTab::Roster;
    state.agents.codex_cli_available = true;
    state.agents.claude_cli_available = true;
    state.agents.gemini_cli_available = true;
    state
        .agents
        .roster_expanded_backend_kinds
        .insert(nit_core::AgentLaneKind::Codex);
    state
        .agents
        .roster_expanded_backend_kinds
        .insert(nit_core::AgentLaneKind::Claude);
    state
        .agents
        .roster_expanded_backend_kinds
        .insert(nit_core::AgentLaneKind::Gemini);
    state
        .agents
        .agents
        .push(idle_lane("gpt-5.4", "Codex", AgentLaneKind::Codex));
    state.agents.agents.push(idle_lane(
        "claude-sonnet-4",
        "Claude",
        AgentLaneKind::Claude,
    ));
    state
        .agents
        .agents
        .push(idle_lane("gemini-2.5-pro", "Gemini", AgentLaneKind::Gemini));

    let lines = current_lines_for_width(&state, 120);

    assert!(lines.iter().any(|line| line.contains("[ ] gpt-5.4")));
    assert!(lines
        .iter()
        .any(|line| line.contains("[ ] claude-sonnet-4")));
    assert!(lines.iter().any(|line| line.contains("[ ] gemini-2.5-pro")));
}

#[test]
fn roster_header_uses_bold_primary_color_for_active_codex_backend() {
    let mut state = AppState::new(
        std::env::temp_dir(),
        Buffer::empty("x", None),
        Buffer::empty("n", None),
    );
    state.agents.dock_tab = AgentOpsTab::Roster;
    state.agents.codex_cli_available = true;

    let theme = Theme::default();
    let lines = current_lines_for_width(&state, 72);
    let styled = roster_styled_line(&state, 1, &lines[1], 72, &theme);

    assert_eq!(styled.spans[1].style.fg, Some(theme.title));
    assert!(styled.spans[1].style.add_modifier.contains(Modifier::BOLD));
    assert_eq!(styled.spans[5].style.fg, Some(theme.title));
    assert!(styled.spans[5].style.add_modifier.contains(Modifier::BOLD));
}

#[test]
fn roster_header_line_1_is_not_misstyled_as_a_divider_by_ops_styled_line() {
    let mut state = AppState::new(
        std::env::temp_dir(),
        Buffer::empty("x", None),
        Buffer::empty("n", None),
    );
    state.agents.dock_tab = AgentOpsTab::Roster;
    state.agents.codex_cli_available = true;

    let theme = Theme::default();
    let lines = current_lines_for_width(&state, 72);
    let styled = ops_styled_line(&state, 1, &lines[1], 72, &theme);

    // A divider row would be a single span with dim border styling. The roster backend row is
    // a multi-span line with an accented backend name.
    assert!(styled.spans.len() > 1);
    assert_eq!(styled.spans[1].style.fg, Some(theme.title));
    assert!(styled.spans[1].style.add_modifier.contains(Modifier::BOLD));
}

#[test]
fn roster_backend_palette_stays_on_primary_theme_colors() {
    let theme = Theme::default();

    assert_eq!(
        roster_inventory_backend_accent(BackendInventoryBackend::Codex, &theme),
        theme.title
    );
    assert_eq!(
        roster_lane_backend_accent(nit_core::AgentLaneKind::Codex, &theme),
        theme.title
    );
    assert_eq!(
        roster_inventory_backend_accent(BackendInventoryBackend::Gemini, &theme),
        theme.accent
    );
    assert_eq!(
        roster_inventory_backend_accent(BackendInventoryBackend::Local, &theme),
        theme.border
    );
    assert_eq!(
        roster_lane_backend_accent(nit_core::AgentLaneKind::Gemini, &theme),
        theme.accent
    );
    assert_eq!(
        roster_lane_backend_accent(nit_core::AgentLaneKind::Mock, &theme),
        theme.border
    );
    assert_ne!(
        roster_inventory_backend_accent(BackendInventoryBackend::Codex, &theme),
        theme.title_focused
    );
    assert_ne!(
        roster_inventory_backend_accent(BackendInventoryBackend::Gemini, &theme),
        theme.success
    );
    assert_ne!(
        roster_inventory_backend_accent(BackendInventoryBackend::Local, &theme),
        theme.seed.accent_2
    );
}

#[test]
fn diagnostics_view_summarizes_state_and_dedupes_runtime_info_noise() {
    let mut state = AppState::new(
        std::env::temp_dir(),
        Buffer::empty("x", None),
        Buffer::empty("n", None),
    );
    state.app_kind = AppKind::Games;
    state.games.status = nit_core::GamesStatus::Running;
    state.games.runtime.backend = nit_games::RuntimeAcceleratorBackend::Metal;
    state.games.runtime.metal_matches = 8192;
    state.games.runtime.cpu_matches = 7;
    state
        .agents
        .diag_events
        .push(nit_core::AgentDiagnosticEvent {
            severity: AgentAlertSeverity::Info,
            source: "runtime".into(),
            message: "nit: Log file: /tmp/nit.log".into(),
            at: "t+0".into(),
        });
    state
        .agents
        .diag_events
        .push(nit_core::AgentDiagnosticEvent {
            severity: AgentAlertSeverity::Warn,
            source: "codex".into(),
            message: "[planner] cleared invalid thread context".into(),
            at: "t+1".into(),
        });
    state
        .logs
        .push("2026-03-09T18:26:35.337654Z INFO nit: Log file: /tmp/nit.log");

    let lines = diagnostics_lines(&state, 96);

    assert!(lines.iter().any(|line| line.contains(" Summary")));
    assert!(lines.iter().any(|line| line.contains("games/running")));
    assert!(lines
        .iter()
        .any(|line| line.contains("metal active (gpu 8192 / cpu 7)")));
    assert!(lines.iter().any(|line| line.contains(" Recent issues")));
    assert!(lines
        .iter()
        .any(|line| line.contains("WARN  t+1") && line.contains("codex")));
    assert!(!lines
        .iter()
        .any(|line| line.contains("t+0") && line.contains("runtime")));
    assert!(lines
        .iter()
        .any(|line| line.contains("18:26:35 INFO nit Log file: /tmp/nit.log")));
}

#[test]
fn template_hit_targets_all_buttons() {
    for label in ["lab", "parallel", "bulk"] {
        let needle = format!(" {label} ");
        let start = super::ROSTER_SWARM_TEMPLATE_LINE
            .find(needle.as_str())
            .expect("template button");
        assert_eq!(roster_swarm_template_hit(start), Some(label));
        assert_eq!(
            roster_swarm_template_hit(start + needle.len().saturating_sub(1)),
            Some(label)
        );
    }
}

#[test]
fn mission_hit_targets_all_buttons() {
    for (label, value) in [
        ("auto", "auto"),
        ("general", "general"),
        ("research", "research"),
        ("computational", "computational-research"),
    ] {
        let needle = format!(" {label} ");
        let start = super::ROSTER_SWARM_MISSION_LINE
            .find(needle.as_str())
            .expect("mission button");
        assert_eq!(roster_swarm_mission_hit(start), Some(value));
        assert_eq!(
            roster_swarm_mission_hit(start + needle.len().saturating_sub(1)),
            Some(value)
        );
    }
}

#[test]
fn roster_column_widths_prioritize_pri_role_with_stable_total_width() {
    let widths = roster_column_widths(80);
    assert_eq!(widths.len(), 5);
    assert_eq!(widths.iter().sum::<usize>() + 4, 79);
    assert_eq!(widths[4], 10);
    assert!(widths[0] > widths[4]);
}

#[test]
fn artifacts_history_entries_list_archived_runs_newest_first() {
    let workspace = unique_test_workspace("artifacts-history-list");
    let mut state = AppState::new(
        workspace.clone(),
        Buffer::empty("x", None),
        Buffer::empty("n", None),
    );
    state.agents.selected_mission = Some("mis-501".into());
    state.agents.missions.push(MissionRecord {
        id: "mis-501".into(),
        title: "Archive history".into(),
        phase: MissionPhase::Execute,
        swarm: false,
        assigned_agents: vec!["codex".into()],
        status: "DONE".into(),
        updated_at: "t+9".into(),
    });

    for (dir_name, updated_at) in [
        ("00000000000000000002", "t+2"),
        ("00000000000000000001", "t+1"),
    ] {
        let run_dir = workspace
            .join(".nit/agents/runs/mis-501/history")
            .join(dir_name);
        fs::create_dir_all(&run_dir).expect("history dir");
        fs::write(
            run_dir.join("run.json"),
            serde_json::json!({
                "id": "mis-501",
                "updated_at": updated_at,
                "messages": [{"at":"t+1","channel":"Agent","agent_id":"codex","mission_id":"mis-501","text":"saved"}],
                "patches": [],
                "evidence": []
            })
            .to_string(),
        )
        .expect("write run json");
    }

    let entries = artifacts_history_entries(&state);
    assert_eq!(entries.len(), 3);
    assert_eq!(entries[1].archive_micros, Some(2));
    assert_eq!(entries[2].archive_micros, Some(1));
    assert!(entries[1].label.starts_with("saved "));
    assert!(entries[2].label.starts_with("saved "));
}

#[test]
fn saved_run_relative_label_prefers_human_readable_units() {
    let now_micros = 10 * 24 * 60 * 60 * 1_000_000u128;
    assert_eq!(
        format_saved_run_relative_label_from_micros(Some(now_micros), now_micros),
        "saved just now"
    );
    assert_eq!(
        format_saved_run_relative_label_from_micros(
            Some(now_micros.saturating_sub(20 * 60 * 1_000_000)),
            now_micros
        ),
        "saved 20m ago"
    );
    assert_eq!(
        format_saved_run_relative_label_from_micros(
            Some(now_micros.saturating_sub(3 * 60 * 60 * 1_000_000)),
            now_micros
        ),
        "saved 3h ago"
    );
    assert_eq!(
        format_saved_run_relative_label_from_micros(
            Some(now_micros.saturating_sub(5 * 24 * 60 * 60 * 1_000_000)),
            now_micros
        ),
        "saved 5d ago"
    );
}

#[test]
fn saved_run_detail_label_includes_absolute_and_run_timestamp() {
    let detail = saved_run_detail_label(
        Some(1_741_608_000_000_000),
        "2026-03-09T18:26:35Z",
        "3 msgs · 1 patches · 0 evidence",
    );
    assert!(detail.contains("UTC"));
    assert!(detail.contains("run 2026-03-09T18:26:35Z"));
    assert!(detail.contains("3 msgs · 1 patches · 0 evidence"));
}

#[test]
fn artifacts_history_visible_entries_apply_date_filter() {
    let workspace = unique_test_workspace("artifacts-history-filter");
    let mut state = AppState::new(
        workspace.clone(),
        Buffer::empty("x", None),
        Buffer::empty("n", None),
    );
    state.agents.selected_mission = Some("mis-502".into());
    state.agents.missions.push(MissionRecord {
        id: "mis-502".into(),
        title: "Archive filter".into(),
        phase: MissionPhase::Execute,
        swarm: false,
        assigned_agents: vec!["codex".into()],
        status: "DONE".into(),
        updated_at: "t+9".into(),
    });

    let now_micros = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_micros();
    let recent = now_micros.saturating_sub(6 * 60 * 60 * 1_000_000);
    let older = now_micros.saturating_sub(3 * 24 * 60 * 60 * 1_000_000);
    for archive_micros in [older, recent] {
        let run_dir = workspace
            .join(".nit/agents/runs/mis-502/history")
            .join(format!("{archive_micros:020}"));
        fs::create_dir_all(&run_dir).expect("history dir");
        fs::write(
            run_dir.join("run.json"),
            serde_json::json!({
                "id": "mis-502",
                "updated_at": "t+2",
                "messages": [{"at":"t+1","channel":"Agent","agent_id":"codex","mission_id":"mis-502","text":"saved"}],
                "patches": [],
                "evidence": []
            })
            .to_string(),
        )
        .expect("write run json");
    }

    state.agents.artifacts_history_filter = SavedRunHistoryFilter::LastDay;
    let entries = artifacts_history_visible_entries(&state);
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].label, "current / latest saved run");
    assert_eq!(entries[1].archive_micros, Some(recent));
    assert!(entries[1].label.starts_with("saved "));
}

#[test]
fn artifacts_view_uses_selected_archived_run_over_live_context() {
    let workspace = unique_test_workspace("artifacts-history-selected");
    let mut state = AppState::new(
        workspace.clone(),
        Buffer::empty("x", None),
        Buffer::empty("n", None),
    );
    state.agents.dock_tab = AgentOpsTab::Evidence;
    state.agents.selected_mission = Some("mis-601".into());
    state.agents.missions.push(MissionRecord {
        id: "mis-601".into(),
        title: "Selected archive".into(),
        phase: MissionPhase::Execute,
        swarm: false,
        assigned_agents: vec!["codex".into()],
        status: "DONE".into(),
        updated_at: "t+9".into(),
    });
    state.agents.messages.push(AgentMessage {
        at: "t+3".into(),
        channel: AgentChannel::Agent,
        agent_id: Some("codex".into()),
        mission_id: Some("mis-601".into()),
        text: "live reply".into(),
        prompt_msg_idx: None,
        kind: None,
    });

    let run_dir = workspace
        .join(".nit")
        .join("agents")
        .join("runs")
        .join("mis-601")
        .join("history")
        .join("00000000000000000003");
    fs::create_dir_all(&run_dir).expect("history dir");
    let archived_run = serde_json::json!({
        "id": "mis-601",
        "updated_at": "t+7",
        "messages": [
            {"at":"t+1","channel":"Agent","agent_id":null,"mission_id":"mis-601","text":"prompt"},
            {"at":"t+2","channel":"Agent","agent_id":"codex","mission_id":"mis-601","text":"archived reply"}
        ],
        "patches": [],
        "evidence": []
    });
    let run_path = run_dir.join("run.json");
    fs::write(&run_path, archived_run.to_string()).expect("write run");
    state.agents.artifacts_selected_saved_run_path = Some(run_path.to_string_lossy().to_string());

    let lines = current_lines_for_width(&state, 96);
    assert!(lines.iter().any(|line| line.contains("saved ")));
    assert!(lines.iter().any(|line| line.contains("archived reply")));
    assert!(!lines.iter().any(|line| line.contains("live reply")));
}

// --- Roster + mission truncation -------------------------------------------
//
// These tests cover the truncation logic added so a 256-clone swarm doesn't
// blow out the dock: per-backend cap, total-count surfacing,
// selection-preservation, mission "(+N more)" row, the running-first
// priority sort, and the env-var escape hatch.

fn make_state_with_codex_lanes(count: usize) -> AppState {
    let mut state = AppState::new(
        std::env::temp_dir(),
        Buffer::empty("x", None),
        Buffer::empty("n", None),
    );
    state.agents.agents.clear();
    for i in 0..count {
        state.agents.agents.push(idle_lane(
            &format!("codex-{i:02}"),
            "codex",
            AgentLaneKind::Codex,
        ));
    }
    state
}

#[test]
fn roster_grouped_agent_indices_caps_visible_per_backend() {
    let state = make_state_with_codex_lanes(20);
    let groups = roster_grouped_agent_indices(&state);
    let codex = groups
        .iter()
        .find(|(kind, _)| *kind == AgentLaneKind::Codex)
        .expect("codex group present");
    assert_eq!(codex.1.len(), ROSTER_VISIBLE_AGENTS_PER_BACKEND);
}

#[test]
fn roster_grouped_agent_indices_returns_full_list_when_under_cap() {
    let state = make_state_with_codex_lanes(5);
    let groups = roster_grouped_agent_indices(&state);
    let codex = groups
        .iter()
        .find(|(kind, _)| *kind == AgentLaneKind::Codex)
        .expect("codex group present");
    assert_eq!(codex.1.len(), 5);
}

#[test]
fn roster_backend_total_counts_returns_full_count() {
    let state = make_state_with_codex_lanes(20);
    let counts = roster_backend_total_counts(&state);
    let codex_count = counts
        .iter()
        .find_map(|(kind, n)| (*kind == AgentLaneKind::Codex).then_some(*n))
        .expect("codex total");
    assert_eq!(codex_count, 20);
}

#[test]
fn roster_grouped_agent_indices_keeps_selected_agent_visible() {
    let mut state = make_state_with_codex_lanes(20);
    // Select an agent past the cap (index 18, well beyond the 12-visible
    // window). Truncation must still produce a list that contains 18.
    state.agents.roster_selected = 18;
    let groups = roster_grouped_agent_indices(&state);
    let codex = &groups
        .iter()
        .find(|(kind, _)| *kind == AgentLaneKind::Codex)
        .expect("codex group present")
        .1;
    assert_eq!(codex.len(), ROSTER_VISIBLE_AGENTS_PER_BACKEND);
    assert!(
        codex.contains(&18),
        "selected agent index 18 must be retained when truncating; got {codex:?}"
    );
}

fn fake_active_turn() -> nit_core::AgentTurnState {
    let now = std::time::Instant::now();
    nit_core::AgentTurnState {
        started_at: now,
        last_heartbeat_at: now,
        last_output_at: now,
        stage: None,
    }
}

#[test]
fn roster_grouped_agent_indices_promotes_running_agents_first() {
    let mut state = make_state_with_codex_lanes(20);
    // Mark agent at index 15 as having an active turn. Truncation should
    // bubble it into the visible window even though it sits past the cap
    // in roster order.
    state
        .agents
        .active_turns
        .insert("codex-15".to_string(), fake_active_turn());
    let groups = roster_grouped_agent_indices(&state);
    let codex = &groups
        .iter()
        .find(|(kind, _)| *kind == AgentLaneKind::Codex)
        .expect("codex group present")
        .1;
    assert!(
        codex.contains(&15),
        "running agent (idx 15) must surface in visible window; got {codex:?}"
    );
}

#[test]
fn roster_running_priority_orders_states_correctly() {
    let mut state = make_state_with_codex_lanes(4);
    state.agents.agents[0].status = AgentStatus::Idle;
    state.agents.agents[1].status = AgentStatus::Running;
    state.agents.agents[2].status = AgentStatus::Idle;
    state.agents.agents[3].status = AgentStatus::Error;
    state
        .agents
        .active_turns
        .insert("codex-02".to_string(), fake_active_turn());
    // codex-02 has an active turn ⇒ priority 0 (highest).
    assert_eq!(roster_running_priority(&state, 2), 0);
    // codex-01 has Running status, no active turn ⇒ priority 2.
    assert_eq!(roster_running_priority(&state, 1), 2);
    // codex-00 is Idle ⇒ priority 3.
    assert_eq!(roster_running_priority(&state, 0), 3);
    // codex-03 is Error ⇒ priority 4 (lowest).
    assert_eq!(roster_running_priority(&state, 3), 4);
}

// Pins the bug where swarm clones from a non-first parent lane rendered
// directly under a sibling base lane in the truncated roster view,
// because the priority sort placed an idle sibling between the running
// parent and its idle clones. The two-arrow clone glyph (`↳ ↳`) made
// them look like children of the wrong lane.
#[test]
fn roster_groups_swarm_clones_with_their_parent_under_truncation() {
    use crate::swarm::SWARM_CLONE_INFIX;

    let mut state = AppState::new(
        std::env::temp_dir(),
        Buffer::empty("x", None),
        Buffer::empty("n", None),
    );
    state.agents.agents.clear();
    // Roster shape that reproduces the bug: an idle base lane (haiku) is
    // inserted BEFORE the planner (opus), then opus, then 10 idle clones
    // of opus, then one more base lane (sonnet). 13 total > 12 cap so
    // truncation kicks in.
    state.agents.agents.push(idle_lane(
        "claude-haiku-4-5",
        "claude",
        AgentLaneKind::Claude,
    ));
    state.agents.agents.push(idle_lane(
        "claude-opus-4-7",
        "claude",
        AgentLaneKind::Claude,
    ));
    for i in 1..=10 {
        let clone_id = format!("claude-opus-4-7{SWARM_CLONE_INFIX}mis-001-clone-{i:02}");
        state
            .agents
            .agents
            .push(idle_lane(&clone_id, "claude", AgentLaneKind::Claude));
    }
    state.agents.agents.push(idle_lane(
        "claude-sonnet-4-6",
        "claude",
        AgentLaneKind::Claude,
    ));
    // opus is the running planner.
    state
        .agents
        .active_turns
        .insert("claude-opus-4-7".to_string(), fake_active_turn());

    let groups = roster_grouped_agent_indices(&state);
    let claude = &groups
        .iter()
        .find(|(kind, _)| *kind == AgentLaneKind::Claude)
        .expect("claude group present")
        .1;
    assert_eq!(claude.len(), ROSTER_VISIBLE_AGENTS_PER_BACKEND);

    // Translate the visible-index sequence back into agent ids so the
    // assertion failure prints a readable order on regression.
    let visible_ids: Vec<&str> = claude
        .iter()
        .map(|idx| state.agents.agents[*idx].id.as_str())
        .collect();

    // Opus is the running planner, so it must be first (running priority
    // outranks any idle base lane).
    assert_eq!(
        visible_ids[0], "claude-opus-4-7",
        "running planner must lead; got {visible_ids:?}"
    );

    // The 10 clones of opus must immediately follow opus, NOT be
    // separated from it by haiku or any other base lane.
    for (i, id) in visible_ids.iter().enumerate().skip(1).take(10) {
        let expected = format!("claude-opus-4-7{SWARM_CLONE_INFIX}mis-001-clone-{i:02}");
        assert_eq!(
            *id, expected.as_str(),
            "clones must sit adjacent to their parent; row {i} = {id:?}, expected {expected:?}, full order = {visible_ids:?}"
        );
    }

    // The remaining visible row is a non-clone Claude base lane (the
    // other idle siblings) — verifies that base lanes still appear after
    // the clone group rather than getting promoted ahead of it.
    let tail = visible_ids[11];
    assert!(
        tail == "claude-haiku-4-5" || tail == "claude-sonnet-4-6",
        "tail row must be a sibling base lane; got {tail:?} in {visible_ids:?}"
    );
}

#[test]
fn mission_visible_agent_lines_caps_above_threshold() {
    let mut mission = MissionRecord {
        id: "mis-001".into(),
        title: "test".into(),
        phase: MissionPhase::Plan,
        status: "in progress".into(),
        swarm: true,
        updated_at: String::new(),
        assigned_agents: (0..20).map(|i| format!("codex-{i:02}")).collect(),
    };
    // 20 agents → 8 visible + 1 (+N more) = 9 rows.
    assert_eq!(
        mission_visible_agent_lines(&mission),
        MISSION_VISIBLE_AGENTS_MAX + 1
    );

    mission.assigned_agents.truncate(5);
    // 5 agents (under the cap) → 5 rows, no overflow indicator.
    assert_eq!(mission_visible_agent_lines(&mission), 5);

    mission.assigned_agents.clear();
    // Empty mission still reserves a row for the "--" placeholder.
    assert_eq!(mission_visible_agent_lines(&mission), 1);
}

#[test]
fn dag_lines_show_per_dep_budget_for_full_output_with_high_fanin() {
    // Regression: bulk-template judge/integrate tasks with many deps get
    // their per-dep budget compressed below the per-dep ceiling, but the
    // operator can't see that without running the swarm and watching
    // proposals get truncated. The DAG dashboard now surfaces the budget
    // directly so the cost is visible at plan time.
    let dashboard = SwarmDashboardView {
        mission_id: "mis-bulk-stress".into(),
        template: "bulk".into(),
        phase: "EXEC".into(),
        done: 0,
        failed: 0,
        skipped: 0,
        running: 1,
        queued: 0,
        pending: 12,
        tasks: vec![SwarmTaskDashboardRow {
            id: "judge-1".into(),
            title: "Compare proposals".into(),
            role: Some("judge".into()),
            agent_id: "claude-opus-4-7".into(),
            state: "Pending".into(),
            deps: (0..12).map(|i| format!("propose-{i:02}")).collect(),
            blocked_on: Vec::new(),
            writes: false,
            done_when: Some("Best proposal selected".into()),
            output_present: false,
        }],
        gate_bundle: None,
        gates: Vec::new(),
    };
    let lines = dag_lines_for_dashboard(&dashboard, 80);
    // 12 deps on a judge role → 240_000 / 12 = 20_000 chars/dep = ~20KB.
    assert!(
        lines.iter().any(|l| l.contains("budget: ~20KB/dep")),
        "expected per-dep budget annotation in DAG output, got: {lines:#?}"
    );
}

#[test]
fn dag_lines_flag_shallow_budget_when_critical() {
    // 50 proposers compresses the per-dep budget to 4.8KB — well below the
    // 8KB threshold where proposals carry usable reasoning. Surface the
    // "shallow" warning so the operator knows the bulk run will be
    // dominated by headers, not analysis.
    let dashboard = SwarmDashboardView {
        mission_id: "mis-bulk-overscale".into(),
        template: "bulk".into(),
        phase: "EXEC".into(),
        done: 0,
        failed: 0,
        skipped: 0,
        running: 1,
        queued: 0,
        pending: 50,
        tasks: vec![SwarmTaskDashboardRow {
            id: "integrate-1".into(),
            title: "Integrate proposals".into(),
            role: Some("integrate".into()),
            agent_id: "claude-opus-4-7".into(),
            state: "Pending".into(),
            deps: (0..50).map(|i| format!("propose-{i:02}")).collect(),
            blocked_on: Vec::new(),
            writes: true,
            done_when: Some("Changes integrated".into()),
            output_present: false,
        }],
        gate_bundle: None,
        gates: Vec::new(),
    };
    let lines = dag_lines_for_dashboard(&dashboard, 80);
    assert!(
        lines.iter().any(|l| l.contains("shallow")),
        "expected shallow-budget warning in DAG output, got: {lines:#?}"
    );
}

#[test]
fn dag_lines_omit_budget_for_few_deps() {
    // With ≤ 5 deps, the per-dep budget hits the per-dep ceiling (48KB).
    // Showing "budget: ~48KB/dep" then would be noise — every dep gets
    // the maximum, no truncation. Annotation must be suppressed.
    let dashboard = SwarmDashboardView {
        mission_id: "mis-small-bulk".into(),
        template: "bulk".into(),
        phase: "EXEC".into(),
        done: 0,
        failed: 0,
        skipped: 0,
        running: 1,
        queued: 0,
        pending: 4,
        tasks: vec![SwarmTaskDashboardRow {
            id: "judge-1".into(),
            title: "Compare proposals".into(),
            role: Some("judge".into()),
            agent_id: "claude-opus-4-7".into(),
            state: "Pending".into(),
            deps: vec![
                "propose-01".into(),
                "propose-02".into(),
                "propose-03".into(),
                "propose-04".into(),
            ],
            blocked_on: Vec::new(),
            writes: false,
            done_when: Some("Best selected".into()),
            output_present: false,
        }],
        gate_bundle: None,
        gates: Vec::new(),
    };
    let lines = dag_lines_for_dashboard(&dashboard, 80);
    assert!(
        !lines.iter().any(|l| l.contains("budget:")),
        "no budget annotation expected for small bulk runs, got: {lines:#?}"
    );
}

#[test]
fn parse_roster_truncation_disabled_handles_common_inputs() {
    // Treated as "still truncate":
    assert!(!parse_roster_truncation_disabled(None));
    assert!(!parse_roster_truncation_disabled(Some("")));
    assert!(!parse_roster_truncation_disabled(Some("   ")));
    assert!(!parse_roster_truncation_disabled(Some("0")));
    assert!(!parse_roster_truncation_disabled(Some("false")));
    assert!(!parse_roster_truncation_disabled(Some("FALSE")));
    assert!(!parse_roster_truncation_disabled(Some("False")));

    // Treated as "disable truncation":
    assert!(parse_roster_truncation_disabled(Some("1")));
    assert!(parse_roster_truncation_disabled(Some("true")));
    assert!(parse_roster_truncation_disabled(Some("yes")));
    assert!(parse_roster_truncation_disabled(Some("anything-else")));
    assert!(parse_roster_truncation_disabled(Some(" 1 "))); // whitespace stripped
}
