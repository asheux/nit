//! Breather row construction split out of `agent_console_view`.
//!
//! Two surface concerns live here: the inline breather (under a
//! single user prompt that triggered an active turn) and the bottom
//! "Working ..." block (when something is still running and there's
//! no inline anchor). Both share the same agent table layout but
//! diverge on label, scope filtering, and shadow-pipeline awareness.
//!
//! The 47dd493 `lane_in_pane_scope` filter — load-bearing for
//! multipane breather isolation — stays here so the trigger
//! (`any_remaining`) and the bottom-block content filter agree on
//! which lanes belong to this pane.

use std::time::Instant;

use nit_core::{
    AgentConsoleRow as ThreadRow, AgentConsoleRowKind as ThreadRowKind, AgentStatus, AppState,
};

use super::{
    agent_roster_label, append_swarm_meta_footer_rows, ecg_indicator, fit_left, fit_right,
    format_agent_stage_label, format_duration_compact, format_message_rows, pad_to_width, pulse_on,
    swarm_exec_label, visible_messages_grouped_for_pane,
};
use crate::swarm::{chat_clone_base_id, SwarmRuntime};

pub fn build_pane_thread_rows_with_breathers(
    state: &AppState,
    swarm: Option<&SwarmRuntime>,
    agent: Option<&str>,
    mission: Option<&str>,
    width: usize,
    suppress_artifacts: bool,
) -> Vec<ThreadRow> {
    build_pane_thread_rows_with_breathers_for_pane(
        state,
        swarm,
        None,
        agent,
        mission,
        width,
        suppress_artifacts,
    )
}

/// True when `lane` belongs to the rendered pane's scope. Used by the
/// `any_remaining` trigger to match the bottom-block content filter, so
/// sibling-pane activity does not inflate this pane's breather. Single-
/// pane callers (`pane_idx = None`) accept every lane.
fn lane_in_pane_scope(
    state: &AppState,
    lane: &nit_core::AgentLane,
    pane_idx: Option<usize>,
    mission: Option<&str>,
    agent: Option<&str>,
) -> bool {
    if pane_idx.is_none() {
        return true;
    }
    if let Some(mid) = mission {
        if lane.current_mission.as_deref() == Some(mid) {
            return true;
        }
        let queued_codex = state
            .agents
            .queued_codex_turns
            .iter()
            .any(|t| t.agent_id == lane.id && t.mission_id.as_deref() == Some(mid));
        let queued_claude = state
            .agents
            .queued_claude_turns
            .iter()
            .any(|t| t.agent_id == lane.id && t.mission_id.as_deref() == Some(mid));
        return queued_codex || queued_claude;
    }
    agent.is_none_or(|ag| lane.id == ag)
}

/// Pane-aware variant of [`build_pane_thread_rows_with_breathers`].
/// `pane_idx = Some(n)` activates the defense-in-depth filter so any
/// `Broadcast` (or stray Agent reply) from another pane is dropped at
/// the renderer.
pub fn build_pane_thread_rows_with_breathers_for_pane(
    state: &AppState,
    swarm: Option<&SwarmRuntime>,
    pane_idx: Option<usize>,
    agent: Option<&str>,
    mission: Option<&str>,
    width: usize,
    suppress_artifacts: bool,
) -> Vec<ThreadRow> {
    let ordered = visible_messages_grouped_for_pane(state, pane_idx, mission, agent);
    let pulse_on = pulse_on(state);

    let mut pending_by_prompt: std::collections::HashMap<usize, Vec<String>> =
        std::collections::HashMap::new();
    for (agent_id, &prompt_idx) in state
        .agents
        .codex_turn_prompt_idx
        .iter()
        .chain(state.agents.claude_turn_prompt_idx.iter())
    {
        let is_active = state.agents.active_turns.contains_key(agent_id)
            || state
                .agents
                .queued_codex_turns
                .iter()
                .any(|t| t.agent_id == *agent_id)
            || state
                .agents
                .queued_claude_turns
                .iter()
                .any(|t| t.agent_id == *agent_id);
        if is_active {
            pending_by_prompt
                .entry(prompt_idx)
                .or_default()
                .push(agent_id.clone());
        }
    }

    let mut inline_shown = std::collections::HashSet::<String>::new();
    let mut combined: Vec<ThreadRow> = Vec::new();
    for (msg_idx, msg) in ordered {
        let msg_rows = format_message_rows(state, swarm, msg, width);
        combined.extend(msg_rows);
        let is_user_prompt = msg.agent_id.is_none();
        if !is_user_prompt {
            continue;
        }
        let Some(agent_ids) = pending_by_prompt.get(&msg_idx) else {
            continue;
        };
        combined.extend(inline_breather_rows(state, agent_ids, pulse_on, width));
        for id in agent_ids {
            inline_shown.insert(id.clone());
        }
    }

    // Pane mode (`pane_idx = Some(_)`): the trigger must match the
    // bottom-block content filter. Otherwise sibling-pane activity fires
    // `any_remaining = true` here and `breather_rows_for_user_prompt`
    // re-emits this pane's own lane that `inline_breather_rows` already
    // showed — the doubled breather the operator hit when pane 4
    // dispatched while pane 0 was active.
    let any_remaining = state.agents.agents.iter().any(|a| {
        if !lane_in_pane_scope(state, a, pane_idx, mission, agent) {
            return false;
        }
        if inline_shown.contains(&a.id) {
            return false;
        }
        state.agents.active_turns.contains_key(&a.id)
            || state
                .agents
                .queued_codex_turns
                .iter()
                .any(|t| t.agent_id == a.id)
            || state
                .agents
                .queued_claude_turns
                .iter()
                .any(|t| t.agent_id == a.id)
    });
    let has_swarm_context =
        mission.is_some_and(|mid| state.agents.missions.iter().any(|m| m.id == mid && m.swarm));
    if any_remaining || (has_swarm_context && inline_shown.is_empty()) {
        combined.extend(breather_rows_for_user_prompt(
            state, swarm, pulse_on, width, true,
        ));
    }

    if suppress_artifacts {
        for row in &mut combined {
            if matches!(row.kind, ThreadRowKind::ArtifactLink) {
                row.text = row.text.replace(" (see ARTIFACTS)", "");
                row.kind = ThreadRowKind::Agent;
            }
        }
    }
    combined
}

pub(super) fn inline_breather_rows(
    state: &AppState,
    agent_ids: &[String],
    pulse_on: bool,
    width: usize,
) -> Vec<ThreadRow> {
    let now = Instant::now();
    let width = width.max(1);
    let elap_w = 6usize;
    let hb_w = 4usize;
    let out_w = 4usize;
    let times_and_spacing = elap_w + hb_w + out_w + 3;
    let agent_w = width.saturating_sub(times_and_spacing + 1).max(1);

    let seed_id = agent_ids.first().map(String::as_str);
    let ecg = ecg_indicator(state.metrics.frame_count, seed_id, pulse_on, true);

    let mut rows = Vec::new();
    rows.push(ThreadRow {
        text: format!("{ecg} Working ..."),
        kind: ThreadRowKind::Breather,
    });
    rows.push(ThreadRow {
        text: pad_to_width(
            &format!(
                "{} {} {} {}",
                fit_left("AGENT", agent_w),
                fit_right("ELAP", elap_w),
                fit_right("HB", hb_w),
                fit_right("OUT", out_w),
            ),
            width,
        ),
        kind: ThreadRowKind::StatusHeader,
    });
    for id in agent_ids {
        let agent = state.agents.agents_get(id.as_str());
        let badge = agent
            .map(agent_roster_label)
            .unwrap_or_else(|| id.to_string());
        let turn = state.agents.active_turns.get(id.as_str());
        let stage_raw = if let Some(turn) = turn {
            turn.stage.as_deref().unwrap_or("starting")
        } else {
            "queued"
        };
        let stage = agent
            .map(|a| format_agent_stage_label(state, a, stage_raw))
            .unwrap_or_else(|| stage_raw.to_string());
        let suppress = stage_raw == "queued";
        let (elapsed_s, hb_s, out_s) = if suppress {
            ("--".into(), "--".into(), "--".into())
        } else {
            let elapsed = turn.and_then(|t| now.checked_duration_since(t.started_at));
            let hb = turn
                .and_then(|t| now.checked_duration_since(t.last_heartbeat_at))
                .map(|d| d.as_secs());
            let out = turn
                .and_then(|t| now.checked_duration_since(t.last_output_at))
                .map(|d| d.as_secs());
            (
                elapsed
                    .map(format_duration_compact)
                    .unwrap_or_else(|| "--".into()),
                hb.map(|s| format!("{s}s")).unwrap_or_else(|| "--".into()),
                out.map(|s| format!("{s}s")).unwrap_or_else(|| "--".into()),
            )
        };
        rows.push(ThreadRow {
            text: pad_to_width(
                &format!(
                    "{} {} {} {}",
                    fit_left(&badge, agent_w),
                    fit_right(&elapsed_s, elap_w),
                    fit_right(&hb_s, hb_w),
                    fit_right(&out_s, out_w),
                ),
                width,
            ),
            kind: ThreadRowKind::StatusRow,
        });
        rows.push(ThreadRow {
            text: pad_to_width(
                &format!(
                    "{} {} {} {}",
                    fit_left(&format!("\u{21b3} {stage}"), agent_w),
                    fit_right("", elap_w),
                    fit_right("", hb_w),
                    fit_right("", out_w),
                ),
                width,
            ),
            kind: ThreadRowKind::StatusSubRow,
        });
    }
    rows
}

/// Sort key for the breather agent table: smaller = higher priority. Used
/// to promote actively-running agents to the front so the truncated table
/// always shows the live work. Mirrors `agent_ops_view::roster_running_priority`
/// but operates on agent ids since the breather works in id-space.
fn breather_running_priority(state: &AppState, agent_id: &str) -> u8 {
    if state.agents.active_turns.contains_key(agent_id) {
        return 0;
    }
    let queued = state
        .agents
        .queued_codex_turns
        .iter()
        .any(|turn| turn.agent_id == agent_id)
        || state
            .agents
            .queued_claude_turns
            .iter()
            .any(|turn| turn.agent_id == agent_id);
    if queued {
        return 1;
    }
    let agent = state.agents.agents_get(agent_id);
    match agent.map(|a| a.status) {
        Some(AgentStatus::Running) | Some(AgentStatus::Waiting) => 2,
        Some(AgentStatus::Idle) => 3,
        Some(AgentStatus::Error) | None => 4,
    }
}

pub(super) fn breather_rows_for_user_prompt(
    state: &AppState,
    swarm: Option<&SwarmRuntime>,
    pulse_on: bool,
    width: usize,
    pane_isolate: bool,
) -> Vec<ThreadRow> {
    let mission_ctx = state.agents.selected_context_mission();
    let agent_ctx = state.agents.selected_context_agent();
    let mut primary_ids = Vec::new();
    let mut secondary_ids = Vec::new();
    for agent in state.agents.agents.iter() {
        let has_active = state.agents.active_turns.contains_key(&agent.id);
        let has_queued = state
            .agents
            .queued_codex_turns
            .iter()
            .any(|turn| turn.agent_id == agent.id)
            || state
                .agents
                .queued_claude_turns
                .iter()
                .any(|turn| turn.agent_id == agent.id);
        if !has_active && !has_queued {
            continue;
        }
        let queued_in_mission = mission_ctx.is_some_and(|mission_id| {
            state.agents.queued_codex_turns.iter().any(|turn| {
                turn.agent_id == agent.id && turn.mission_id.as_deref() == Some(mission_id)
            }) || state.agents.queued_claude_turns.iter().any(|turn| {
                turn.agent_id == agent.id && turn.mission_id.as_deref() == Some(mission_id)
            })
        });
        let in_context = if let Some(mission_id) = mission_ctx {
            agent.current_mission.as_deref() == Some(mission_id) || queued_in_mission
        } else if let Some(selected_agent) = agent_ctx {
            agent.id == selected_agent || chat_clone_base_id(&agent.id) == Some(selected_agent)
        } else {
            true
        };
        if in_context {
            primary_ids.push(agent.id.clone());
        } else {
            secondary_ids.push(agent.id.clone());
        }
    }

    let width = width.max(1);
    let indent_str = String::new();
    let inner = width;

    let now = Instant::now();
    let mut swarm_assigned_ids: Vec<String> = Vec::new();
    let mut swarm_mission_id: Option<&str> = None;
    if let Some(mission_id) = mission_ctx {
        if let Some(mission) = state.agents.missions.iter().find(|m| m.id == mission_id) {
            if mission.swarm {
                swarm_mission_id = Some(mission_id);
                for id in mission.assigned_agents.iter() {
                    if swarm_assigned_ids.iter().any(|existing| existing == id) {
                        continue;
                    }
                    swarm_assigned_ids.push(id.clone());
                }
            }
        }
    }

    let mut ordered_ids = Vec::new();
    ordered_ids.extend(swarm_assigned_ids.iter().cloned());
    let secondary_iter: &mut dyn Iterator<Item = &String> = if pane_isolate {
        &mut std::iter::empty()
    } else {
        &mut secondary_ids.iter()
    };
    for id in primary_ids.iter().chain(secondary_iter) {
        if ordered_ids.iter().any(|existing: &String| existing == id) {
            continue;
        }
        ordered_ids.push(id.clone());
    }
    if ordered_ids.is_empty() {
        return Vec::new();
    }

    // Shadow agents are hidden from the roster but still count for
    // activity/queue detection — otherwise the breather would show "Waiting"
    // while a shadow (propose/judge/review) is actively working. Scope to
    // the selected agent context so a shadow on agent A doesn't trigger
    // agent B's breather.
    let shadow_ids: Vec<&str> = state
        .agents
        .agents
        .iter()
        .filter(|lane| lane.shadow)
        .filter(|lane| match agent_ctx {
            Some(ctx) => crate::shadow::parse_shadow_lane_id(&lane.id)
                .map(|(base, _, _)| base == ctx)
                .unwrap_or(false),
            None => true,
        })
        .map(|lane| lane.id.as_str())
        .collect();

    let any_active = ordered_ids
        .iter()
        .any(|id| state.agents.active_turns.contains_key(id.as_str()))
        || shadow_ids
            .iter()
            .any(|id| state.agents.active_turns.contains_key(*id));
    let any_queued = ordered_ids.iter().any(|id| {
        state
            .agents
            .queued_codex_turns
            .iter()
            .any(|turn| turn.agent_id == id.as_str())
            || state
                .agents
                .queued_claude_turns
                .iter()
                .any(|turn| turn.agent_id == id.as_str())
    }) || shadow_ids.iter().any(|id| {
        state
            .agents
            .queued_codex_turns
            .iter()
            .any(|turn| turn.agent_id == *id)
            || state
                .agents
                .queued_claude_turns
                .iter()
                .any(|turn| turn.agent_id == *id)
    });
    // Source of truth (production): the swarm runtime moves a run into
    // `completed_runs` once the planner's synthesis step lands. Per-agent
    // message scans miss clones whose tasks were skipped or never
    // dispatched, leaving the UI stuck at "Waiting ..." even though the
    // swarm was done. Fall back to the message scan only when no swarm
    // runtime is provided (test-only path).
    let all_swarm_done = swarm_mission_id.is_some_and(|mid| match swarm {
        Some(s) => s.mission_is_complete(mid),
        None => {
            !swarm_assigned_ids.is_empty()
                && swarm_assigned_ids.iter().all(|id| {
                    state.agents.messages.iter().any(|msg| {
                        msg.mission_id.as_deref() == Some(mid)
                            && msg.agent_id.as_deref() == Some(id.as_str())
                    })
                })
        }
    });
    // Distinguish a normal completion ("Done") from an operator abort
    // ("Aborted") so the breather doesn't lie about the run's outcome.
    let swarm_was_aborted =
        swarm_mission_id.is_some_and(|mid| swarm.is_some_and(|s| s.mission_was_aborted(mid)));
    // Whether the swarm run has been moved to `completed_runs`.
    let swarm_mission_complete =
        swarm_mission_id.is_some_and(|mid| swarm.is_some_and(|s| s.mission_is_complete(mid)));
    let working = any_active || any_queued;
    let swarm_phase = swarm_mission_id.and_then(|mid| swarm.and_then(|s| s.swarm_stage_label(mid)));
    let swarm_hint = swarm_mission_id.and_then(|mid| swarm.and_then(|s| s.swarm_stage_hint(mid)));
    let is_swarm = swarm_mission_id.is_some();
    let shadow_stage = crate::shadow::shadow_stage_label_from_state(state, agent_ctx);
    // Aborted takes priority over every other stage label. After
    // `/abort`, runner CancelTurn commands take ~50 ms to kill the
    // subprocesses and emit TurnFailed; during that window the
    // active_turns / queued_*_turns vectors aren't fully drained, so
    // `any_active || any_queued` would otherwise mask the abort.
    let base_label: std::borrow::Cow<'_, str> = if is_swarm && swarm_was_aborted {
        "Aborted".into()
    } else if !is_swarm && shadow_stage.is_some() {
        format!("{} ...", shadow_stage.unwrap()).into()
    } else if any_active || any_queued {
        match swarm_phase {
            Some("PLAN") => "Planning ...".into(),
            Some("VERIFY") => "Verifying ...".into(),
            Some("SYNTH") => "Synthesizing ...".into(),
            Some("EXEC") => swarm_exec_label(state, &ordered_ids, swarm).into(),
            _ if is_swarm && any_active => swarm_exec_label(state, &ordered_ids, swarm).into(),
            _ if any_active => "Working ...".into(),
            _ => "Queued ...".into(),
        }
    } else if is_swarm && swarm_hint.is_some() {
        match swarm_phase {
            Some("VERIFY") => "Verifying ...".into(),
            Some("SYNTH") => "Synthesizing ...".into(),
            _ => "Waiting ...".into(),
        }
    } else if is_swarm && all_swarm_done {
        "Done".into()
    } else if is_swarm {
        "Waiting ...".into()
    } else {
        "Working ...".into()
    };
    let label: std::borrow::Cow<'_, str> = match swarm_hint {
        Some(hint) if base_label.ends_with("...") => {
            let trimmed = base_label.trim_end_matches("...").trim_end();
            format!("{trimmed} ({hint}) ...").into()
        }
        _ => base_label,
    };

    // Cap the breather agent table to a manageable height. With swarms of
    // up to `MAX_SWARM_SIZE` clones, this view would otherwise dominate the
    // chat pane. `NIT_ROSTER_NO_TRUNCATE=1` opts out of the cap.
    const BREATHER_VISIBLE_AGENTS: usize = 6;
    ordered_ids.sort_by_key(|id| breather_running_priority(state, id.as_str()));
    let cap = if super::super::agent_ops_view::roster_truncation_disabled() {
        ordered_ids.len()
    } else {
        BREATHER_VISIBLE_AGENTS
    };
    let visible_count = ordered_ids.len().min(cap);
    let hidden_agent_count = ordered_ids.len() - visible_count;
    let visible_ids = &ordered_ids[..visible_count];

    let seed_id = primary_ids
        .first()
        .or_else(|| secondary_ids.first())
        .or_else(|| ordered_ids.first())
        .map(String::as_str);
    let animating = !matches!(label.as_ref(), "Done" | "Aborted");
    let ecg = ecg_indicator(state.metrics.frame_count, seed_id, pulse_on, animating);

    let mut rows = Vec::new();
    rows.push(ThreadRow {
        text: format!("{ecg} {label}"),
        kind: ThreadRowKind::Breather,
    });

    let elap_w = 6usize;
    let hb_w = 4usize;
    let out_w = 4usize;
    let times_and_spacing = elap_w + hb_w + out_w + 3;
    let agent_w = inner.saturating_sub(times_and_spacing + 1).max(1);

    if agent_w < 6 {
        for id in visible_ids.iter() {
            let agent = state
                .agents
                .agents
                .iter()
                .find(|agent| agent.id == id.as_str());
            let badge = agent
                .map(agent_roster_label)
                .unwrap_or_else(|| id.to_string());
            let turn = state.agents.active_turns.get(id.as_str());
            let queued_for_swarm = swarm_mission_id.is_some_and(|mid| {
                state.agents.queued_codex_turns.iter().any(|turn| {
                    turn.agent_id == id.as_str() && turn.mission_id.as_deref() == Some(mid)
                }) || state.agents.queued_claude_turns.iter().any(|turn| {
                    turn.agent_id == id.as_str() && turn.mission_id.as_deref() == Some(mid)
                })
            });
            let queued_any = state
                .agents
                .queued_codex_turns
                .iter()
                .any(|turn| turn.agent_id == id.as_str())
                || state
                    .agents
                    .queued_claude_turns
                    .iter()
                    .any(|turn| turn.agent_id == id.as_str());
            let has_message = swarm_mission_id.is_some_and(|mid| {
                state.agents.messages.iter().any(|msg| {
                    msg.mission_id.as_deref() == Some(mid)
                        && msg.agent_id.as_deref() == Some(id.as_str())
                })
            });
            let stage_raw = if let Some(turn) = turn {
                turn.stage.as_deref().unwrap_or("starting")
            } else if matches!(agent.map(|agent| agent.status), Some(AgentStatus::Error)) {
                "error"
            } else if queued_for_swarm {
                "swarm_queued"
            } else if queued_any {
                "queued"
            } else if swarm_assigned_ids.iter().any(|assigned| assigned == id) {
                if has_message {
                    "swarm_done"
                } else if swarm_mission_complete {
                    "swarm_skipped"
                } else {
                    "swarm_pending"
                }
            } else {
                "pending"
            };
            let suppress_times = matches!(stage_raw, "queued" | "swarm_queued");
            let stage = agent
                .map(|agent| format_agent_stage_label(state, agent, stage_raw))
                .unwrap_or_else(|| stage_raw.to_string());

            let (elapsed_s, hb_s, out_s) = if suppress_times {
                ("--".into(), "--".into(), "--".into())
            } else {
                let elapsed = turn.and_then(|turn| now.checked_duration_since(turn.started_at));
                let hb_age = turn
                    .and_then(|turn| now.checked_duration_since(turn.last_heartbeat_at))
                    .map(|d| d.as_secs());
                let out_age = turn
                    .and_then(|turn| now.checked_duration_since(turn.last_output_at))
                    .map(|d| d.as_secs());

                let elapsed_s = elapsed
                    .map(format_duration_compact)
                    .unwrap_or_else(|| "--".into());
                let hb_s = hb_age
                    .map(|s| format!("{s}s"))
                    .unwrap_or_else(|| "--".into());
                let out_s = out_age
                    .map(|s| format!("{s}s"))
                    .unwrap_or_else(|| "--".into());

                (elapsed_s, hb_s, out_s)
            };

            rows.push(ThreadRow {
                text: pad_to_width(
                    &format!("{indent_str}{badge} {elapsed_s} {hb_s} {out_s}"),
                    width,
                ),
                kind: ThreadRowKind::StatusRow,
            });
            rows.push(ThreadRow {
                text: pad_to_width(&format!("{indent_str}\u{21b3} {stage}"), width),
                kind: ThreadRowKind::StatusSubRow,
            });
        }
        if hidden_agent_count > 0 {
            rows.push(ThreadRow {
                text: pad_to_width(
                    &format!("{indent_str}\u{21b3} (+{hidden_agent_count} more)"),
                    width,
                ),
                kind: ThreadRowKind::StatusSubRow,
            });
        }
        if let Some(mission_id) = swarm_mission_id {
            append_swarm_meta_footer_rows(
                &mut rows,
                state,
                mission_id,
                &indent_str,
                width,
                inner,
                working,
            );
        }
        return rows;
    }

    rows.push(ThreadRow {
        text: pad_to_width(
            &format!(
                "{indent_str}{} {} {} {}",
                fit_left("AGENT", agent_w),
                fit_right("ELAP", elap_w),
                fit_right("HB", hb_w),
                fit_right("OUT", out_w),
            ),
            width,
        ),
        kind: ThreadRowKind::StatusHeader,
    });
    for id in visible_ids.iter() {
        let agent = state
            .agents
            .agents
            .iter()
            .find(|agent| agent.id == id.as_str());
        let badge = agent
            .map(agent_roster_label)
            .unwrap_or_else(|| id.to_string());
        let turn = state.agents.active_turns.get(id.as_str());
        let queued_for_swarm =
            swarm_mission_id.is_some_and(|mid| {
                state.agents.queued_codex_turns.iter().any(|turn| {
                    turn.agent_id == id.as_str() && turn.mission_id.as_deref() == Some(mid)
                }) || state.agents.queued_claude_turns.iter().any(|turn| {
                    turn.agent_id == id.as_str() && turn.mission_id.as_deref() == Some(mid)
                })
            });
        let queued_any = state
            .agents
            .queued_codex_turns
            .iter()
            .any(|turn| turn.agent_id == id.as_str())
            || state
                .agents
                .queued_claude_turns
                .iter()
                .any(|turn| turn.agent_id == id.as_str());
        let has_message = swarm_mission_id.is_some_and(|mid| {
            state.agents.messages.iter().any(|msg| {
                msg.mission_id.as_deref() == Some(mid)
                    && msg.agent_id.as_deref() == Some(id.as_str())
            })
        });
        let stage_raw = if let Some(turn) = turn {
            turn.stage.as_deref().unwrap_or("starting")
        } else if matches!(agent.map(|agent| agent.status), Some(AgentStatus::Error)) {
            "error"
        } else if queued_for_swarm {
            "swarm_queued"
        } else if queued_any {
            "queued"
        } else if swarm_assigned_ids.iter().any(|assigned| assigned == id) {
            if has_message {
                "swarm_done"
            } else if swarm_mission_complete {
                "swarm_skipped"
            } else {
                "swarm_pending"
            }
        } else {
            "pending"
        };
        let suppress_times = matches!(stage_raw, "queued" | "swarm_queued");
        let stage = agent
            .map(|agent| format_agent_stage_label(state, agent, stage_raw))
            .unwrap_or_else(|| stage_raw.to_string());

        let (elapsed_s, hb_s, out_s) = if suppress_times {
            ("--".into(), "--".into(), "--".into())
        } else {
            let elapsed = turn.and_then(|turn| now.checked_duration_since(turn.started_at));
            let hb_age = turn
                .and_then(|turn| now.checked_duration_since(turn.last_heartbeat_at))
                .map(|d| d.as_secs());
            let out_age = turn
                .and_then(|turn| now.checked_duration_since(turn.last_output_at))
                .map(|d| d.as_secs());

            let elapsed_s = elapsed
                .map(format_duration_compact)
                .unwrap_or_else(|| "--".into());
            let hb_s = hb_age
                .map(|s| format!("{s}s"))
                .unwrap_or_else(|| "--".into());
            let out_s = out_age
                .map(|s| format!("{s}s"))
                .unwrap_or_else(|| "--".into());

            (elapsed_s, hb_s, out_s)
        };
        rows.push(ThreadRow {
            text: pad_to_width(
                &format!(
                    "{indent_str}{} {} {} {}",
                    fit_left(&badge, agent_w),
                    fit_right(&elapsed_s, elap_w),
                    fit_right(&hb_s, hb_w),
                    fit_right(&out_s, out_w),
                ),
                width,
            ),
            kind: ThreadRowKind::StatusRow,
        });
        rows.push(ThreadRow {
            text: pad_to_width(
                &format!(
                    "{indent_str}{} {} {} {}",
                    fit_left(&format!("\u{21b3} {stage}"), agent_w),
                    fit_right("", elap_w),
                    fit_right("", hb_w),
                    fit_right("", out_w),
                ),
                width,
            ),
            kind: ThreadRowKind::StatusSubRow,
        });
    }
    if hidden_agent_count > 0 {
        rows.push(ThreadRow {
            text: pad_to_width(
                &format!(
                    "{indent_str}{} {} {} {}",
                    fit_left(&format!("\u{21b3} (+{hidden_agent_count} more)"), agent_w),
                    fit_right("", elap_w),
                    fit_right("", hb_w),
                    fit_right("", out_w),
                ),
                width,
            ),
            kind: ThreadRowKind::StatusRow,
        });
    }

    if let Some(mission_id) = swarm_mission_id {
        append_swarm_meta_footer_rows(
            &mut rows,
            state,
            mission_id,
            &indent_str,
            width,
            inner,
            working,
        );
    }

    rows
}
