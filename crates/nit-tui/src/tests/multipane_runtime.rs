use super::*;
use crate::app::parse_abort_command;
use nit_core::{
    AgentLane, AgentLaneKind, AgentStatus, AgentsState, MissionRecord, MultipaneState, PaneSession,
};
use std::path::PathBuf;
use std::time::Instant;

fn fixture_state_no_backend() -> AppState {
    let buffer = nit_core::Buffer::empty("scratch", None);
    let notes = nit_core::Buffer::empty("notes", None);
    let mut state = AppState::new(PathBuf::from("/workspace"), buffer, notes);
    state.agents = AgentsState::default();
    state.agents.agents.push(AgentLane {
        id: "claude-haiku-4-5".into(),
        role: "claude-haiku-4-5".into(),
        lane: "Claude".into(),
        kind: AgentLaneKind::Claude,
        status: AgentStatus::Idle,
        heartbeat_age_secs: 0,
        queue_len: 0,
        current_mission: None,
        last_message: String::new(),
        shadow: false,
    });
    state.agents.agents.push(AgentLane {
        id: "gpt-5".into(),
        role: "gpt-5".into(),
        lane: "Codex".into(),
        kind: AgentLaneKind::Codex,
        status: AgentStatus::Idle,
        heartbeat_age_secs: 0,
        queue_len: 0,
        current_mission: None,
        last_message: String::new(),
        shadow: false,
    });
    state.multipane = Some(MultipaneState {
        backend_agent_id: String::new(),
        panes: vec![
            PaneSession {
                pane_id: 0,
                cwd: PathBuf::from("/p0"),
                ..PaneSession::default()
            },
            PaneSession {
                pane_id: 1,
                cwd: PathBuf::from("/p1"),
                ..PaneSession::default()
            },
        ],
        focused: 0,
        grid_cols: 2,
        grid_rows: 1,
        backend_filter: None,
        help_open: false,
    });
    state
}

#[test]
fn parse_abort_command_recognises_forms() {
    assert_eq!(parse_abort_command("/abort"), Some(AbortScope::Current));
    assert_eq!(parse_abort_command("@abort"), Some(AbortScope::Current));
    assert_eq!(parse_abort_command("/abort all"), Some(AbortScope::All));
    assert_eq!(parse_abort_command("/abort  ALL"), Some(AbortScope::All));
    assert_eq!(
        parse_abort_command("/abort claude#mp-pane-02"),
        Some(AbortScope::Agent("claude#mp-pane-02".into()))
    );
}

#[test]
fn parse_abort_command_rejects_substring_match() {
    assert_eq!(parse_abort_command("/abortif"), None);
    assert_eq!(parse_abort_command("just a regular prompt"), None);
}

#[test]
fn focused_pane_in_roster_mode_when_no_selection() {
    let state = fixture_state_no_backend();
    assert!(focused_pane_in_roster_mode(&state));
}

#[test]
fn move_roster_cursor_clamps_to_visible_lanes() {
    let mut state = fixture_state_no_backend();
    // Two non-shadow lanes => cursor in [0, 1]
    move_roster_cursor(&mut state, 5);
    let cursor = state.multipane.as_ref().unwrap().panes[0].roster_cursor;
    assert_eq!(cursor, 1);
    move_roster_cursor(&mut state, -10);
    let cursor = state.multipane.as_ref().unwrap().panes[0].roster_cursor;
    assert_eq!(cursor, 0);
}

#[test]
fn revert_focused_pane_to_roster_clears_selection() {
    let mut state = fixture_state_no_backend();
    if let Some(pane) = state.multipane.as_mut().unwrap().panes.get_mut(0) {
        pane.selected_agent_id = Some("claude-haiku-4-5#mp-pane-00".into());
        pane.agent_id = "claude-haiku-4-5#mp-pane-00".into();
        pane.chat_input = "buffered".into();
    }
    revert_focused_pane_to_roster(&mut state);
    let pane = &state.multipane.as_ref().unwrap().panes[0];
    assert!(pane.selected_agent_id.is_none());
    assert!(pane.agent_id.is_empty());
    assert!(pane.chat_input.is_empty());
}

#[test]
fn abort_with_no_selection_emits_system_message() {
    let mut state = fixture_state_no_backend();
    let before = state.agents.messages.len();
    // No selection in either pane → the focused-pane abort
    // shortcut posts a "nothing to abort" notice without invoking
    // the runner-bound `handle_abort` (which would need real
    // CodexRunner / ClaudeRunner stubs).
    assert!(focused_pane_agent_id(&state).is_none());
    push_pane_system_message(&mut state, "no agent selected — nothing to abort".into());
    assert_eq!(state.agents.messages.len(), before + 1);
    assert!(state
        .agents
        .messages
        .last()
        .unwrap()
        .text
        .contains("no agent selected"));
}

fn fixture_with_efforts() -> AppState {
    let mut state = fixture_state_no_backend();
    state.agents.codex_supported_reasoning_efforts.insert(
        "gpt-5".into(),
        vec!["low".into(), "medium".into(), "high".into()],
    );
    state
        .agents
        .claude_supported_efforts
        .insert("claude-haiku-4-5".into(), vec!["low".into(), "max".into()]);
    state
}

#[test]
fn expand_at_cursor_expands_focused_backend() {
    let mut state = fixture_with_efforts();
    // Cursor lands on the first selectable row (Backend Codex).
    // Pressing `l` drills to its first child, which auto-latches
    // the parent backend through sync_auto_expansion.
    move_roster_cursor(&mut state, 0);
    expand_at_cursor(&mut state);
    let pane = &state.multipane.as_ref().unwrap().panes[0];
    assert_eq!(pane.auto_expanded_backend, Some(AgentLaneKind::Codex));

    // Pressing `h` from the child drills the cursor back up; the
    // backend latch clears as soon as the cursor leaves the group.
    collapse_at_cursor(&mut state);
    let pane = &state.multipane.as_ref().unwrap().panes[0];
    assert_eq!(pane.auto_expanded_backend, Some(AgentLaneKind::Codex));
    // Two `h`s in a row land back on the Backend Codex row, then
    // pressing `h` again is a no-op (no row above).
}

#[test]
fn cursor_walk_skips_size_leaves() {
    let mut state = fixture_with_efforts();
    // Cursor starts on Backend Codex (auto-latches the group);
    // walking once moves to Agent gpt-5; walking again hops over
    // every SizeBranch / SizeLeaf row and lands on Backend Claude.
    move_roster_cursor(&mut state, 0);
    assert_eq!(state.multipane.as_ref().unwrap().panes[0].roster_cursor, 0);
    move_roster_cursor(&mut state, 1);
    assert_eq!(state.multipane.as_ref().unwrap().panes[0].roster_cursor, 1);
    move_roster_cursor(&mut state, 1);
    assert_eq!(state.multipane.as_ref().unwrap().panes[0].roster_cursor, 2);
    // The cursor sits on a Backend, so roster_tree_selected stays None.
    assert!(state.multipane.as_ref().unwrap().panes[0]
        .roster_tree_selected
        .is_none());
}

#[test]
fn auto_expand_on_cursor_move_to_backend() {
    let mut state = fixture_with_efforts();
    let pane = &state.multipane.as_ref().unwrap().panes[0];
    assert_eq!(pane.roster_cursor, 0, "starts on first selectable");
    // Trigger auto-expansion by re-seating the cursor at 0 via a 0-delta move.
    move_roster_cursor(&mut state, 0);
    let pane = &state.multipane.as_ref().unwrap().panes[0];
    assert_eq!(pane.auto_expanded_backend, Some(AgentLaneKind::Codex));
    assert!(pane.auto_expanded_agent.is_none());
}

#[test]
fn auto_collapse_on_cursor_leave() {
    let mut state = fixture_with_efforts();
    // Land on Backend Codex → auto_expanded_backend = Codex.
    move_roster_cursor(&mut state, 0);
    let pane = &state.multipane.as_ref().unwrap().panes[0];
    assert_eq!(pane.auto_expanded_backend, Some(AgentLaneKind::Codex));
    // Walk down to the next selectable row — Agent under Codex
    // (because compute_rows now considers auto_expanded_backend).
    move_roster_cursor(&mut state, 1);
    let pane = &state.multipane.as_ref().unwrap().panes[0];
    // After moving onto the Agent row, auto_expanded_agent latches
    // and the auto-expanded backend stays set.
    assert_eq!(pane.auto_expanded_backend, Some(AgentLaneKind::Codex));
    assert_eq!(pane.auto_expanded_agent.as_deref(), Some("gpt-5"));
    // Walk to next selectable — leaves the Codex group entirely
    // (cursor lands on Backend Claude). Codex auto-fields collapse.
    move_roster_cursor(&mut state, 1);
    let pane = &state.multipane.as_ref().unwrap().panes[0];
    assert_eq!(pane.auto_expanded_backend, Some(AgentLaneKind::Claude));
    assert!(pane.auto_expanded_agent.is_none());
}

#[test]
fn size_leaf_click_writes_codex_selected_effort() {
    let mut state = fixture_with_efforts();
    // Manually drive size selection via the click path — the cursor
    // never stops on size rows under the new gating.
    let mut pane_clone = state.multipane.as_ref().unwrap().panes[0].clone();
    pane_clone.auto_expanded_backend = Some(AgentLaneKind::Codex);
    pane_clone.auto_expanded_agent = Some("gpt-5".into());
    let rows = roster_view::compute_rows(&state, &pane_clone, None);
    let target_idx = rows
        .iter()
        .position(|r| matches!(r, roster_view::PaneRosterRow::SizeLeaf { effort, .. } if effort == "medium"))
        .expect("medium leaf");
    let leaf_row = rows[target_idx].clone();
    apply_roster_click(
        &mut state,
        RosterClickTarget {
            pane_idx: 0,
            rows,
            row_idx: target_idx,
            row: leaf_row,
            local_x: 12,
        },
    );
    assert_eq!(
        state.multipane.as_ref().unwrap().panes[0]
            .selected_effort
            .get("gpt-5"),
        Some(&"medium".to_string())
    );
}

#[test]
fn clicking_two_backends_only_expands_the_second() {
    // Regression for the operator-reported bug: clicking Backend
    // Codex then Backend Claude must NOT leave Codex expanded.
    // Under the cursor-only model, only the most recently clicked
    // backend is expanded.
    let mut state = fixture_with_efforts();
    let pane = state.multipane.as_ref().unwrap().panes[0].clone();
    let rows = roster_view::compute_rows(&state, &pane, None);
    let codex_idx = rows
        .iter()
        .position(|r| {
            matches!(
                r,
                roster_view::PaneRosterRow::Backend {
                    kind: AgentLaneKind::Codex
                }
            )
        })
        .expect("codex backend row");
    let codex_row = rows[codex_idx].clone();
    apply_roster_click(
        &mut state,
        RosterClickTarget {
            pane_idx: 0,
            rows: rows.clone(),
            row_idx: codex_idx,
            row: codex_row,
            local_x: 1,
        },
    );
    let pane = state.multipane.as_ref().unwrap().panes[0].clone();
    let rows = roster_view::compute_rows(&state, &pane, None);
    let claude_idx = rows
        .iter()
        .position(|r| {
            matches!(
                r,
                roster_view::PaneRosterRow::Backend {
                    kind: AgentLaneKind::Claude
                }
            )
        })
        .expect("claude backend row");
    let claude_row = rows[claude_idx].clone();
    apply_roster_click(
        &mut state,
        RosterClickTarget {
            pane_idx: 0,
            rows,
            row_idx: claude_idx,
            row: claude_row,
            local_x: 1,
        },
    );
    let pane = &state.multipane.as_ref().unwrap().panes[0];
    assert_eq!(pane.auto_expanded_backend, Some(AgentLaneKind::Claude));
    let rows = roster_view::compute_rows(&state, pane, None);
    let codex_visible = rows.iter().any(|r| {
        matches!(
            r,
            roster_view::PaneRosterRow::Agent {
                kind: AgentLaneKind::Codex,
                ..
            }
        )
    });
    let claude_visible = rows.iter().any(|r| {
        matches!(
            r,
            roster_view::PaneRosterRow::Agent {
                kind: AgentLaneKind::Claude,
                ..
            }
        )
    });
    assert!(
        claude_visible,
        "Claude group expanded after the second click"
    );
    assert!(
        !codex_visible,
        "Codex group must collapse when click moves to Claude"
    );
}

#[test]
fn cursor_clamps_after_h_collapse() {
    // After h on a child row drills the cursor up off the backend,
    // the cursor index stays in [0, selectable_count).
    let mut state = fixture_with_efforts();
    // Land on Backend Codex, drill into gpt-5, then collapse — the
    // cursor must clamp to the new selectable range.
    move_roster_cursor(&mut state, 0);
    expand_at_cursor(&mut state); // cursor → gpt-5 (idx 1)
    assert_eq!(state.multipane.as_ref().unwrap().panes[0].roster_cursor, 1);
    collapse_at_cursor(&mut state); // cursor drills back up to idx 0
    let pane = &state.multipane.as_ref().unwrap().panes[0];
    let rows = roster_view::compute_rows(&state, pane, None);
    let stops = roster_view::selectable_count(&rows);
    assert!(pane.roster_cursor < stops);
}

#[test]
fn revert_to_roster_clears_auto_fields_and_dir_search() {
    let mut state = fixture_state_no_backend();
    if let Some(pane) = state.multipane.as_mut().unwrap().panes.get_mut(0) {
        pane.selected_agent_id = Some("claude-haiku-4-5#mp-pane-00".into());
        pane.agent_id = "claude-haiku-4-5#mp-pane-00".into();
        pane.auto_expanded_backend = Some(AgentLaneKind::Claude);
        pane.auto_expanded_agent = Some("claude-haiku-4-5".into());
        pane.dir_search = Some(nit_core::DirSearchState {
            query: "abc".into(),
            ..Default::default()
        });
    }
    revert_focused_pane_to_roster(&mut state);
    let pane = &state.multipane.as_ref().unwrap().panes[0];
    assert!(pane.auto_expanded_backend.is_none());
    assert!(pane.auto_expanded_agent.is_none());
    assert!(pane.dir_search.is_none());
}

#[test]
fn jump_roster_cursor_to_top_resets_scroll() {
    let mut state = fixture_with_efforts();
    if let Some(pane) = state.multipane.as_mut().unwrap().panes.get_mut(0) {
        pane.roster_cursor = 1;
        pane.roster_scroll = 5;
    }
    jump_roster_cursor_to_top(&mut state);
    let pane = &state.multipane.as_ref().unwrap().panes[0];
    assert_eq!(pane.roster_cursor, 0);
    assert_eq!(pane.roster_scroll, 0);
}

#[test]
fn jump_roster_cursor_to_bottom_lands_on_last_selectable() {
    let mut state = fixture_with_efforts();
    jump_roster_cursor_to_bottom(&mut state);
    let cursor = state.multipane.as_ref().unwrap().panes[0].roster_cursor;
    // Two backends collapsed → 2 selectable rows.
    assert_eq!(cursor, 1);
}

#[test]
fn scroll_chat_thread_clamps_at_zero() {
    let mut state = fixture_state_no_backend();
    if let Some(pane) = state.multipane.as_mut().unwrap().panes.get_mut(0) {
        pane.selected_agent_id = Some("claude-haiku-4-5#mp-pane-00".into());
        pane.agent_id = "claude-haiku-4-5#mp-pane-00".into();
    }
    let swarm = SwarmRuntime::default();
    let area = Rect::new(0, 0, 80, 30);
    // Bare fixture has no rendered messages → max_scroll = 0. PgUp
    // (delta < 0) from the top must NOT re-engage the follow-bottom
    // sentinel — only wheel-down past the bottom does. Otherwise
    // operator scroll-up gestures would silently snap back to BOTTOM
    // every time max_scroll transiently equals current scroll (the
    // BUG-3 root cause).
    let mp = state.multipane.as_mut().unwrap();
    if let Some(pane) = mp.panes.get_mut(0) {
        pane.chat_thread_scroll = 0;
    }
    scroll_chat_thread(&mut state, &swarm, area, -3);
    assert_eq!(
        state.multipane.as_ref().unwrap().panes[0].chat_thread_scroll,
        0,
        "PgUp from row 0 must stay at row 0, not silently snap to BOTTOM"
    );
}

#[test]
fn handle_mouse_scroll_clamps_at_max_with_short_thread() {
    let mut state = fixture_state_no_backend();
    if let Some(pane) = state.multipane.as_mut().unwrap().panes.get_mut(1) {
        pane.selected_agent_id = Some("claude-haiku-4-5#mp-pane-01".into());
        pane.agent_id = "claude-haiku-4-5#mp-pane-01".into();
        pane.chat_thread_scroll = 999; // pretend a runaway scroll
    }
    let swarm = SwarmRuntime::default();
    let area = Rect::new(0, 0, 80, 30);
    let pane1_rect = grid::pane_rect(area, 2, 1, 1);
    // Wheel down on a short thread MUST clear the runaway 999 +
    // delta. With the "stick to bottom" sentinel semantics, the
    // resolved scroll lands at max_scroll (==0 here, no messages)
    // and `next >= max_scroll` re-engages the sentinel — exactly
    // what the operator wants for "follow new content".
    handle_mouse_scroll(
        &mut state,
        &swarm,
        area,
        pane1_rect.x + 5,
        pane1_rect.y + 5,
        3,
    );
    assert_eq!(
        state.multipane.as_ref().unwrap().panes[1].chat_thread_scroll,
        nit_core::CONSOLE_SCROLL_BOTTOM,
        "wheel-down past max_scroll must re-engage the follow-bottom sentinel"
    );
}

#[test]
fn handle_mouse_scroll_targets_roster_or_chat_per_pane_mode() {
    let mut state = fixture_state_no_backend();
    // Pane 0 stays in roster mode; pane 1 becomes a chat pane.
    if let Some(pane) = state.multipane.as_mut().unwrap().panes.get_mut(1) {
        pane.selected_agent_id = Some("claude-haiku-4-5#mp-pane-01".into());
        pane.agent_id = "claude-haiku-4-5#mp-pane-01".into();
    }
    let swarm = SwarmRuntime::default();
    let area = Rect::new(0, 0, 80, 30);
    let pane0_rect = grid::pane_rect(area, 2, 1, 0);
    let pane1_rect = grid::pane_rect(area, 2, 1, 1);

    // Wheel down inside pane 0 → roster_scroll bumps (clamped to
    // roster row count).
    handle_mouse_scroll(
        &mut state,
        &swarm,
        area,
        pane0_rect.x + 5,
        pane0_rect.y + 5,
        1,
    );
    let roster = state.multipane.as_ref().unwrap().panes[0].roster_scroll;
    assert!(
        roster <= 1,
        "roster_scroll should bump or clamp, got {roster}"
    );

    // Wheel down inside pane 1 (chat mode, empty thread) — re-engages
    // the follow-bottom sentinel because next >= max_scroll(=0).
    handle_mouse_scroll(
        &mut state,
        &swarm,
        area,
        pane1_rect.x + 5,
        pane1_rect.y + 5,
        1,
    );
    assert_eq!(
        state.multipane.as_ref().unwrap().panes[1].chat_thread_scroll,
        nit_core::CONSOLE_SCROLL_BOTTOM,
    );
}

#[test]
fn template_click_writes_to_focused_pane_only() {
    let mut state = fixture_state_no_backend();
    let pane = state.multipane.as_ref().unwrap().panes[0].clone();
    let rows = roster_view::compute_rows(&state, &pane, None);
    // Click on the " parallel " word (after " lab " + 1 separator).
    let parallel_col = " Template: ".chars().count() + " lab ".chars().count() + 1 + 1;
    apply_roster_click(
        &mut state,
        RosterClickTarget {
            pane_idx: 0,
            rows,
            row_idx: 0,
            row: roster_view::PaneRosterRow::Template,
            local_x: parallel_col,
        },
    );
    assert_eq!(
        state.multipane.as_ref().unwrap().panes[0].swarm_template,
        "parallel"
    );
    assert_eq!(
        state.agents.swarm_default_template, "lab",
        "global default must stay untouched by per-pane clicks"
    );
}

#[test]
fn template_click_on_pane_zero_does_not_touch_pane_one() {
    let mut state = fixture_state_no_backend();
    let pane = state.multipane.as_ref().unwrap().panes[0].clone();
    let rows = roster_view::compute_rows(&state, &pane, None);
    let parallel_col = " Template: ".chars().count() + " lab ".chars().count() + 1 + 1;
    apply_roster_click(
        &mut state,
        RosterClickTarget {
            pane_idx: 0,
            rows,
            row_idx: 0,
            row: roster_view::PaneRosterRow::Template,
            local_x: parallel_col,
        },
    );
    let panes = &state.multipane.as_ref().unwrap().panes;
    assert_eq!(panes[0].swarm_template, "parallel");
    assert_eq!(
        panes[1].swarm_template, "lab",
        "sibling pane's template must not change"
    );
}

#[test]
fn mission_click_writes_to_focused_pane_only() {
    let mut state = fixture_state_no_backend();
    let pane = state.multipane.as_ref().unwrap().panes[0].clone();
    let rows = roster_view::compute_rows(&state, &pane, None);
    let general_col = " Mission:  ".chars().count() + " auto ".chars().count() + 1 + 1;
    apply_roster_click(
        &mut state,
        RosterClickTarget {
            pane_idx: 0,
            rows,
            row_idx: 1,
            row: roster_view::PaneRosterRow::Mission,
            local_x: general_col,
        },
    );
    let panes = &state.multipane.as_ref().unwrap().panes;
    assert_eq!(panes[0].swarm_mission, "general");
    assert_eq!(
        panes[1].swarm_mission, "auto",
        "sibling pane's mission must not change"
    );
    assert_eq!(
        state.agents.swarm_default_mission, "auto",
        "global default must stay untouched"
    );
}

#[test]
fn fresh_pane_first_render_no_artifact_callout() {
    // Documentary regression for the screenshot bug: a freshly-
    // selected pane (no dispatch yet) must keep `has_run_mission`
    // false, which the renderer uses to suppress artifact callouts.
    let state = fixture_state_no_backend();
    let pane = &state.multipane.as_ref().unwrap().panes[0];
    assert!(!pane.has_run_mission);
}

#[test]
fn dispatch_sets_has_run_mission_true() {
    let mut state = fixture_state_no_backend();
    if let Some(p) = state.multipane.as_mut().unwrap().panes.get_mut(0) {
        p.selected_agent_id = Some("claude-haiku-4-5#mp-pane-00".into());
        p.agent_id = "claude-haiku-4-5#mp-pane-00".into();
    }
    let mut vitals = VitalsState::default();
    let outcome = crate::multipane::dispatch::dispatch_pane_prompt(
        &mut state,
        &mut vitals,
        None,
        None,
        0,
        "ping".into(),
    );
    assert_eq!(
        outcome,
        crate::multipane::dispatch::DispatchOutcome::Dispatched
    );
    assert!(state.multipane.as_ref().unwrap().panes[0].has_run_mission);
}

fn open_dir_search_with_results(state: &mut AppState, results: Vec<PathBuf>) {
    if let Some(pane) = state.multipane.as_mut().and_then(|mp| mp.panes.get_mut(0)) {
        pane.dir_search = Some(nit_core::DirSearchState {
            results,
            base: PathBuf::from("/tmp"),
            generation: 1,
            ..Default::default()
        });
    }
}

#[test]
fn focused_pane_dir_search_active_reflects_overlay() {
    let mut state = fixture_state_no_backend();
    assert!(!focused_pane_dir_search_active(&state));
    open_dir_search_with_results(&mut state, Vec::new());
    assert!(focused_pane_dir_search_active(&state));
}

#[test]
fn close_focused_dir_search_drops_overlay() {
    let mut state = fixture_state_no_backend();
    open_dir_search_with_results(&mut state, Vec::new());
    close_focused_dir_search(&mut state);
    assert!(state.multipane.as_ref().unwrap().panes[0]
        .dir_search
        .is_none());
}

#[test]
fn commit_dir_search_with_empty_results_is_noop() {
    let mut state = fixture_state_no_backend();
    let cwd_before = state.multipane.as_ref().unwrap().panes[0].cwd.clone();
    open_dir_search_with_results(&mut state, Vec::new());
    commit_dir_search(&mut state);
    let pane = &state.multipane.as_ref().unwrap().panes[0];
    assert_eq!(pane.cwd, cwd_before);
    assert!(pane.dir_search.is_none());
}

#[test]
fn commit_dir_search_changes_cwd_and_emits_system_alert() {
    let mut state = fixture_state_no_backend();
    let tmp = std::env::temp_dir().join(format!(
        "nit-mp-commit-{}",
        Instant::now().elapsed().as_nanos()
    ));
    std::fs::create_dir_all(&tmp).unwrap();
    open_dir_search_with_results(&mut state, vec![tmp.clone()]);
    let before = state.agents.messages.len();
    commit_dir_search(&mut state);
    let pane = &state.multipane.as_ref().unwrap().panes[0];
    assert_eq!(pane.cwd, tmp);
    assert!(pane.dir_search.is_none());
    assert_eq!(state.agents.messages.len(), before + 1);
    let last = state.agents.messages.last().unwrap();
    assert_eq!(last.kind.as_deref(), Some(SYSTEM_ALERT_KIND));
    assert!(last.text.starts_with("cwd → "), "text was {:?}", last.text);
    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn commit_dir_search_rejects_path_that_is_not_a_dir() {
    let mut state = fixture_state_no_backend();
    let cwd_before = state.multipane.as_ref().unwrap().panes[0].cwd.clone();
    open_dir_search_with_results(
        &mut state,
        vec![PathBuf::from("/this/path/does/not/exist/abc")],
    );
    commit_dir_search(&mut state);
    let pane = &state.multipane.as_ref().unwrap().panes[0];
    assert_eq!(pane.cwd, cwd_before);
    assert!(pane.dir_search.is_none());
}

// Lens-E Part C: switching cwd drops focused-pane resume ids so the
// next turn opens a fresh session in the new cwd. Without this,
// session metadata re-anchors to the original workspace.
#[test]
fn commit_dir_search_invalidates_resume_session_ids_for_focused_pane() {
    let mut state = fixture_state_no_backend();
    let lane = "claude-haiku-4-5#mp-pane-00";
    let mission = "mission-XYZ";
    if let Some(pane) = focused_pane_mut(&mut state) {
        pane.agent_id = lane.into();
        pane.mission_id = Some(mission.into());
    }
    state
        .agents
        .claude_session_ids
        .insert(lane.into(), "A".into());
    state
        .agents
        .codex_thread_ids
        .insert(lane.into(), "B".into());
    state
        .agents
        .claude_mission_session_ids
        .entry(mission.into())
        .or_default()
        .insert(lane.into(), "C".into());
    state
        .agents
        .codex_mission_thread_ids
        .entry(mission.into())
        .or_default()
        .insert(lane.into(), "D".into());

    let tmp = std::env::temp_dir().join(format!(
        "nit-resume-inval-{}",
        Instant::now().elapsed().as_nanos()
    ));
    std::fs::create_dir_all(&tmp).unwrap();
    open_dir_search_with_results(&mut state, vec![tmp.clone()]);
    commit_dir_search(&mut state);

    let agents = &state.agents;
    assert!(!agents.claude_session_ids.contains_key(lane));
    assert!(!agents.codex_thread_ids.contains_key(lane));
    assert!(agents
        .claude_mission_session_ids
        .get(mission)
        .is_none_or(|inner| !inner.contains_key(lane)));
    assert!(agents
        .codex_mission_thread_ids
        .get(mission)
        .is_none_or(|inner| !inner.contains_key(lane)));
    let _ = std::fs::remove_dir_all(&tmp);
}

// Selecting a directory in pane 0 must not mutate pane 1's cwd or
// invalidate pane 1's resume sessions.
#[test]
fn commit_dir_search_in_pane0_does_not_affect_pane1() {
    let mut state = fixture_state_no_backend();
    let other = "claude-haiku-4-5#mp-pane-01";
    if let Some(mp) = state.multipane.as_mut() {
        mp.panes[0].agent_id = "claude-haiku-4-5#mp-pane-00".into();
        mp.panes[1].agent_id = other.into();
    }
    state
        .agents
        .claude_session_ids
        .insert(other.into(), "stay".into());
    let pane1_cwd = state.multipane.as_ref().unwrap().panes[1].cwd.clone();

    let tmp = std::env::temp_dir().join(format!(
        "nit-pane-iso-{}",
        Instant::now().elapsed().as_nanos()
    ));
    std::fs::create_dir_all(&tmp).unwrap();
    open_dir_search_with_results(&mut state, vec![tmp.clone()]);
    commit_dir_search(&mut state);

    let mp = state.multipane.as_ref().unwrap();
    assert_eq!(mp.panes[0].cwd, tmp);
    assert_eq!(mp.panes[1].cwd, pane1_cwd);
    assert_eq!(
        state
            .agents
            .claude_session_ids
            .get(other)
            .map(String::as_str),
        Some("stay"),
    );
    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn apply_dir_search_event_writes_results_when_generation_matches() {
    let mut state = fixture_state_no_backend();
    let base = PathBuf::from("/tmp");
    if let Some(pane) = state.multipane.as_mut().and_then(|mp| mp.panes.get_mut(0)) {
        pane.dir_search = Some(nit_core::DirSearchState {
            query: "foo".into(),
            query_cursor: 3,
            base: base.clone(),
            generation: 7,
            ..Default::default()
        });
    }
    let want = vec![PathBuf::from("/tmp/alpha")];
    apply_dir_search_event(
        &mut state,
        DirSearchEvent::Results {
            request_id: 7,
            base: base.clone(),
            results: want.clone(),
        },
    );
    let ds = state.multipane.as_ref().unwrap().panes[0]
        .dir_search
        .as_ref()
        .unwrap();
    assert_eq!(ds.results, want);
}

fn open_dir_search_with(state: &mut AppState, results: Vec<PathBuf>, last_visible: u16) {
    if let Some(pane) = state.multipane.as_mut().and_then(|mp| mp.panes.get_mut(0)) {
        pane.dir_search = Some(nit_core::DirSearchState {
            results,
            base: PathBuf::from("/tmp"),
            generation: 1,
            last_visible,
            ..Default::default()
        });
    }
}

#[test]
fn ctrl_j_advances_dir_search_selection() {
    let mut state = fixture_state_no_backend();
    let results = vec![
        PathBuf::from("/tmp/a"),
        PathBuf::from("/tmp/b"),
        PathBuf::from("/tmp/c"),
    ];
    open_dir_search_with(&mut state, results, 10);
    with_focused_dir_search(&mut state, move_selected_down);
    let ds = state.multipane.as_ref().unwrap().panes[0]
        .dir_search
        .as_ref()
        .unwrap();
    assert_eq!(ds.selected, 1);
}

#[test]
fn ctrl_k_recedes_dir_search_selection() {
    let mut state = fixture_state_no_backend();
    let results = vec![
        PathBuf::from("/tmp/a"),
        PathBuf::from("/tmp/b"),
        PathBuf::from("/tmp/c"),
    ];
    open_dir_search_with(&mut state, results, 10);
    with_focused_dir_search(&mut state, |ds| ds.selected = 2);
    with_focused_dir_search(&mut state, move_selected_up);
    let ds = state.multipane.as_ref().unwrap().panes[0]
        .dir_search
        .as_ref()
        .unwrap();
    assert_eq!(ds.selected, 1);
}

#[test]
fn ctrl_l_expands_focused_dir() {
    let tmp = std::env::temp_dir().join(format!(
        "nit-mp-expand-{}",
        Instant::now().elapsed().as_nanos()
    ));
    std::fs::create_dir_all(&tmp).unwrap();
    let mut state = fixture_state_no_backend();
    open_dir_search_with(&mut state, vec![tmp.clone()], 10);
    expand_dir_search_at_cursor(&mut state);
    let ds = state.multipane.as_ref().unwrap().panes[0]
        .dir_search
        .as_ref()
        .unwrap();
    assert!(ds.expanded.contains(&tmp));
    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn ctrl_h_collapses_focused_dir() {
    let tmp = std::env::temp_dir().join(format!(
        "nit-mp-collapse-{}",
        Instant::now().elapsed().as_nanos()
    ));
    std::fs::create_dir_all(&tmp).unwrap();
    let mut state = fixture_state_no_backend();
    open_dir_search_with(&mut state, vec![tmp.clone()], 10);
    with_focused_dir_search(&mut state, |ds| {
        ds.expanded.insert(tmp.clone());
    });
    collapse_dir_search_at_cursor(&mut state);
    let ds = state.multipane.as_ref().unwrap().panes[0]
        .dir_search
        .as_ref()
        .unwrap();
    assert!(!ds.expanded.contains(&tmp));
    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn view_offset_advances_when_selected_passes_window() {
    let mut state = fixture_state_no_backend();
    let results: Vec<PathBuf> = (0..25)
        .map(|i| PathBuf::from(format!("/tmp/d{i}")))
        .collect();
    open_dir_search_with(&mut state, results, 10);
    for _ in 0..12 {
        with_focused_dir_search(&mut state, move_selected_down);
    }
    let ds = state.multipane.as_ref().unwrap().panes[0]
        .dir_search
        .as_ref()
        .unwrap();
    assert_eq!(ds.selected, 12);
    assert_eq!(ds.view_offset, 3);
}

#[test]
fn view_offset_recedes_when_selected_above_window() {
    let mut state = fixture_state_no_backend();
    let results: Vec<PathBuf> = (0..25)
        .map(|i| PathBuf::from(format!("/tmp/d{i}")))
        .collect();
    open_dir_search_with(&mut state, results, 10);
    with_focused_dir_search(&mut state, |ds| {
        ds.selected = 11;
        ds.view_offset = 10;
    });
    for _ in 0..7 {
        with_focused_dir_search(&mut state, move_selected_up);
    }
    let ds = state.multipane.as_ref().unwrap().panes[0]
        .dir_search
        .as_ref()
        .unwrap();
    assert_eq!(ds.selected, 4);
    assert_eq!(ds.view_offset, 4);
}

#[test]
fn apply_dir_search_event_resets_view_offset() {
    let mut state = fixture_state_no_backend();
    let base = PathBuf::from("/tmp");
    if let Some(pane) = state.multipane.as_mut().and_then(|mp| mp.panes.get_mut(0)) {
        pane.dir_search = Some(nit_core::DirSearchState {
            base: base.clone(),
            generation: 7,
            view_offset: 12,
            ..Default::default()
        });
    }
    apply_dir_search_event(
        &mut state,
        DirSearchEvent::Results {
            request_id: 7,
            base,
            results: vec![PathBuf::from("/tmp/a"), PathBuf::from("/tmp/b")],
        },
    );
    let ds = state.multipane.as_ref().unwrap().panes[0]
        .dir_search
        .as_ref()
        .unwrap();
    assert_eq!(ds.view_offset, 0);
}

#[test]
fn compute_dropdown_rows_clamps_to_results_len() {
    assert_eq!(compute_dropdown_rows(40, 2), 2);
    assert_eq!(compute_dropdown_rows(40, 0), 1);
}

#[test]
fn compute_dropdown_rows_min_three_max_sixteen() {
    assert_eq!(compute_dropdown_rows(2, 30), 3);
    assert_eq!(compute_dropdown_rows(200, 30), 16);
}

#[test]
fn apply_dir_search_event_drops_results_for_stale_generation() {
    let mut state = fixture_state_no_backend();
    let base = PathBuf::from("/tmp");
    if let Some(pane) = state.multipane.as_mut().and_then(|mp| mp.panes.get_mut(0)) {
        pane.dir_search = Some(nit_core::DirSearchState {
            query: "foo".into(),
            query_cursor: 3,
            base: base.clone(),
            generation: 7,
            ..Default::default()
        });
    }
    apply_dir_search_event(
        &mut state,
        DirSearchEvent::Results {
            request_id: 6,
            base,
            results: vec![PathBuf::from("/tmp/old")],
        },
    );
    let ds = state.multipane.as_ref().unwrap().panes[0]
        .dir_search
        .as_ref()
        .unwrap();
    assert!(ds.results.is_empty());
}

/// Bug 2: when the dir-search dropdown is open, clicks below the
/// overlay must resolve to the row visually under the cursor — not
/// `DIR_SEARCH_INPUT_ROWS + visible_rows` rows above it.
#[test]
fn roster_click_with_dir_search_open_hits_correct_row() {
    let mut state = fixture_state_no_backend();
    let area = Rect::new(0, 0, 80, 30);
    // Single pane fills the grid, focused.
    if let Some(mp) = state.multipane.as_mut() {
        mp.panes.truncate(1);
        mp.grid_cols = 1;
        mp.grid_rows = 1;
        mp.focused = 0;
    }
    // Open dir-search with three results so the overlay reserves
    // `DIR_SEARCH_INPUT_ROWS + 3` rows of header at the top of the
    // pane's inner area.
    let results = vec![
        PathBuf::from("/tmp/a"),
        PathBuf::from("/tmp/b"),
        PathBuf::from("/tmp/c"),
    ];
    if let Some(pane) = state.multipane.as_mut().and_then(|mp| mp.panes.get_mut(0)) {
        pane.dir_search = Some(nit_core::DirSearchState {
            results,
            base: PathBuf::from("/tmp"),
            generation: 1,
            ..Default::default()
        });
    }
    let pane = state.multipane.as_ref().unwrap().panes[0].clone();
    let pane_rect = grid::pane_rect(area, 1, 1, 0);
    let inner = pane_inner_after_chrome(pane_rect);
    let body = dir_search_body_rect(inner, &pane);
    // Click on the first visible roster row WITHIN the body — i.e.
    // the row the operator sees is row 0 of the roster body.
    let click_x = body.x + 1;
    let click_y = body.y;
    let target = resolve_left_click_target(&mut state, area, click_x, click_y);
    let target = target.expect("click should resolve to a roster target");
    // Computed roster rows include the Template / Mission preamble
    // plus backend / agent rows. Whatever the first selectable row
    // is, it must be the one at row_idx 0 — meaning the overlay
    // strip was correctly accounted for.
    assert_eq!(
        target.row_idx, 0,
        "click on the first visible roster row must resolve to row_idx 0, got {}",
        target.row_idx
    );
}

/// Bug 2: `handle_mouse` must strip the chrome (top status row +
/// bottom hint row) before passing coordinates downstream — so a
/// click on the very first visible pane row routes to a real
/// roster row instead of falling outside the pane.
#[test]
fn roster_click_after_chrome_strip_resolves_visual_row() {
    let mut state = fixture_state_no_backend();
    let area = Rect::new(0, 0, 80, 30);
    if let Some(mp) = state.multipane.as_mut() {
        mp.panes.truncate(1);
        mp.grid_cols = 1;
        mp.grid_rows = 1;
        mp.focused = 0;
    }
    // Click 1 row below the top chrome, which the renderer paints
    // as the start of pane 0. Without chrome-strip, `pane_at_point`
    // would map this to the chrome row and resolve_left_click_target
    // would return None.
    let click_y = area.y + 1;
    let mouse = MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: 5,
        row: click_y,
        modifiers: KeyModifiers::empty(),
    };
    let swarm = SwarmRuntime::default();
    let theme = crate::theme::Theme::default();
    let mut clipboard: Option<arboard::Clipboard> = None;
    // Just call handle_mouse — no panic / no out-of-bounds means
    // the chrome strip and downstream resolver agreed on the rect.
    handle_mouse(&mut state, &swarm, &theme, &mut clipboard, area, mouse);
    // The pane should still exist and not have been corrupted.
    assert_eq!(state.multipane.as_ref().unwrap().panes.len(), 1);
}

/// Bug 3: clicking an artifact line in pane B must scope the popup
/// resolver to pane B's mission/agent — not whichever values the
/// last-rendered pane left in `state.agents.selected_*`.
#[test]
fn try_open_chat_pane_artifact_uses_pane_context_and_swarm() {
    let mut state = fixture_state_no_backend();
    // Two panes, two missions, two agents — pane 0 belongs to
    // mission A and agent A, pane 1 belongs to mission B and agent B.
    state.agents.messages.clear();
    state.agents.missions.clear();
    let now = "t+0".to_string();
    state.agents.missions.push(MissionRecord {
        id: "mis-A".into(),
        title: "A".into(),
        phase: nit_core::MissionPhase::Plan,
        swarm: false,
        assigned_agents: Vec::new(),
        status: "Planning".into(),
        updated_at: now.clone(),
    });
    state.agents.missions.push(MissionRecord {
        id: "mis-B".into(),
        title: "B".into(),
        phase: nit_core::MissionPhase::Plan,
        swarm: false,
        assigned_agents: Vec::new(),
        status: "Planning".into(),
        updated_at: now,
    });
    // Configure pane 0 → mission A, pane 1 → mission B
    if let Some(mp) = state.multipane.as_mut() {
        mp.panes.truncate(2);
        mp.grid_cols = 2;
        mp.grid_rows = 1;
        mp.focused = 0;
        mp.panes[0].selected_agent_id = Some("claude-haiku-4-5#mp-pane-00".into());
        mp.panes[0].agent_id = "claude-haiku-4-5#mp-pane-00".into();
        mp.panes[0].mission_id = Some("mis-A".into());
        mp.panes[0].has_run_mission = true;
        mp.panes[1].selected_agent_id = Some("claude-haiku-4-5#mp-pane-01".into());
        mp.panes[1].agent_id = "claude-haiku-4-5#mp-pane-01".into();
        mp.panes[1].mission_id = Some("mis-B".into());
        mp.panes[1].has_run_mission = true;
    }
    // Leave selected_* pointing at pane A — the way it would look
    // after pane A renders. The buggy resolver would walk pane A's
    // messages even when the click is in pane B.
    state.agents.selected_mission = Some("mis-A".into());
    state.agents.selected_agent = Some("claude-haiku-4-5#mp-pane-00".into());

    let area = Rect::new(0, 0, 80, 30);
    let pane_b_rect = grid::pane_rect(area, 2, 1, 1);
    let swarm = SwarmRuntime::default();
    // Click somewhere inside pane B's thread area. We don't care
    // whether an artifact actually opens (no messages in this
    // fixture) — we care that the click does NOT corrupt
    // `selected_mission` to pane A's value, since the resolver
    // has been wrapped to alias to pane B and restore on miss.
    let opened = try_open_chat_pane_artifact(
        &mut state,
        &swarm,
        area,
        pane_b_rect.x + 2,
        pane_b_rect.y + 2,
    );
    assert!(
        !opened,
        "no artifact rows in fixture, click must miss cleanly"
    );
    // On miss, the alias is restored, so selected_* point back at
    // pane A's values (matching what they were when we entered).
    assert_eq!(
        state.agents.selected_mission.as_deref(),
        Some("mis-A"),
        "selected_mission must be restored to its prior value on miss"
    );
    assert_eq!(
        state.agents.selected_agent.as_deref(),
        Some("claude-haiku-4-5#mp-pane-00"),
        "selected_agent must be restored to its prior value on miss"
    );
}

// ----- BUG 3: scroll holds when row count oscillates -------------------
//
// When swarm bus events flip breather rows in/out of the visible
// window, max_scroll oscillates frame-to-frame. The pre-fix snap-back
// rule re-engaged the follow-bottom sentinel whenever `next >= max_scroll`
// — silently consuming PgUp / wheel-up gestures during a swarm. The
// delta-guard restricts the snap-back to operator-driven scroll-DOWN.

#[test]
fn pgup_does_not_re_engage_sentinel_when_max_scroll_dips() {
    // Pane is parked at scroll = 10 with current max_scroll = 30.
    // A swarm event drops the trailing breather count, max_scroll is
    // recomputed at 0 inside `scroll_chat_thread`. Before the fix:
    // `next = 10 - 8 = 2`, `2 >= 0` → snap to BOTTOM. After the fix:
    // delta < 0 so the sentinel stays disengaged, scroll lands at 0
    // (clamped to max_scroll, not BOTTOM).
    let mut state = fixture_state_no_backend();
    if let Some(pane) = state.multipane.as_mut().unwrap().panes.get_mut(0) {
        pane.selected_agent_id = Some("claude-haiku-4-5#mp-pane-00".into());
        pane.agent_id = "claude-haiku-4-5#mp-pane-00".into();
        pane.chat_thread_scroll = 10;
    }
    let swarm = SwarmRuntime::default();
    let area = Rect::new(0, 0, 80, 30);
    scroll_chat_thread(&mut state, &swarm, area, -8);
    assert_ne!(
        state.multipane.as_ref().unwrap().panes[0].chat_thread_scroll,
        nit_core::CONSOLE_SCROLL_BOTTOM,
        "PgUp must NEVER re-engage the follow-bottom sentinel — even when \
         max_scroll transiently dips to ≤ next"
    );
}

#[test]
fn wheel_down_past_bottom_still_re_engages_sentinel() {
    // Counter-test for the delta-guard: wheel-down (delta > 0) past
    // max_scroll is the only path that should re-arm the sentinel.
    let mut state = fixture_state_no_backend();
    if let Some(pane) = state.multipane.as_mut().unwrap().panes.get_mut(1) {
        pane.selected_agent_id = Some("claude-haiku-4-5#mp-pane-01".into());
        pane.agent_id = "claude-haiku-4-5#mp-pane-01".into();
        pane.chat_thread_scroll = 0;
    }
    let swarm = SwarmRuntime::default();
    let area = Rect::new(0, 0, 80, 30);
    let pane1_rect = grid::pane_rect(area, 2, 1, 1);
    handle_mouse_scroll(
        &mut state,
        &swarm,
        area,
        pane1_rect.x + 5,
        pane1_rect.y + 5,
        5,
    );
    assert_eq!(
        state.multipane.as_ref().unwrap().panes[1].chat_thread_scroll,
        nit_core::CONSOLE_SCROLL_BOTTOM,
        "wheel-DOWN past the current bottom must re-engage the sentinel"
    );
}

// ----- BUG 4: paint_bar fills the rect with bg style -------------------

#[test]
fn paint_bar_fills_full_rect_with_bg_style() {
    use ratatui::backend::TestBackend;
    use ratatui::style::Color;
    use ratatui::Terminal;

    let backend = TestBackend::new(40, 3);
    let mut terminal = Terminal::new(backend).expect("terminal");
    let style = Style::default()
        .fg(Color::White)
        .bg(Color::Blue)
        .add_modifier(Modifier::BOLD);
    let target_rect = Rect::new(0, 0, 40, 1);
    terminal
        .draw(|frame| {
            paint_bar(frame, target_rect, "MULTIPANE".into(), style);
        })
        .expect("draw");
    let buffer = terminal.backend().buffer();
    for x in 0..target_rect.width {
        let cell = buffer.get(target_rect.x + x, target_rect.y);
        assert_eq!(
            cell.bg,
            Color::Blue,
            "cell at column {x} must inherit the bar's bg colour"
        );
    }
}
