use super::{
    artifact_message_index_for_line, chat_input_scroll_metrics, chat_input_text_area,
    ecg_indicator, format_message_rows, map_chat_input_point_to_cursor, thread_lines, thread_rows,
    user_prompt_bg, wrap_input_with_cursor, wrap_visual_line, ThreadRow, ThreadRowKind,
};
use crate::swarm::{SwarmRuntime, SwarmSize};
use crate::theme::Theme;
use nit_core::{
    AgentBusEvent, AgentChannel, AgentLane, AgentMessage, AgentStatus, AppState, Buffer,
    MissionPhase, MissionRecord, QueuedCodexTurn,
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
        id: "coder".into(),
        role: "Coder".into(),
        lane: "Lane B".into(),
        kind: nit_core::AgentLaneKind::Mock,
        status: AgentStatus::Running,
        heartbeat_age_secs: 1,
        queue_len: 1,
        current_mission: None,
        last_message: "active".into(),
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
        id: "planner#swarm-mis-001-clone-01".into(),
        role: "Planner (clone 01)".into(),
        lane: "Lane A".into(),
        kind: nit_core::AgentLaneKind::Mock,
        status: AgentStatus::Running,
        heartbeat_age_secs: 0,
        queue_len: 1,
        current_mission: Some("mis-001".into()),
        last_message: String::new(),
    });

    assert_eq!(
        super::agent_identity_badge(&state, "planner#swarm-mis-001-clone-01"),
        "clone 01"
    );
}

#[test]
fn clone_roster_label_shows_base_model_and_clone_suffix() {
    let agent = AgentLane {
        id: "planner#swarm-mis-001-clone-01".into(),
        role: "Planner (clone 01)".into(),
        lane: "Lane A".into(),
        kind: nit_core::AgentLaneKind::Mock,
        status: AgentStatus::Idle,
        heartbeat_age_secs: 0,
        queue_len: 0,
        current_mission: Some("mis-001".into()),
        last_message: String::new(),
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
        id: clone_id.into(),
        role: "GPT-5.4 (clone 01)".into(),
        lane: "Codex".into(),
        kind: nit_core::AgentLaneKind::Codex,
        status: AgentStatus::Running,
        heartbeat_age_secs: 0,
        queue_len: 1,
        current_mission: Some("mis-001".into()),
        last_message: "active".into(),
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
        id: "coder".into(),
        role: "Coder".into(),
        lane: "Lane B".into(),
        kind: nit_core::AgentLaneKind::Mock,
        status: AgentStatus::Idle,
        heartbeat_age_secs: 1,
        queue_len: 0,
        current_mission: None,
        last_message: "idle".into(),
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
    state.agents.agents.push(AgentLane {
        id: "coder".into(),
        role: "Coder".into(),
        lane: "Lane B".into(),
        kind: nit_core::AgentLaneKind::Mock,
        status: AgentStatus::Idle,
        heartbeat_age_secs: 0,
        queue_len: 0,
        current_mission: None,
        last_message: String::new(),
    });
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
    state.agents.agents.push(AgentLane {
        id: "planner".into(),
        role: "Planner".into(),
        lane: "Codex".into(),
        kind: nit_core::AgentLaneKind::Codex,
        status: AgentStatus::Idle,
        heartbeat_age_secs: 0,
        queue_len: 0,
        current_mission: None,
        last_message: String::new(),
    });

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
    state.agents.agents.push(AgentLane {
        id: "planner".into(),
        role: "Planner".into(),
        lane: "Codex".into(),
        kind: nit_core::AgentLaneKind::Codex,
        status: AgentStatus::Idle,
        heartbeat_age_secs: 0,
        queue_len: 0,
        current_mission: None,
        last_message: String::new(),
    });

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
        id: "reviewer".into(),
        role: "UltraLongReviewerRoleName".into(),
        lane: "Lane C".into(),
        kind: nit_core::AgentLaneKind::Mock,
        status: AgentStatus::Running,
        heartbeat_age_secs: 1,
        queue_len: 0,
        current_mission: None,
        last_message: "active".into(),
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
        id: "planner".into(),
        role: "Planner".into(),
        lane: "Lane A".into(),
        kind: nit_core::AgentLaneKind::Mock,
        status: AgentStatus::Running,
        heartbeat_age_secs: 0,
        queue_len: 1,
        current_mission: None,
        last_message: "active".into(),
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
        id: "planner".into(),
        role: "Planner".into(),
        lane: "Lane A".into(),
        kind: nit_core::AgentLaneKind::Mock,
        status: AgentStatus::Idle,
        heartbeat_age_secs: 0,
        queue_len: 1,
        current_mission: None,
        last_message: "active".into(),
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
        id: "planner".into(),
        role: "Planner".into(),
        lane: "Lane A".into(),
        kind: nit_core::AgentLaneKind::Mock,
        status: AgentStatus::Running,
        heartbeat_age_secs: 0,
        queue_len: 1,
        current_mission: None,
        last_message: "active".into(),
    });
    state.agents.agents.push(AgentLane {
        id: "coder".into(),
        role: "Coder".into(),
        lane: "Lane B".into(),
        kind: nit_core::AgentLaneKind::Mock,
        status: AgentStatus::Running,
        heartbeat_age_secs: 0,
        queue_len: 1,
        current_mission: None,
        last_message: "active".into(),
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
        id: "planner".into(),
        role: "Planner".into(),
        lane: "Lane A".into(),
        kind: nit_core::AgentLaneKind::Codex,
        status: AgentStatus::Running,
        heartbeat_age_secs: 0,
        queue_len: 1,
        current_mission: None,
        last_message: "active".into(),
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
        id: "planner".into(),
        role: "Planner".into(),
        lane: "Lane A".into(),
        kind: nit_core::AgentLaneKind::Mock,
        status: AgentStatus::Waiting,
        heartbeat_age_secs: 0,
        queue_len: 1,
        current_mission: None,
        last_message: "queued".into(),
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
        id: "planner".into(),
        role: "Planner".into(),
        lane: "Lane A".into(),
        kind: nit_core::AgentLaneKind::Mock,
        status: AgentStatus::Waiting,
        heartbeat_age_secs: 0,
        queue_len: 1,
        current_mission: None,
        last_message: "queued".into(),
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
        id: "a".into(),
        role: "".into(),
        lane: "Lane A".into(),
        kind: nit_core::AgentLaneKind::Mock,
        status: AgentStatus::Waiting,
        heartbeat_age_secs: 0,
        queue_len: 1,
        current_mission: None,
        last_message: "queued".into(),
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
        id: "planner".into(),
        role: "Planner".into(),
        lane: "Lane A".into(),
        kind: nit_core::AgentLaneKind::Mock,
        status: AgentStatus::Running,
        heartbeat_age_secs: 0,
        queue_len: 1,
        current_mission: Some("mis-001".into()),
        last_message: "active".into(),
    });
    state.agents.agents.push(AgentLane {
        id: "coder".into(),
        role: "Coder".into(),
        lane: "Lane B".into(),
        kind: nit_core::AgentLaneKind::Mock,
        status: AgentStatus::Idle,
        heartbeat_age_secs: 0,
        queue_len: 0,
        current_mission: Some("mis-001".into()),
        last_message: "idle".into(),
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
    // While agents are working, the footer shows only mission, gates, and notes.
    assert!(!rows.iter().any(|row| row.text.contains("Template:")));
    assert!(!rows.iter().any(|row| row.text.contains("Integrator:")));
    assert!(!rows.iter().any(|row| row.text.contains("Verifier:")));
    assert!(rows.iter().any(|row| row.text.contains("Gates:")));
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
        id: "planner".into(),
        role: "Planner".into(),
        lane: "Lane A".into(),
        kind: nit_core::AgentLaneKind::Mock,
        status: AgentStatus::Idle,
        heartbeat_age_secs: 0,
        queue_len: 0,
        current_mission: Some("mis-001".into()),
        last_message: "done".into(),
    });
    state.agents.agents.push(AgentLane {
        id: "coder".into(),
        role: "Coder".into(),
        lane: "Lane B".into(),
        kind: nit_core::AgentLaneKind::Mock,
        status: AgentStatus::Idle,
        heartbeat_age_secs: 0,
        queue_len: 0,
        current_mission: Some("mis-001".into()),
        last_message: "done".into(),
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
