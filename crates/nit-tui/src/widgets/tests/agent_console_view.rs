//! Tests for the agent console view (thread rendering, chat input metrics,
//! breather rows, styled spans). Each test constructs a minimal `AppState`
//! fixture and asserts against the row stream produced by `thread_rows` or
//! `format_message_rows`.

use super::{
    artifact_message_index_for_line, build_pane_thread_rows_with_breathers,
    chat_input_scroll_metrics, chat_input_text_area, ecg_indicator, format_message_rows,
    map_chat_input_point_to_cursor, swarm_exec_label, thread_lines, thread_rows, user_prompt_bg,
    wrap_input_with_cursor, wrap_visual_line, ThreadRow, ThreadRowKind,
};
use crate::swarm::{test_runtime_with_running_tasks, SwarmRuntime, SwarmSize};
use crate::theme::Theme;
use nit_core::{
    AgentBusEvent, AgentChannel, AgentLane, AgentLaneKind, AgentMessage, AgentStatus, AppState,
    Buffer, MissionPhase, MissionRecord, QueuedCodexTurn,
};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier};
use std::path::PathBuf;
use std::time::Instant;
use unicode_width::UnicodeWidthStr;

fn test_state() -> AppState {
    AppState::new(
        PathBuf::new(),
        Buffer::empty("editor", None),
        Buffer::empty("notes", None),
    )
}

/// Baseline `AgentLane` with zeroed counters and empty strings. Tests layer
/// test-specific values on top via `..make_lane(...)` so only the fields the
/// assertion cares about stay at the call site.
fn make_lane(
    id: &str,
    role: &str,
    lane: &str,
    kind: AgentLaneKind,
    status: AgentStatus,
) -> AgentLane {
    AgentLane {
        id: id.into(),
        role: role.into(),
        lane: lane.into(),
        kind,
        status,
        heartbeat_age_secs: 0,
        queue_len: 0,
        current_mission: None,
        shadow: false,
        last_message: String::new(),
    }
}

#[test]
fn wrap_input_with_cursor_expands_tabs_and_keeps_markdown_lines() {
    let markdown = "# Plan\n- item\tone\n```rust\n\tlet x = 1;\n```";
    let (lines, _, _) = wrap_input_with_cursor("", markdown, markdown.chars().count(), 80);
    assert_eq!(lines.len(), 5);
    assert_eq!(lines[0], "# Plan");
    assert_eq!(lines[1], "- item  one");
    assert_eq!(lines[2], "```rust");
    assert_eq!(lines[3], "    let x = 1;");
    assert_eq!(lines[4], "```");
}

#[test]
fn wrap_visual_line_handles_carriage_return_and_tabs() {
    let lines = wrap_visual_line("alpha\rbeta\tgamma", 80);
    assert_eq!(
        lines,
        vec!["alpha".to_string(), "beta    gamma".to_string()]
    );
}

#[test]
fn user_message_renders_right_aligned_bubble() {
    let state = test_state();
    let msg = AgentMessage {
        at: "10:00:00".into(),
        channel: AgentChannel::Agent,
        agent_id: None,
        mission_id: None,
        text: "line one\nline two".into(),
        prompt_msg_idx: None,
        kind: None,
    };
    let rows = format_message_rows(&state, None, &msg, 48);
    assert!(rows.len() >= 5);
    assert!(matches!(rows[0].kind, ThreadRowKind::User));
    // Top padding row + label row.
    assert!(rows[0].text.trim().is_empty());
    assert_eq!(rows[1].text.trim(), "You");
    assert!(rows[2].text.trim_start().starts_with("line one"));
    assert!(rows[3].text.trim_start().starts_with("line two"));
    // Bottom padding row.
    assert!(rows[4].text.trim().is_empty());
}

#[test]
fn ecg_indicator_freezes_when_agent_not_running() {
    let a = ecg_indicator(10, Some("coder"), true, false);
    let b = ecg_indicator(100, Some("coder"), false, false);
    assert_eq!(a, "▁▁▁▁▁▁");
    assert_eq!(b, "▁▁▁▁▁▁");
}

#[test]
fn agent_messages_use_stable_badge_header() {
    let mut state = test_state();
    state.agents.agents.push(AgentLane {
        heartbeat_age_secs: 1,
        queue_len: 1,
        last_message: "active".into(),
        ..make_lane(
            "coder",
            "Coder",
            "Lane B",
            AgentLaneKind::Mock,
            AgentStatus::Running,
        )
    });
    let msg = AgentMessage {
        at: "10:00:00".into(),
        channel: AgentChannel::Agent,
        agent_id: Some("coder".into()),
        mission_id: None,
        text: "working".into(),
        prompt_msg_idx: None,
        kind: None,
    };
    state.agents.messages.push(msg.clone());
    state.metrics.frame_count = 3;
    let first_rows = format_message_rows(
        &state,
        None,
        state.agents.messages.last().expect("reply"),
        80,
    );
    state.metrics.frame_count = 30;
    let second_rows = format_message_rows(
        &state,
        None,
        state.agents.messages.last().expect("reply"),
        80,
    );
    // Combined callout row with badge inline (no separate header).
    assert!(first_rows[0].text.contains("[Coder]"));
    assert!(second_rows[0].text.contains("[Coder]"));
    assert!(first_rows[0].text.contains("done (see ARTIFACTS)"));
    assert!(second_rows[0].text.contains("done (see ARTIFACTS)"));
    assert_eq!(UnicodeWidthStr::width(first_rows[0].text.as_str()), 80);
    assert_eq!(UnicodeWidthStr::width(second_rows[0].text.as_str()), 80);
    assert!(matches!(first_rows[0].kind, ThreadRowKind::ArtifactLink));
    assert!(matches!(second_rows[0].kind, ThreadRowKind::ArtifactLink));
}

#[test]
fn clone_identity_badge_uses_compact_label() {
    let mut state = test_state();
    state.agents.agents.push(AgentLane {
        queue_len: 1,
        current_mission: Some("mis-001".into()),
        ..make_lane(
            "planner#swarm-mis-001-clone-01",
            "Planner (clone 01)",
            "Lane A",
            AgentLaneKind::Mock,
            AgentStatus::Running,
        )
    });

    assert_eq!(
        super::agent_identity_badge(&state, "planner#swarm-mis-001-clone-01"),
        "clone 01"
    );
}

#[test]
fn clone_roster_label_shows_base_model_and_clone_suffix() {
    let agent = AgentLane {
        current_mission: Some("mis-001".into()),
        ..make_lane(
            "planner#swarm-mis-001-clone-01",
            "Planner (clone 01)",
            "Lane A",
            AgentLaneKind::Mock,
            AgentStatus::Idle,
        )
    };

    assert_eq!(super::agent_roster_label(&agent), "planner#clone-01");
}

#[test]
fn breather_rows_show_clone_source_model_name() {
    let mut state = test_state();
    state.agents.messages.clear();
    state.agents.missions.clear();
    state.agents.agents.clear();
    state.agents.active_turns.clear();

    let clone_id = "gpt-5.4#swarm-mis-001-clone-01";
    state.agents.missions.push(MissionRecord {
        id: "mis-001".into(),
        title: "Swarm: clone demo".into(),
        phase: MissionPhase::Execute,
        swarm: true,
        assigned_agents: vec![clone_id.into()],
        status: "EXEC".into(),
        updated_at: "t+0".into(),
    });
    state.agents.selected_mission = Some("mis-001".into());
    state.agents.mission_selected = 0;
    state.agents.selected_agent = Some(clone_id.into());

    state.agents.agents.push(AgentLane {
        queue_len: 1,
        current_mission: Some("mis-001".into()),
        last_message: "active".into(),
        ..make_lane(
            clone_id,
            "GPT-5.4 (clone 01)",
            "Codex",
            AgentLaneKind::Codex,
            AgentStatus::Running,
        )
    });

    let now = Instant::now();
    state.agents.active_turns.insert(
        clone_id.into(),
        nit_core::state::AgentTurnState {
            started_at: now,
            last_heartbeat_at: now,
            last_output_at: now,
            stage: Some("starting".into()),
        },
    );

    state.agents.messages.push(AgentMessage {
        at: "10:00:01".into(),
        channel: AgentChannel::Agent,
        agent_id: None,
        mission_id: Some("mis-001".into()),
        text: "do the work".into(),
        prompt_msg_idx: None,
        kind: None,
    });

    let rows = thread_rows(&state, None, 120, true);
    assert!(rows.iter().any(|row| row.text.contains("gpt-5.4#clone-01")));
    assert!(!rows
        .iter()
        .any(|row| row.text.contains("gpt-5.4#swarm-mis-001-clone-01")));
}

#[test]
fn agent_badge_shown_when_single_agent_context_selected() {
    let mut state = test_state();
    state.agents.selected_mission = None;
    state.agents.selected_agent = Some("coder".into());
    state.agents.agents.push(AgentLane {
        heartbeat_age_secs: 1,
        last_message: "idle".into(),
        ..make_lane(
            "coder",
            "Coder",
            "Lane B",
            AgentLaneKind::Mock,
            AgentStatus::Idle,
        )
    });
    let msg = AgentMessage {
        at: "10:00:00".into(),
        channel: AgentChannel::Agent,
        agent_id: Some("coder".into()),
        mission_id: None,
        text: "hello".into(),
        prompt_msg_idx: None,
        kind: None,
    };
    state.agents.messages.push(msg.clone());
    let rows = format_message_rows(
        &state,
        None,
        state.agents.messages.last().expect("reply"),
        120,
    );
    // Combined callout with badge and status.
    assert!(rows[0].text.contains("[Coder]"));
    assert!(rows[0].text.contains("done (see ARTIFACTS)"));
    assert_eq!(UnicodeWidthStr::width(rows[0].text.as_str()), 120);
    assert!(matches!(rows[0].kind, ThreadRowKind::ArtifactLink));
    assert!(!rows.iter().any(|row| row.text.contains("hello")));
}

#[test]
fn artifact_message_index_for_line_maps_transcript_artifact_row() {
    let mut state = test_state();
    state.agents.selected_agent = Some("coder".into());
    state.agents.agents.push(make_lane(
        "coder",
        "Coder",
        "Lane B",
        AgentLaneKind::Mock,
        AgentStatus::Idle,
    ));
    state.agents.messages.push(AgentMessage {
        at: "10:00:00".into(),
        channel: AgentChannel::Agent,
        agent_id: Some("coder".into()),
        mission_id: None,
        text: "hello".into(),
        prompt_msg_idx: None,
        kind: None,
    });

    // ArtifactLink is now the first row (combined callout).
    assert_eq!(artifact_message_index_for_line(&state, 120, 0), Some(0));
    assert_eq!(artifact_message_index_for_line(&state, 120, 1), None);
}

#[test]
fn swarm_planning_message_stays_plain_done_when_no_artifact_exists() {
    let mut state = AppState::new(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")),
        Buffer::empty("editor", None),
        Buffer::empty("notes", None),
    );
    state.agents.messages.clear();
    state.agents.missions.clear();
    state.agents.agents.clear();
    state.agents.agents.push(make_lane(
        "planner",
        "Planner",
        "Codex",
        AgentLaneKind::Codex,
        AgentStatus::Idle,
    ));

    let mut swarm = SwarmRuntime::default();
    let (mission_id, _dispatches) = swarm
        .start(
            &mut state,
            "planner".into(),
            vec!["planner".into()],
            SwarmSize::Count(2),
            Some("parallel".into()),
            None,
            "root".into(),
        )
        .expect("swarm start");
    let clone_id = format!("planner#swarm-{mission_id}-clone-01");
    let planner_message = format!(
        r#"
```json
{{
  "version": 2,
  "template": "parallel",
  "tasks": [
{{ "id": "t1", "agent_id": "{clone_id}", "title": "T1", "prompt": "DONE t1" }}
  ]
}}
```
"#
    );
    let planner_event = AgentBusEvent::TurnCompleted {
        agent_id: "planner".into(),
        mission_id: Some(mission_id.clone()),
        thread_id: None,
        token_count: None,
        message: planner_message,
    };
    planner_event.apply(&mut state);
    let _ = swarm.handle_event_outcome(&mut state, &planner_event);
    let planner_message = state
        .agents
        .messages
        .iter()
        .find(|message| message.agent_id.as_deref() == Some("planner"))
        .expect("planner message");
    let rows = format_message_rows(&state, Some(&swarm), planner_message, 120);

    // Planner messages now also show the artifact link.
    assert!(rows[0].text.contains("done (see ARTIFACTS)"));
    assert!(matches!(rows[0].kind, ThreadRowKind::ArtifactLink));
}

#[test]
fn swarm_report_message_renders_artifact_link() {
    let mut state = AppState::new(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")),
        Buffer::empty("editor", None),
        Buffer::empty("notes", None),
    );
    state.agents.messages.clear();
    state.agents.missions.clear();
    state.agents.agents.clear();
    state.agents.agents.push(make_lane(
        "planner",
        "Planner",
        "Codex",
        AgentLaneKind::Codex,
        AgentStatus::Idle,
    ));

    let mut swarm = SwarmRuntime::default();
    let (mission_id, _dispatches) = swarm
        .start(
            &mut state,
            "planner".into(),
            vec!["planner".into()],
            SwarmSize::Count(2),
            Some("parallel".into()),
            None,
            "root".into(),
        )
        .expect("swarm start");
    let clone_id = format!("planner#swarm-{mission_id}-clone-01");
    let planner_message = format!(
        r#"
```json
{{
  "version": 2,
  "template": "parallel",
  "tasks": [
{{ "id": "t1", "agent_id": "{clone_id}", "title": "T1", "prompt": "DONE t1" }}
  ]
}}
```
"#
    );
    let planner_event = AgentBusEvent::TurnCompleted {
        agent_id: "planner".into(),
        mission_id: Some(mission_id.clone()),
        thread_id: None,
        token_count: None,
        message: planner_message,
    };
    planner_event.apply(&mut state);
    let _ = swarm.handle_event_outcome(&mut state, &planner_event);

    let clone_event = AgentBusEvent::TurnCompleted {
        agent_id: clone_id.clone(),
        mission_id: Some(mission_id.clone()),
        thread_id: Some("thr-clone".into()),
        token_count: None,
        message: "clone output".into(),
    };
    clone_event.apply(&mut state);
    let _ = swarm.handle_event_outcome(&mut state, &clone_event);

    let verify_event = AgentBusEvent::TurnCompleted {
        agent_id: clone_id,
        mission_id: Some(mission_id.clone()),
        thread_id: Some("thr-verify".into()),
        token_count: None,
        message: "verify output".into(),
    };
    verify_event.apply(&mut state);
    let _ = swarm.handle_event_outcome(&mut state, &verify_event);

    let report_event = AgentBusEvent::TurnCompleted {
        agent_id: "planner".into(),
        mission_id: Some(mission_id),
        thread_id: None,
        token_count: None,
        message: "# Final Report\n\nShip it.\n".into(),
    };
    report_event.apply(&mut state);
    let _ = swarm.handle_event_outcome(&mut state, &report_event);
    let rows = format_message_rows(
        &state,
        Some(&swarm),
        state.agents.messages.last().expect("report message"),
        120,
    );

    // Combined callout: "↳ [Planner] done (see ARTIFACTS)"
    assert!(rows[0].text.contains("done (see ARTIFACTS)"));
    assert_eq!(UnicodeWidthStr::width(rows[0].text.as_str()), 120);
    assert!(matches!(rows[0].kind, ThreadRowKind::ArtifactLink));
}

#[test]
fn agent_header_includes_truncated_role_badge() {
    let mut state = test_state();
    state.agents.selected_agent = Some("planner".into());
    state.agents.agents.push(AgentLane {
        heartbeat_age_secs: 1,
        last_message: "active".into(),
        ..make_lane(
            "reviewer",
            "UltraLongReviewerRoleName",
            "Lane C",
            AgentLaneKind::Mock,
            AgentStatus::Running,
        )
    });
    let msg = AgentMessage {
        at: "10:00:00".into(),
        channel: AgentChannel::Agent,
        agent_id: Some("reviewer".into()),
        mission_id: None,
        text: "ok".into(),
        prompt_msg_idx: None,
        kind: None,
    };
    let row = format_message_rows(&state, None, &msg, 120)
        .into_iter()
        .find(|row| !row.text.trim().is_empty())
        .expect("row");
    assert!(row.text.contains("[UltraLongRe…/reviewer]"));
}

#[test]
fn agent_ecg_renders_in_accent_color_and_text_is_cyan_theme() {
    let theme = Theme::default();
    let rows = [ThreadRow {
        text: "▁▁▁▁▁▁ hello".to_string(),
        kind: ThreadRowKind::Agent,
    }];
    let lines = thread_lines(rows.iter(), &theme);
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0].spans[0].style.fg, Some(theme.accent));
    assert_eq!(lines[0].spans[1].style.fg, Some(theme.title));
}

#[test]
fn agent_badge_renders_in_warning_color() {
    let theme = Theme::default();
    let rows = [ThreadRow {
        text: "[Coder] hello".to_string(),
        kind: ThreadRowKind::Agent,
    }];
    let lines = thread_lines(rows.iter(), &theme);
    let badge_span = lines[0]
        .spans
        .iter()
        .find(|span| span.content.as_ref() == "[Coder]")
        .expect("badge span");
    assert_eq!(badge_span.style.fg, Some(theme.warning));
    assert!(badge_span.style.add_modifier.contains(Modifier::BOLD));
}

#[test]
fn inline_command_style_is_light_gray_not_accent() {
    let theme = Theme::default();
    let rows = [ThreadRow {
        text: "  try `git status`".to_string(),
        kind: ThreadRowKind::Agent,
    }];
    let lines = thread_lines(rows.iter(), &theme);
    let code_span = lines[0]
        .spans
        .iter()
        .find(|span| span.content.as_ref() == "git status")
        .expect("expected inline code span");
    assert_eq!(code_span.style.fg, Some(theme.hl.operator));
    assert_ne!(code_span.style.fg, Some(theme.accent));
}

#[test]
fn inline_number_style_uses_accent() {
    let theme = Theme::default();
    let rows = [ThreadRow {
        text: "  ctx=`600`".to_string(),
        kind: ThreadRowKind::Agent,
    }];
    let lines = thread_lines(rows.iter(), &theme);
    let num_span = lines[0]
        .spans
        .iter()
        .find(|span| span.content.as_ref() == "600")
        .expect("expected numeric inline code span");
    assert_eq!(num_span.style.fg, Some(theme.accent));
}

#[test]
fn plain_text_paths_commands_and_numbers_are_highlighted() {
    let theme = Theme::default();
    let rows = [ThreadRow {
        text: "  see crates/nit-tui/src/widgets/agent_ops_view.rs:906; run cargo; wait 600s"
            .to_string(),
        kind: ThreadRowKind::Agent,
    }];
    let lines = thread_lines(rows.iter(), &theme);
    let line = &lines[0];

    let path_span = line
        .spans
        .iter()
        .find(|span| {
            span.content
                .as_ref()
                .contains("crates/nit-tui/src/widgets/agent_ops_view.rs:906")
        })
        .expect("expected path span");
    assert_eq!(path_span.style.fg, Some(theme.hl.link));
    assert!(path_span.style.add_modifier.contains(Modifier::UNDERLINED));

    let cargo_span = line
        .spans
        .iter()
        .find(|span| span.content.as_ref() == "cargo")
        .expect("expected command span");
    assert_eq!(cargo_span.style.fg, Some(theme.hl.operator));

    let num_span = line
        .spans
        .iter()
        .find(|span| span.content.as_ref() == "600s")
        .expect("expected number span");
    assert_eq!(num_span.style.fg, Some(theme.accent));
}

#[test]
fn verify_result_outcome_is_color_coded() {
    let theme = Theme::default();
    let rows = [
        ThreadRow {
            text: "  VERIFY result: FAIL".to_string(),
            kind: ThreadRowKind::Agent,
        },
        ThreadRow {
            text: "  VERIFY result: SUCCESS".to_string(),
            kind: ThreadRowKind::Agent,
        },
        ThreadRow {
            text: "  VERIFY result: ERROR".to_string(),
            kind: ThreadRowKind::Agent,
        },
    ];
    let lines = thread_lines(rows.iter(), &theme);
    assert_eq!(lines.len(), 3);

    let fail_span = lines[0]
        .spans
        .iter()
        .find(|span| span.content.as_ref() == "FAIL")
        .expect("expected FAIL span");
    assert_eq!(fail_span.style.fg, Some(theme.error));

    let success_span = lines[1]
        .spans
        .iter()
        .find(|span| span.content.as_ref() == "SUCCESS")
        .expect("expected SUCCESS span");
    assert_eq!(success_span.style.fg, Some(theme.success));

    let other_span = lines[2]
        .spans
        .iter()
        .find(|span| span.content.as_ref() == "ERROR")
        .expect("expected ERROR span");
    assert_eq!(other_span.style.fg, Some(theme.warning));
}

#[test]
fn thread_rows_keep_chronological_order() {
    let mut state = test_state();
    state.agents.selected_mission = None;
    state.agents.selected_agent = None;
    state.agents.messages.clear();
    state.agents.messages.push(AgentMessage {
        at: "10:00:00".into(),
        channel: AgentChannel::Agent,
        agent_id: Some("planner".into()),
        mission_id: None,
        text: "older message".into(),
        prompt_msg_idx: None,
        kind: None,
    });
    state.agents.messages.push(AgentMessage {
        at: "10:00:01".into(),
        channel: AgentChannel::Agent,
        agent_id: Some("coder".into()),
        mission_id: None,
        text: "newest message".into(),
        prompt_msg_idx: None,
        kind: None,
    });

    let rows = thread_rows(&state, None, 100, true);
    assert!(!rows.is_empty());
    let flattened = rows
        .iter()
        .map(|row| row.text.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    let newer_pos = flattened.find("[coder]").expect("newer badge present");
    let older_pos = flattened.find("[planner]").expect("older badge present");
    assert!(newer_pos > older_pos);
}

#[test]
fn breather_row_renders_below_user_prompt_when_agent_running() {
    let mut state = test_state();
    state.agents.selected_mission = None;
    state.agents.selected_agent = Some("planner".into());
    state.agents.messages.clear();
    state.agents.agents.clear();
    state.agents.agents.push(AgentLane {
        queue_len: 1,
        last_message: "active".into(),
        ..make_lane(
            "planner",
            "Planner",
            "Lane A",
            AgentLaneKind::Mock,
            AgentStatus::Running,
        )
    });
    let now = Instant::now();
    state.agents.active_turns.insert(
        "planner".into(),
        nit_core::state::AgentTurnState {
            started_at: now,
            last_heartbeat_at: now,
            last_output_at: now,
            stage: Some("starting".into()),
        },
    );
    state.agents.messages.push(AgentMessage {
        at: "10:00:02".into(),
        channel: AgentChannel::Agent,
        agent_id: None,
        mission_id: None,
        text: "please plan".into(),
        prompt_msg_idx: None,
        kind: None,
    });

    let rows = thread_rows(&state, None, 100, true);
    let breather_idx = rows
        .iter()
        .position(|row| matches!(row.kind, ThreadRowKind::Breather))
        .expect("breather row");
    let breather = rows.get(breather_idx).expect("breather row");
    assert!(matches!(breather.kind, ThreadRowKind::Breather));
    assert!(breather.text.contains("Working ..."));
    assert!(!breather.text.contains("Planner"));
}

#[test]
fn breather_row_hidden_when_latest_message_is_agent() {
    let mut state = test_state();
    state.agents.selected_mission = None;
    state.agents.selected_agent = Some("planner".into());
    state.agents.messages.clear();
    state.agents.agents.clear();
    state.agents.agents.push(AgentLane {
        queue_len: 1,
        last_message: "active".into(),
        ..make_lane(
            "planner",
            "Planner",
            "Lane A",
            AgentLaneKind::Mock,
            AgentStatus::Idle,
        )
    });
    state.agents.messages.push(AgentMessage {
        at: "10:00:01".into(),
        channel: AgentChannel::Agent,
        agent_id: None,
        mission_id: None,
        text: "please plan".into(),
        prompt_msg_idx: None,
        kind: None,
    });
    state.agents.messages.push(AgentMessage {
        at: "10:00:02".into(),
        channel: AgentChannel::Agent,
        agent_id: Some("planner".into()),
        mission_id: None,
        text: "on it".into(),
        prompt_msg_idx: None,
        kind: None,
    });

    let rows = thread_rows(&state, None, 100, true);
    assert!(!rows
        .iter()
        .any(|row| matches!(row.kind, ThreadRowKind::Breather)));
}

#[test]
fn breather_rows_include_multiple_running_agents() {
    let mut state = test_state();
    state.agents.selected_mission = None;
    state.agents.selected_agent = Some("planner".into());
    state.agents.messages.clear();
    state.agents.agents.clear();
    state.agents.agents.push(AgentLane {
        queue_len: 1,
        last_message: "active".into(),
        ..make_lane(
            "planner",
            "Planner",
            "Lane A",
            AgentLaneKind::Mock,
            AgentStatus::Running,
        )
    });
    state.agents.agents.push(AgentLane {
        queue_len: 1,
        last_message: "active".into(),
        ..make_lane(
            "coder",
            "Coder",
            "Lane B",
            AgentLaneKind::Mock,
            AgentStatus::Running,
        )
    });

    let now = Instant::now();
    state.agents.active_turns.insert(
        "planner".into(),
        nit_core::state::AgentTurnState {
            started_at: now,
            last_heartbeat_at: now,
            last_output_at: now,
            stage: Some("starting".into()),
        },
    );
    state.agents.active_turns.insert(
        "coder".into(),
        nit_core::state::AgentTurnState {
            started_at: now,
            last_heartbeat_at: now,
            last_output_at: now,
            stage: Some("tools/call(codex)".into()),
        },
    );

    state.agents.messages.push(AgentMessage {
        at: "10:00:02".into(),
        channel: AgentChannel::Agent,
        agent_id: None,
        mission_id: None,
        text: "do the work".into(),
        prompt_msg_idx: None,
        kind: None,
    });

    let rows = thread_rows(&state, None, 120, true);
    let flattened = rows.iter().map(|row| row.text.as_str()).collect::<Vec<_>>();
    assert!(flattened.iter().any(|line| line.contains("Working ...")));
    assert!(flattened.iter().any(|line| line.contains("Planner")));
    assert!(flattened.iter().any(|line| line.contains("Coder")));
    assert!(flattened
        .iter()
        .any(|line| line.contains("Starting session")));
}

#[test]
fn breather_rows_hide_stage_column_and_show_stage_subrow() {
    let mut state = test_state();
    state.agents.selected_mission = None;
    state.agents.selected_agent = Some("planner".into());
    state.agents.messages.clear();
    state.agents.agents.clear();
    state.agents.agents.push(AgentLane {
        queue_len: 1,
        last_message: "active".into(),
        ..make_lane(
            "planner",
            "Planner",
            "Lane A",
            AgentLaneKind::Codex,
            AgentStatus::Running,
        )
    });

    let now = Instant::now();
    state.agents.active_turns.insert(
        "planner".into(),
        nit_core::state::AgentTurnState {
            started_at: now,
            last_heartbeat_at: now,
            last_output_at: now,
            stage: Some("starting".into()),
        },
    );
    state
        .agents
        .codex_default_reasoning_effort
        .insert("planner".into(), "high".into());

    state.agents.messages.push(AgentMessage {
        at: "10:00:02".into(),
        channel: AgentChannel::Agent,
        agent_id: None,
        mission_id: None,
        text: "do the work".into(),
        prompt_msg_idx: None,
        kind: None,
    });

    let rows = thread_rows(&state, None, 120, true);
    let flattened = rows.iter().map(|row| row.text.as_str()).collect::<Vec<_>>();
    assert!(flattened
        .iter()
        .any(|line| line.contains("AGENT") && line.contains("ELAP")));
    assert!(!flattened.iter().any(|line| line.contains("STAGE")));
    assert!(!flattened.iter().any(|line| line.contains("SIZE")));
    assert!(flattened.iter().any(|line| line.contains("↳ Starting")));
}

#[test]
fn breather_rows_show_when_prompt_queued_but_not_yet_started() {
    let mut state = test_state();
    state.agents.selected_mission = None;
    state.agents.selected_agent = Some("planner".into());
    state.agents.messages.clear();
    state.agents.agents.clear();
    state.agents.agents.push(AgentLane {
        queue_len: 1,
        last_message: "queued".into(),
        ..make_lane(
            "planner",
            "Planner",
            "Lane A",
            AgentLaneKind::Mock,
            AgentStatus::Waiting,
        )
    });
    state.agents.queued_codex_turns.push_back(QueuedCodexTurn {
        agent_id: "planner".into(),
        mission_id: None,
        prompt: "do the thing".into(),
        prompt_msg_idx: None,
    });
    state.agents.messages.push(AgentMessage {
        at: "10:00:01".into(),
        channel: AgentChannel::Agent,
        agent_id: Some("planner".into()),
        mission_id: None,
        text: "finished previous turn".into(),
        prompt_msg_idx: None,
        kind: None,
    });

    let rows = thread_rows(&state, None, 120, true);
    let flattened = rows.iter().map(|row| row.text.as_str()).collect::<Vec<_>>();
    assert!(flattened.iter().any(|line| line.contains("Queued ...")));
    assert!(flattened.iter().any(|line| line.contains("Queued")));
}

#[test]
fn breather_rows_suppress_turn_metrics_when_queued_in_wide_layout() {
    let mut state = test_state();
    state.agents.selected_mission = None;
    state.agents.selected_agent = Some("planner".into());
    state.agents.messages.clear();
    state.agents.agents.clear();
    state.agents.agents.push(AgentLane {
        queue_len: 1,
        last_message: "queued".into(),
        ..make_lane(
            "planner",
            "Planner",
            "Lane A",
            AgentLaneKind::Mock,
            AgentStatus::Waiting,
        )
    });
    let now = Instant::now();
    state.agents.active_turns.insert(
        "planner".into(),
        nit_core::state::AgentTurnState {
            started_at: now,
            last_heartbeat_at: now,
            last_output_at: now,
            stage: Some("queued".into()),
        },
    );

    let rows = thread_rows(&state, None, 120, true);
    let flattened = rows.iter().map(|row| row.text.as_str()).collect::<Vec<_>>();
    assert!(flattened.iter().any(|line| line.contains("Queued")));
    let roster_line = flattened
        .iter()
        .find(|line| line.contains("Planner"))
        .expect("missing queued roster row");
    assert_eq!(roster_line.matches("--").count(), 3);
}

#[test]
fn breather_rows_suppress_turn_metrics_when_queued_in_narrow_layout() {
    let mut state = test_state();
    state.agents.selected_mission = None;
    state.agents.selected_agent = Some("a".into());
    state.agents.messages.clear();
    state.agents.agents.clear();
    state.agents.agents.push(AgentLane {
        queue_len: 1,
        last_message: "queued".into(),
        ..make_lane("a", "", "Lane A", AgentLaneKind::Mock, AgentStatus::Waiting)
    });
    state.agents.queued_codex_turns.push_back(QueuedCodexTurn {
        agent_id: "a".into(),
        mission_id: None,
        prompt: "do the thing".into(),
        prompt_msg_idx: None,
    });
    let now = Instant::now();
    state.agents.active_turns.insert(
        "a".into(),
        nit_core::state::AgentTurnState {
            started_at: now,
            last_heartbeat_at: now,
            last_output_at: now,
            stage: Some("queued".into()),
        },
    );
    state.agents.messages.push(AgentMessage {
        at: "10:00:01".into(),
        channel: AgentChannel::Agent,
        agent_id: Some("a".into()),
        mission_id: None,
        text: "finished previous turn".into(),
        prompt_msg_idx: None,
        kind: None,
    });

    let rows = thread_rows(&state, None, 23, true);
    let flattened = rows.iter().map(|row| row.text.as_str()).collect::<Vec<_>>();
    let roster_line = flattened
        .iter()
        .find(|line| line.trim_end().starts_with("a "))
        .expect("missing queued roster row");
    assert_eq!(roster_line.trim_end(), "a -- -- --");
}

#[test]
fn breather_rows_include_swarm_assigned_agents_even_when_idle() {
    let mut state = test_state();
    state.agents.messages.clear();
    state.agents.missions.clear();
    state.agents.agents.clear();
    state.agents.active_turns.clear();

    state.agents.missions.push(MissionRecord {
        id: "mis-001".into(),
        title: "Swarm: demo".into(),
        phase: MissionPhase::Plan,
        swarm: true,
        assigned_agents: vec!["planner".into(), "coder".into()],
        status: "PLAN".into(),
        updated_at: "t+0".into(),
    });
    state.agents.selected_mission = Some("mis-001".into());
    state.agents.mission_selected = 0;
    state.agents.selected_agent = Some("planner".into());

    state.agents.agents.push(AgentLane {
        queue_len: 1,
        current_mission: Some("mis-001".into()),
        last_message: "active".into(),
        ..make_lane(
            "planner",
            "Planner",
            "Lane A",
            AgentLaneKind::Mock,
            AgentStatus::Running,
        )
    });
    state.agents.agents.push(AgentLane {
        current_mission: Some("mis-001".into()),
        last_message: "idle".into(),
        ..make_lane(
            "coder",
            "Coder",
            "Lane B",
            AgentLaneKind::Mock,
            AgentStatus::Idle,
        )
    });

    let now = Instant::now();
    state.agents.active_turns.insert(
        "planner".into(),
        nit_core::state::AgentTurnState {
            started_at: now,
            last_heartbeat_at: now,
            last_output_at: now,
            stage: Some("starting".into()),
        },
    );

    state.agents.messages.push(AgentMessage {
        at: "10:00:02".into(),
        channel: AgentChannel::Agent,
        agent_id: None,
        mission_id: Some("mis-001".into()),
        text: "do the work".into(),
        prompt_msg_idx: None,
        kind: None,
    });
    state.agents.messages.push(AgentMessage {
        at: "10:00:03".into(),
        channel: AgentChannel::Broadcast,
        agent_id: Some("swarm".into()),
        mission_id: Some("mis-001".into()),
        text: "Swarm template: lab | integrator: planner | verifier: coder | gates: rust-ci".into(),
        prompt_msg_idx: None,
        kind: None,
    });

    let rows = thread_rows(&state, None, 120, true);
    let flattened = rows.iter().map(|row| row.text.as_str()).collect::<Vec<_>>();
    assert!(flattened.iter().any(|line| line.contains("Executing ...")));
    assert!(flattened.iter().any(|line| line.contains("Planner")));
    assert!(flattened.iter().any(|line| line.contains("Coder")));
    assert!(flattened.iter().any(|line| line.contains("Swarm pending")));
    // Compact footer: only the "Swarm · template · mission" header and the
    // gates summary should appear. Integrator/Verifier/Status/Notes labels
    // are deliberately dropped — the clone agents for those roles are in the
    // breather rows above, and the "Done" badge covers overall status.
    assert!(!rows.iter().any(|row| row.text.contains("Template:")));
    assert!(!rows.iter().any(|row| row.text.contains("Mission:")));
    assert!(!rows.iter().any(|row| row.text.contains("Integrator:")));
    assert!(!rows.iter().any(|row| row.text.contains("Verifier:")));
    assert!(!rows.iter().any(|row| row.text.contains("Status:")));
    assert!(!rows.iter().any(|row| row.text.contains("Notes:")));
    // Bullet markers (`• `) used by the old footer must not appear anywhere.
    assert!(!rows.iter().any(|row| row.text.contains("• ")));
    // Header and gates line are both present in the compact form. The
    // fixture's launch message has no explicit "mission:" field, so the
    // header collapses to "Swarm · <template>".
    assert!(rows.iter().any(|row| row.text.contains("Swarm · lab")));
    assert!(rows.iter().any(|row| row.text.contains("Gates: rust-ci")));
}

#[test]
fn breather_rows_show_done_when_swarm_idle_and_all_assigned_reported() {
    let mut state = test_state();
    state.agents.messages.clear();
    state.agents.missions.clear();
    state.agents.agents.clear();
    state.agents.active_turns.clear();
    state.agents.queued_codex_turns.clear();

    state.agents.missions.push(MissionRecord {
        id: "mis-001".into(),
        title: "Swarm: demo".into(),
        phase: MissionPhase::Plan,
        swarm: true,
        assigned_agents: vec!["planner".into(), "coder".into()],
        status: "PLAN".into(),
        updated_at: "t+0".into(),
    });
    state.agents.selected_mission = Some("mis-001".into());
    state.agents.mission_selected = 0;
    state.agents.selected_agent = Some("planner".into());

    state.agents.agents.push(AgentLane {
        current_mission: Some("mis-001".into()),
        last_message: "done".into(),
        ..make_lane(
            "planner",
            "Planner",
            "Lane A",
            AgentLaneKind::Mock,
            AgentStatus::Idle,
        )
    });
    state.agents.agents.push(AgentLane {
        current_mission: Some("mis-001".into()),
        last_message: "done".into(),
        ..make_lane(
            "coder",
            "Coder",
            "Lane B",
            AgentLaneKind::Mock,
            AgentStatus::Idle,
        )
    });

    state.agents.messages.push(AgentMessage {
        at: "10:00:01".into(),
        channel: AgentChannel::Agent,
        agent_id: Some("planner".into()),
        mission_id: Some("mis-001".into()),
        text: "planner output".into(),
        prompt_msg_idx: None,
        kind: None,
    });
    state.agents.messages.push(AgentMessage {
        at: "10:00:02".into(),
        channel: AgentChannel::Agent,
        agent_id: Some("coder".into()),
        mission_id: Some("mis-001".into()),
        text: "coder output".into(),
        prompt_msg_idx: None,
        kind: None,
    });
    state.agents.messages.push(AgentMessage {
        at: "10:00:03".into(),
        channel: AgentChannel::Broadcast,
        agent_id: Some("swarm".into()),
        mission_id: Some("mis-001".into()),
        text: "Swarm template: lab | integrator: planner | verifier: coder | gates: rust-ci".into(),
        prompt_msg_idx: None,
        kind: None,
    });

    let rows = thread_rows(&state, None, 120, true);
    assert!(rows.iter().any(|row| {
        matches!(row.kind, ThreadRowKind::Breather) && row.text.contains("▁▁▁▁▁▁ Done")
    }));
    assert!(!rows.iter().any(|row| row.text.contains("Working ...")));
}

#[test]
fn swarm_clones_without_message_render_as_skipped_after_mission_complete() {
    // Regression: clones the planner never dispatched would stay
    // "Swarm pending" forever once the run finished, because the per-agent
    // status check fell back to "did this agent emit a message?". Once
    // `mission_is_complete(mid)` is true, those clones should render as
    // "Swarm skipped" instead.
    let mut state = AppState::new(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")),
        Buffer::empty("editor", None),
        Buffer::empty("notes", None),
    );
    state.agents.messages.clear();
    state.agents.missions.clear();
    state.agents.agents.clear();
    state.agents.agents.push(make_lane(
        "planner",
        "Planner",
        "Codex",
        AgentLaneKind::Codex,
        AgentStatus::Idle,
    ));

    let mut swarm = SwarmRuntime::default();
    let (mission_id, _dispatches) = swarm
        .start(
            &mut state,
            "planner".into(),
            vec!["planner".into()],
            SwarmSize::Count(3),
            Some("parallel".into()),
            None,
            "root".into(),
        )
        .expect("swarm start");

    // Only clone-01 emits a message. clone-02 was never dispatched.
    let clone_one = format!("planner#swarm-{mission_id}-clone-01");
    state.agents.messages.push(AgentMessage {
        at: "10:00:01".into(),
        channel: AgentChannel::Agent,
        agent_id: Some(clone_one.clone()),
        mission_id: Some(mission_id.clone()),
        text: "clone-01 output".into(),
        prompt_msg_idx: None,
        kind: None,
    });

    // Move the run into `completed_runs` so `mission_is_complete` flips on.
    // Using abort here — the production "synth" path also lands in
    // `completed_runs`; the per-clone label logic doesn't care which.
    let _ = swarm.abort_mission(&mut state, &mission_id);

    state.agents.selected_mission = Some(mission_id.clone());
    state.agents.selected_agent = Some("planner".into());

    let rows = thread_rows(&state, Some(&swarm), 120, true);
    let flat = rows
        .iter()
        .map(|r| r.text.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        flat.contains("Swarm skipped"),
        "expected a 'Swarm skipped' row for the never-dispatched clone, got:\n{flat}"
    );
    assert!(
        !flat.contains("Swarm pending"),
        "no clone should still be 'Swarm pending' once the mission is in completed_runs:\n{flat}"
    );
}

#[test]
fn pane_breather_does_not_leak_active_turn_from_other_pane() {
    // Regression: in multipane, a running agent in pane A used to bleed
    // into pane B's chat-thread breather because
    // `breather_rows_for_user_prompt` always merged the
    // out-of-context "secondary" agents into the breather table.
    // `build_pane_thread_rows_with_breathers` now passes
    // `pane_isolate=true`, which drops secondary_ids — so pane B
    // (idle) should render no breather rows even though pane A is
    // mid-turn.
    let mut state = AppState::new(
        PathBuf::new(),
        Buffer::empty("editor", None),
        Buffer::empty("notes", None),
    );
    state.agents.messages.clear();
    state.agents.missions.clear();
    state.agents.agents.clear();

    let pane_a = "claude-opus-4-7#mp-pane-01";
    let pane_b = "claude-opus-4-7#mp-pane-02";
    state.agents.agents.push(make_lane(
        pane_a,
        "Claude (pane A)",
        "Claude",
        AgentLaneKind::Claude,
        AgentStatus::Running,
    ));
    state.agents.agents.push(make_lane(
        pane_b,
        "Claude (pane B)",
        "Claude",
        AgentLaneKind::Claude,
        AgentStatus::Idle,
    ));

    let now = Instant::now();
    state.agents.active_turns.insert(
        pane_a.into(),
        nit_core::state::AgentTurnState {
            started_at: now,
            last_heartbeat_at: now,
            last_output_at: now,
            stage: Some("starting".into()),
        },
    );

    // Render pane B's chat thread (mimic the multipane render flow:
    // selected_agent / selected_mission aliased to pane B's values).
    state.agents.selected_agent = Some(pane_b.into());
    state.agents.selected_mission = None;

    let rows = build_pane_thread_rows_with_breathers(&state, None, Some(pane_b), None, 120, false);
    let flat = rows
        .iter()
        .map(|r| r.text.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        !flat.contains(pane_a),
        "pane B must not show pane A's agent id in its breather table:\n{flat}"
    );
    assert!(
        !flat.contains("Working ..."),
        "pane B is idle — no 'Working ...' breather should render even when pane A is busy:\n{flat}"
    );
}

#[test]
fn chat_input_height_grows_with_text_but_stays_capped() {
    let mut state = test_state();
    state.agents.chat_input = (0..48).map(|i| format!("line-{i}\n")).collect::<String>();
    state.agents.chat_input_cursor = state.agents.chat_input.chars().count();
    let area = Rect {
        x: 0,
        y: 0,
        width: 90,
        height: 28,
    };
    let metrics = chat_input_scroll_metrics(area, &state).expect("chat metrics");
    assert!(metrics.visible_height >= 4);
    assert!(metrics.visible_height <= 12);
    assert!(metrics.visible_height < area.height as usize);
    assert!(metrics.max_scroll > 0);
}

#[test]
fn map_chat_input_click_to_cursor_index() {
    let mut state = test_state();
    state.agents.chat_input = "hello\nworld".into();
    state.agents.chat_input_cursor = 0;
    let area = Rect {
        x: 0,
        y: 0,
        width: 60,
        height: 16,
    };
    let input_area = chat_input_text_area(area, &state).expect("input area");

    let top = map_chat_input_point_to_cursor(
        area,
        &state,
        input_area.x.saturating_add(4),
        input_area.y,
        false,
    )
    .expect("cursor from top row");
    assert_eq!(top, 4);

    let second = map_chat_input_point_to_cursor(
        area,
        &state,
        input_area.x.saturating_add(2),
        input_area.y.saturating_add(1),
        false,
    )
    .expect("cursor from second row");
    assert_eq!(second, 8);
}

#[test]
fn user_bubble_rows_use_dim_prompt_bg_and_highlight_you_label() {
    let theme = Theme::default();
    let prompt_bg = user_prompt_bg(&theme);
    let rows = [
        ThreadRow {
            text: "  You      ".to_string(),
            kind: ThreadRowKind::User,
        },
        ThreadRow {
            text: "  hello    ".to_string(),
            kind: ThreadRowKind::User,
        },
    ];
    let lines = thread_lines(rows.iter(), &theme);

    assert!(lines[0]
        .spans
        .iter()
        .all(|span| span.style.bg == Some(prompt_bg)));
    assert!(lines[1]
        .spans
        .iter()
        .all(|span| span.style.bg == Some(prompt_bg)));
    assert!(
        lines[0]
            .spans
            .iter()
            .any(|span| span.content.as_ref().contains("You")
                && span.style.fg == Some(Color::Gray)
                && span.style.add_modifier.contains(Modifier::BOLD)),
        "expected 'You' label span to use gray + bold style"
    );
    assert!(
        lines[1]
            .spans
            .iter()
            .any(|span| span.content.as_ref().contains("hello")
                && span.style.fg == Some(Color::Gray)),
        "expected user prompt text to use gray foreground"
    );
}

#[test]
fn artifact_link_rows_use_user_prompt_bg() {
    let theme = Theme::default();
    let prompt_bg = user_prompt_bg(&theme);
    let rows = [ThreadRow {
        text: "done (see ARTIFACTS)    ".to_string(),
        kind: ThreadRowKind::ArtifactLink,
    }];
    let lines = thread_lines(rows.iter(), &theme);

    assert!(
        lines[0]
            .spans
            .iter()
            .filter(|span| span.content.as_ref() != "ARTIFACTS")
            .all(|span| span.style.bg == Some(prompt_bg)),
        "expected artifact link stripe to reuse the user prompt background"
    );
    assert!(
        lines[0]
            .spans
            .iter()
            .any(|span| span.content.as_ref() == "ARTIFACTS"
                && span.style.bg == Some(theme.title_focused)),
        "expected ARTIFACTS badge to keep its focused accent background"
    );
}

fn state_with_active_clones(clone_ids: &[&str]) -> AppState {
    state_with_active_clones_inner(clone_ids, None)
}

/// Same as `state_with_active_clones` but lets callers seed each clone's
/// `AgentTurnState.stage` so tests can exercise the turn-stage path of
/// `swarm_exec_label` (the agent's self-reported phase wins over the
/// swarm-task role).
#[allow(dead_code)]
fn state_with_active_clones_at_stage(clone_ids: &[&str], stage: &str) -> AppState {
    state_with_active_clones_inner(clone_ids, Some(stage.to_string()))
}

fn state_with_active_clones_inner(clone_ids: &[&str], stage: Option<String>) -> AppState {
    let mut state = test_state();
    state.agents.agents.clear();
    state.agents.active_turns.clear();
    let now = Instant::now();
    for id in clone_ids {
        state.agents.agents.push(make_lane(
            id,
            "(clone 02)",
            "swarm clone",
            AgentLaneKind::Mock,
            AgentStatus::Running,
        ));
        state.agents.active_turns.insert(
            (*id).into(),
            nit_core::state::AgentTurnState {
                started_at: now,
                last_heartbeat_at: now,
                last_output_at: now,
                stage: stage.clone(),
            },
        );
    }
    state
}

#[test]
fn swarm_exec_label_uses_task_role_for_swarm_clones() {
    // Two swarm clones running propose tasks. Their lane `role` is the
    // generic "(clone NN)" placeholder, but the swarm DAG knows they're
    // proposers — the breather should reflect that.
    let clone_a = "claude-opus-4-7#swarm-mis-001-clone-02".to_string();
    let clone_b = "claude-opus-4-7#swarm-mis-001-clone-04".to_string();
    let state = state_with_active_clones(&[clone_a.as_str(), clone_b.as_str()]);
    let runtime = test_runtime_with_running_tasks(
        "mis-001",
        &[(clone_a.as_str(), "propose"), (clone_b.as_str(), "propose")],
    );

    let label = swarm_exec_label(&state, &[clone_a.clone(), clone_b.clone()], Some(&runtime));
    assert_eq!(label, "Proposing ...");
}

#[test]
fn swarm_exec_label_falls_back_to_executing_when_runtime_missing() {
    // Without a swarm runtime the lookup falls through to `agent.role` —
    // for swarm clones that's "(clone NN)" which doesn't match any keyword,
    // so the generic "Executing ..." label is correct.
    let clone_a = "claude-opus-4-7#swarm-mis-001-clone-02".to_string();
    let state = state_with_active_clones(&[clone_a.as_str()]);

    let label = swarm_exec_label(&state, &[clone_a.clone()], None);
    assert_eq!(label, "Executing ...");
}

#[test]
fn swarm_exec_label_returns_executing_for_mixed_running_roles() {
    // Different running roles (e.g. propose + integrate) should not collapse
    // to either-or — the breather falls back to the generic label.
    let clone_a = "claude-opus-4-7#swarm-mis-001-clone-02".to_string();
    let clone_b = "claude-opus-4-7#swarm-mis-001-clone-03".to_string();
    let state = state_with_active_clones(&[clone_a.as_str(), clone_b.as_str()]);
    let runtime = test_runtime_with_running_tasks(
        "mis-001",
        &[
            (clone_a.as_str(), "propose"),
            (clone_b.as_str(), "integrate"),
        ],
    );

    let label = swarm_exec_label(&state, &[clone_a.clone(), clone_b.clone()], Some(&runtime));
    assert_eq!(label, "Executing ...");
}

#[test]
fn swarm_exec_label_uses_researching_for_research_mission_with_mixed_roles() {
    // Research missions fan out across recon / design / propose / review
    // simultaneously; without the mission-kind shortcut the role-uniformity
    // test below would drop to "Executing ...". The mission_kind check makes
    // the breather say "Researching ..." for the whole run.
    use crate::swarm::{test_runtime_with_running_tasks_and_kind, SwarmMissionKind};
    let clone_recon = "claude-opus-4-7#swarm-mis-001-clone-02".to_string();
    let clone_design = "claude-opus-4-7#swarm-mis-001-clone-03".to_string();
    let state = state_with_active_clones(&[clone_recon.as_str(), clone_design.as_str()]);
    let runtime = test_runtime_with_running_tasks_and_kind(
        "mis-001",
        &[
            (clone_recon.as_str(), "recon"),
            (clone_design.as_str(), "design"),
        ],
        SwarmMissionKind::Research,
    );

    let label = swarm_exec_label(
        &state,
        &[clone_recon.clone(), clone_design.clone()],
        Some(&runtime),
    );
    assert_eq!(label, "Researching ...");
}

#[test]
fn swarm_exec_label_compound_role_uses_last_token_gerund() {
    // `computational-research` (and the legacy `computational research`
    // form) compound role names should label using only the trailing
    // activity verb — the qualifier is dropped so the breather reads
    // naturally ("Researching ..." instead of
    // "Computational-researching ..."). Operators who want the raw
    // role see it in the agent-ops DAG.
    //
    // Roles whose trailing token is an agent-noun ("genome-reviewer",
    // "*-or") would gerund to awkward strings ("Reviewering"), but
    // those roles only fire during the VERIFY phase which has its own
    // hard-coded "Verifying ..." label in `breather.rs` — they never
    // reach `swarm_exec_label`. No need to special-case them here.
    use crate::swarm::{test_runtime_with_running_tasks_and_kind, SwarmMissionKind};
    let clone_a = "claude-opus-4-7#swarm-mis-001-clone-02".to_string();
    let state = state_with_active_clones(&[clone_a.as_str()]);

    for (role, expected) in [
        ("computational-research", "Researching ..."),
        ("computational research", "Researching ..."),
        ("computational_research", "Researching ..."),
    ] {
        let runtime = test_runtime_with_running_tasks_and_kind(
            "mis-001",
            &[(clone_a.as_str(), role)],
            SwarmMissionKind::ComputationalResearch,
        );
        let label = swarm_exec_label(&state, &[clone_a.clone()], Some(&runtime));
        assert_eq!(
            label, expected,
            "role `{role}` did not produce the expected breather label"
        );
    }
}

#[test]
fn swarm_exec_label_screenshot_repro_research_with_integrator_only() {
    // End-to-end reproduction of the production screenshot: a `parallel`
    // research swarm with multiple clones registered, only clone-01
    // currently Running the `integrate` task. Other clones are idle (no
    // active turn). The breather must show "Integrating ..." — the
    // previous bug surfaced "Researching ..." because the mission-kind
    // shortcut fired before the role pass, and even after that fix the
    // turn-stage `?` was failing fast on idle clones.
    use crate::swarm::{test_runtime_with_running_tasks_and_kind, SwarmMissionKind};
    let clone_integrator = "claude-opus-4-7#swarm-mis-001-clone-01".to_string();
    let clone_idle_a = "claude-opus-4-7#swarm-mis-001-clone-02".to_string();
    let clone_idle_b = "claude-opus-4-7#swarm-mis-001-clone-03".to_string();
    let clone_idle_c = "claude-opus-4-7#swarm-mis-001-clone-04".to_string();
    // Only the integrator has an active turn.
    let state = state_with_active_clones(&[clone_integrator.as_str()]);
    let runtime = test_runtime_with_running_tasks_and_kind(
        "mis-001",
        &[(clone_integrator.as_str(), "integrate")],
        SwarmMissionKind::Research,
    );
    let ordered_ids = vec![
        clone_integrator.clone(),
        clone_idle_a,
        clone_idle_b,
        clone_idle_c,
    ];

    let label = swarm_exec_label(&state, &ordered_ids, Some(&runtime));
    assert_eq!(label, "Integrating ...");
}

#[test]
fn swarm_exec_label_uses_role_label_when_uniform_in_research_mission() {
    // When a research swarm has the integrator clone running on its own
    // (e.g. synthesizing the recon/design/propose outputs), the breather
    // must reflect the active role ("Integrating ...") rather than the
    // generic "Researching ..." — the mission-kind label only kicks in
    // as the fallback for mixed/empty roles.
    use crate::swarm::{test_runtime_with_running_tasks_and_kind, SwarmMissionKind};
    let clone_integrator = "claude-opus-4-7#swarm-mis-001-clone-01".to_string();
    let state = state_with_active_clones(&[clone_integrator.as_str()]);
    let runtime = test_runtime_with_running_tasks_and_kind(
        "mis-001",
        &[(clone_integrator.as_str(), "integrate")],
        SwarmMissionKind::Research,
    );

    let label = swarm_exec_label(&state, &[clone_integrator.clone()], Some(&runtime));
    assert_eq!(label, "Integrating ...");
}

#[test]
fn swarm_exec_label_uses_role_label_for_computational_research_mission() {
    // Same precedence rule for computational-research: an active
    // `implement` role wins over the mission-kind fallback. The label is
    // derived algorithmically from the role string, so the planner-
    // emitted role ("implement") surfaces as "Implementing ..." rather
    // than being aliased to a hard-coded "Coding ..." synonym.
    use crate::swarm::{test_runtime_with_running_tasks_and_kind, SwarmMissionKind};
    let clone_a = "claude-opus-4-7#swarm-mis-001-clone-02".to_string();
    let state = state_with_active_clones(&[clone_a.as_str()]);
    let runtime = test_runtime_with_running_tasks_and_kind(
        "mis-001",
        &[(clone_a.as_str(), "implement")],
        SwarmMissionKind::ComputationalResearch,
    );

    let label = swarm_exec_label(&state, &[clone_a.clone()], Some(&runtime));
    assert_eq!(label, "Implementing ...");
}

#[test]
fn swarm_exec_label_prefers_agent_reported_turn_stage_over_role() {
    // When every active clone surfaces the same `AgentTurnState.stage`
    // value (the freshest signal — emitted by the runner via
    // TurnStage bus events), the breather uses it verbatim instead of
    // falling back to the swarm-task role. Lets the model say what
    // it's doing without the orchestrator second-guessing it.
    use crate::swarm::test_runtime_with_running_tasks;
    let clone_a = "claude-opus-4-7#swarm-mis-001-clone-02".to_string();
    let clone_b = "claude-opus-4-7#swarm-mis-001-clone-03".to_string();
    let state =
        state_with_active_clones_at_stage(&[clone_a.as_str(), clone_b.as_str()], "Synthesizing");
    let runtime = test_runtime_with_running_tasks(
        "mis-001",
        &[(clone_a.as_str(), "propose"), (clone_b.as_str(), "propose")],
    );

    let label = swarm_exec_label(&state, &[clone_a.clone(), clone_b.clone()], Some(&runtime));
    assert_eq!(label, "Synthesizing ...");
}

#[test]
fn swarm_exec_label_ignores_protocol_shaped_turn_stages() {
    // Production stage strings from the Claude/Codex runners look like
    // `assistant(text)`, `tool_use(read)`, `tools/call(grep)`,
    // `content_block_start`, `starting` — none of which read well as a
    // breather phase. The semantic-stage filter rejects them so the
    // role-based pass takes over.
    use crate::swarm::test_runtime_with_running_tasks;
    let clone_a = "claude-opus-4-7#swarm-mis-001-clone-02".to_string();
    let clone_b = "claude-opus-4-7#swarm-mis-001-clone-03".to_string();

    for noise in [
        "assistant(text)",
        "tool_use(read)",
        "tools/call(grep)",
        "content_block_start",
        "starting",
        "running",
        "system",
        "result",
    ] {
        let state = state_with_active_clones_at_stage(&[clone_a.as_str(), clone_b.as_str()], noise);
        let runtime = test_runtime_with_running_tasks(
            "mis-001",
            &[(clone_a.as_str(), "propose"), (clone_b.as_str(), "propose")],
        );
        let label = swarm_exec_label(&state, &[clone_a.clone(), clone_b.clone()], Some(&runtime));
        assert_eq!(
            label, "Proposing ...",
            "stage `{noise}` should not surface as the breather label"
        );
    }
}

#[test]
fn swarm_exec_label_falls_through_when_turn_stages_disagree() {
    // Mixed stages across clones → drop the turn-stage path and fall
    // back to the role pass (or mission-kind fallback if roles are
    // also mixed). Reproduces the "different clones at different
    // phases" case where surfacing one of them would be misleading.
    use crate::swarm::test_runtime_with_running_tasks;
    let clone_a = "claude-opus-4-7#swarm-mis-001-clone-02".to_string();
    let clone_b = "claude-opus-4-7#swarm-mis-001-clone-03".to_string();
    let mut state =
        state_with_active_clones_at_stage(&[clone_a.as_str(), clone_b.as_str()], "Reviewing");
    if let Some(turn) = state.agents.active_turns.get_mut(clone_b.as_str()) {
        turn.stage = Some("Tooling".into());
    }
    let runtime = test_runtime_with_running_tasks(
        "mis-001",
        &[(clone_a.as_str(), "propose"), (clone_b.as_str(), "propose")],
    );

    let label = swarm_exec_label(&state, &[clone_a.clone(), clone_b.clone()], Some(&runtime));
    assert_eq!(label, "Proposing ...");
}

#[test]
fn swarm_exec_label_resolves_role_via_clones_own_mission_id() {
    // Even if the caller's selected mission isn't the same as the clone's
    // mission, role resolution should still work because we extract the
    // mission_id from the clone's agent ID directly. This mirrors
    // `agent_ops_view::swarm_clone_label_parts`.
    let clone_a = "claude-opus-4-7#swarm-mis-007-clone-02".to_string();
    let state = state_with_active_clones(&[clone_a.as_str()]);
    let runtime = test_runtime_with_running_tasks("mis-007", &[(clone_a.as_str(), "propose")]);

    let label = swarm_exec_label(&state, &[clone_a.clone()], Some(&runtime));
    assert_eq!(label, "Proposing ...");
}

#[test]
fn swarm_exec_label_ignores_queued_tasks_for_active_agents() {
    // Regression: a parallel swarm with two Running proposers and a third
    // active clone whose ONLY matching task is "Queued" (Ready/Dispatched).
    // The Queued task's role must not be added to the role list, otherwise
    // the breather flips from "Proposing ..." to "Executing ..." (mixed
    // roles). Reproduces the production scenario where clone-05's queued
    // "test" task poisoned the uniformity check.
    use crate::swarm::test_runtime_with_running_and_queued_tasks;
    let clone_propose_a = "claude-opus-4-7#swarm-mis-001-clone-02".to_string();
    let clone_propose_b = "claude-opus-4-7#swarm-mis-001-clone-04".to_string();
    let clone_test = "claude-opus-4-7#swarm-mis-001-clone-05".to_string();
    let state = state_with_active_clones(&[
        clone_propose_a.as_str(),
        clone_propose_b.as_str(),
        clone_test.as_str(),
    ]);
    let runtime = test_runtime_with_running_and_queued_tasks(
        "mis-001",
        &[
            (clone_propose_a.as_str(), "propose"),
            (clone_propose_b.as_str(), "propose"),
        ],
        &[(clone_test.as_str(), "test")],
    );

    let label = swarm_exec_label(
        &state,
        &[clone_propose_a, clone_propose_b, clone_test],
        Some(&runtime),
    );
    assert_eq!(label, "Proposing ...");
}

#[test]
fn ordinary_swarm_broadcast_is_filtered_from_chat() {
    // Regression: the chat console hides agent_id="swarm" + Broadcast as
    // redundant with per-agent callouts. Routine system messages
    // ("Starting VERIFY", "Dispatching genome review", etc.) must still
    // be filtered so we don't double-up notifications.
    let state = test_state();
    let msg = AgentMessage {
        at: "10:00:00".into(),
        channel: AgentChannel::Broadcast,
        agent_id: Some("swarm".into()),
        mission_id: Some("mis-001".into()),
        text: "Dispatching genome review".into(),
        prompt_msg_idx: None,
        kind: None,
    };
    let rows = format_message_rows(&state, None, &msg, 80);
    assert!(rows.is_empty(), "ordinary swarm broadcast should be hidden");
}

#[test]
fn system_alert_swarm_broadcast_is_rendered_in_chat() {
    // Regression: clamp / FD-warning messages route through
    // push_system_alert_to_mission, which tags the message with
    // SYSTEM_ALERT_KIND. Those MUST render so the operator actually sees
    // "Requested 1000 agents, started 56 ..." instead of the broadcast
    // being silently dropped by the redundancy filter.
    let state = test_state();
    let msg = AgentMessage {
        at: "10:00:00".into(),
        channel: AgentChannel::Broadcast,
        agent_id: Some("swarm".into()),
        mission_id: Some("mis-001".into()),
        text: "Requested 1000 agents, started 56 (effective ceiling 56)".into(),
        prompt_msg_idx: None,
        kind: Some(crate::swarm::SYSTEM_ALERT_KIND.into()),
    };
    let rows = format_message_rows(&state, None, &msg, 80);
    assert!(
        !rows.is_empty(),
        "system-alert swarm broadcast must render — operator-facing warnings cannot be silently dropped"
    );
    assert!(
        rows.iter()
            .any(|r| r.text.contains("Requested 1000 agents")),
        "alert text must appear in rendered rows: {:?}",
        rows.iter().map(|r| r.text.as_str()).collect::<Vec<_>>()
    );
}

#[test]
fn system_alert_long_message_wraps_within_pane_width() {
    // Regression: alert text was overflowing the pane because the wrap
    // width didn't account for the "↳ [swarm] " prefix, so visible chars
    // got clipped at the right edge. The operator's screenshot showed
    // "...; `uli" / "is 256). Bump..." cut mid-word — exactly this bug.
    // Every rendered row's visible width must fit in the pane.
    let state = test_state();
    let long = "Requested 1000 agents, started 56 (effective ceiling 56; \
                `ulimit -n` is 256). Bump `ulimit -n 4096` and restart \
                nit for more headroom.";
    let msg = AgentMessage {
        at: "10:00:00".into(),
        channel: AgentChannel::Broadcast,
        agent_id: Some("swarm".into()),
        mission_id: Some("mis-001".into()),
        text: long.into(),
        prompt_msg_idx: None,
        kind: Some(crate::swarm::SYSTEM_ALERT_KIND.into()),
    };
    let pane_width = 60;
    let rows = format_message_rows(&state, None, &msg, pane_width);
    for row in &rows {
        let visible = UnicodeWidthStr::width(row.text.as_str());
        assert!(
            visible <= pane_width,
            "row exceeds pane width ({visible} > {pane_width}): {:?}",
            row.text
        );
    }
    // Sanity: the message must have actually rendered (not been swallowed
    // by an earlier-returning branch with empty `out`).
    assert!(rows.iter().any(|r| r.text.contains("Requested 1000")));
}

#[test]
fn multipane_system_message_not_an_artifact_callout() {
    // Reproduces the screenshot bug: a `multipane-system` roster
    // acknowledgement carries an `agent_id`, which used to drag it
    // through `artifacts_popup_ref_for_message` and emit a `(see
    // ARTIFACTS)` callout before any mission had run. The kind-based
    // filter inside `format_message_rows` keeps the row plain.
    let state = test_state();
    let msg = AgentMessage {
        at: "t+1".into(),
        channel: AgentChannel::Agent,
        agent_id: Some("claude-haiku-4-5#mp-pane-00".into()),
        mission_id: None,
        text: "selected agent → claude-haiku-4-5#mp-pane-00".into(),
        prompt_msg_idx: None,
        kind: Some("multipane-system".into()),
    };
    let rows = format_message_rows(&state, None, &msg, 80);
    assert!(
        rows.iter()
            .all(|row| !matches!(row.kind, ThreadRowKind::ArtifactLink)),
        "system roster ack must not produce an artifact link"
    );
    assert!(
        rows.iter().all(|row| !row.text.contains("(see ARTIFACTS)")),
        "system roster ack must not get the (see ARTIFACTS) suffix"
    );
    assert!(
        rows.iter().all(|row| !row.text.contains(" done")),
        "system roster ack must not produce a `done` callout"
    );
    assert!(
        rows.iter().all(|row| !row.text.contains('[')),
        "system roster ack must not include an agent badge in brackets"
    );
    assert!(
        rows.iter().any(|row| row.text.contains("selected agent")),
        "the rendered text should still surface the message body"
    );
    assert!(
        rows.iter()
            .all(|row| matches!(row.kind, ThreadRowKind::StatusSubRow)),
        "all rendered rows must be StatusSubRow"
    );
}
