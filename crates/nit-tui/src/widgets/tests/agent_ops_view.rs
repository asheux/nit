use super::{
    append_swarm_artifact_lines, arrow_glyph, artifacts_history_entries,
    artifacts_history_visible_entries, current_lines_for_width, cursor_glyph,
    dag_lines_for_dashboard, diagnostics_lines, format_saved_run_relative_label_from_micros,
    ops_styled_line, roster_column_widths, roster_inventory_backend_accent,
    roster_lane_backend_accent, roster_styled_line, roster_swarm_mission_hit,
    roster_swarm_mission_line_idx, roster_swarm_template_hit, roster_swarm_template_line_idx,
    saved_run_detail_label, swarm_clone_display_label, table_role_label, tree_closed_glyph,
    tree_open_glyph, BackendInventoryBackend,
};
use crate::swarm::{
    GateReport, GateReportGate, SwarmDashboardView, SwarmGateDashboardRow, SwarmPersistenceView,
    SwarmTaskDashboardRow, SwarmTaskPersistenceView,
};
use crate::theme::Theme;
use nit_core::{
    AgentAlertSeverity, AgentChannel, AgentMessage, AgentOpsTab, AgentStatus, AppKind, AppState,
    Buffer, MissionPhase, MissionRecord, PatchProposal, PatchStatus, SavedRunHistoryFilter,
};
use ratatui::style::Modifier;
use std::{
    fs,
    time::{SystemTime, UNIX_EPOCH},
};

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
        },
        AgentMessage {
            at: "10:01".into(),
            channel: AgentChannel::Agent,
            agent_id: Some("gpt-5.4".into()),
            mission_id: Some("mis-201".into()),
            text: "Bulk orchestration looks mostly correct; docs need follow-up.".into(),
            prompt_msg_idx: None,
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
        },
        AgentMessage {
            at: "11:01".into(),
            channel: AgentChannel::Agent,
            agent_id: Some("gpt-5.4".into()),
            mission_id: None,
            text: "I checked the repo health; no flickering issue was reproduced.".into(),
            prompt_msg_idx: None,
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
    state.agents.agents.push(nit_core::AgentLane {
        id: "gpt-5.4".into(),
        role: "gpt-5.4".into(),
        lane: "Codex".into(),
        kind: nit_core::AgentLaneKind::Codex,
        status: AgentStatus::Idle,
        heartbeat_age_secs: 0,
        queue_len: 0,
        current_mission: None,
        last_message: String::new(),
    });

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
    state.agents.agents.push(nit_core::AgentLane {
        id: "gpt-5.4".into(),
        role: "gpt-5.4".into(),
        lane: "Codex".into(),
        kind: nit_core::AgentLaneKind::Codex,
        status: AgentStatus::Idle,
        heartbeat_age_secs: 0,
        queue_len: 0,
        current_mission: None,
        last_message: String::new(),
    });

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
    state.agents.agents.push(nit_core::AgentLane {
        id: "gpt-5.4".into(),
        role: "gpt-5.4".into(),
        lane: "Codex".into(),
        kind: nit_core::AgentLaneKind::Codex,
        status: AgentStatus::Idle,
        heartbeat_age_secs: 0,
        queue_len: 0,
        current_mission: None,
        last_message: String::new(),
    });

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
    state.agents.agents.push(nit_core::AgentLane {
        id: "claude-sonnet-4".into(),
        role: "claude-sonnet-4".into(),
        lane: "Claude".into(),
        kind: nit_core::AgentLaneKind::Claude,
        status: nit_core::AgentStatus::Idle,
        heartbeat_age_secs: 0,
        queue_len: 0,
        current_mission: None,
        last_message: String::new(),
    });
    state.agents.agents.push(nit_core::AgentLane {
        id: "gemini-2.5-pro".into(),
        role: "gemini-2.5-pro".into(),
        lane: "Gemini".into(),
        kind: nit_core::AgentLaneKind::Gemini,
        status: nit_core::AgentStatus::Idle,
        heartbeat_age_secs: 0,
        queue_len: 0,
        current_mission: None,
        last_message: String::new(),
    });

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
    state.agents.agents.push(nit_core::AgentLane {
        id: "gpt-5.4".into(),
        role: "gpt-5.4".into(),
        lane: "Codex".into(),
        kind: nit_core::AgentLaneKind::Codex,
        status: nit_core::AgentStatus::Idle,
        heartbeat_age_secs: 0,
        queue_len: 0,
        current_mission: None,
        last_message: String::new(),
    });
    state.agents.agents.push(nit_core::AgentLane {
        id: "claude-sonnet-4".into(),
        role: "claude-sonnet-4".into(),
        lane: "Claude".into(),
        kind: nit_core::AgentLaneKind::Claude,
        status: nit_core::AgentStatus::Idle,
        heartbeat_age_secs: 0,
        queue_len: 0,
        current_mission: None,
        last_message: String::new(),
    });
    state.agents.agents.push(nit_core::AgentLane {
        id: "gemini-2.5-pro".into(),
        role: "gemini-2.5-pro".into(),
        lane: "Gemini".into(),
        kind: nit_core::AgentLaneKind::Gemini,
        status: nit_core::AgentStatus::Idle,
        heartbeat_age_secs: 0,
        queue_len: 0,
        current_mission: None,
        last_message: String::new(),
    });

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
    });

    let run_dir = workspace.join(".nit/agents/runs/mis-601/history/00000000000000000003");
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
