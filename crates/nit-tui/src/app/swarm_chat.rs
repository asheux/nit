use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use nit_core::{
    followups::Source, AgentConsoleTab, AgentMessage, AppState, Mode, PaneId, SwarmDraftTarget,
};

use super::{chat_history_remember, chat_input::reset_chat_input_and_history_nav, timestamp_label};
use crate::swarm::{push_system_alert_to_mission, SwarmRuntime};

pub(crate) fn prepare_swarm_followup_draft(
    state: &mut AppState,
    mission_id: &str,
    task_id: Option<&str>,
    source: Option<Source>,
) -> Result<(), String> {
    let target = SwarmDraftTarget {
        mission_id: mission_id.to_owned(),
        suggested_task: source.as_ref().and(task_id).map(str::to_owned),
        task_id: task_id.filter(|_| source.is_none()).map(str::to_owned),
        source,
    };
    if !state.agents.chat_input.is_empty()
        && state.agents.swarm_draft_target.as_ref().is_some_and(|old| {
            old.mission_id != target.mission_id
                || (old.task_id.is_some()
                    && target.task_id.is_some()
                    && old.task_id != target.task_id)
                || (old.source.is_some() && old.source != target.source)
        })
    {
        return Err(
            "Your draft already targets another task or change. Send or clear it first.".into(),
        );
    }
    let mut target = target;
    if target.task_id.is_none() && !state.agents.chat_input.is_empty() {
        target.task_id = state
            .agents
            .swarm_draft_target
            .as_ref()
            .and_then(|old| old.task_id.clone());
    }
    state.agents.swarm_draft_target = Some(target);
    state.terminal_pane_active = false;
    state.agents.console_tab = AgentConsoleTab::Thread;
    state.focus = PaneId::Notes;
    state.mode = Mode::Normal;
    Ok(())
}

fn follows_selection(raw: &str) -> bool {
    !matches!(
        raw.split_whitespace().next(),
        Some(
            "@all"
                | "@new"
                | "@queue"
                | "@q"
                | "@swarm"
                | "@shadow"
                | "@multiway"
                | "@abort"
                | "/abort"
        )
    )
}

pub(crate) fn followup_mission<'a>(state: &'a AppState, swarm: &SwarmRuntime) -> Option<&'a str> {
    if let Some(target) = &state.agents.swarm_draft_target {
        return Some(&target.mission_id);
    }
    state
        .agents
        .selected_context_mission()
        .filter(|mid| swarm.is_active_mission(mid))
}

pub(crate) fn pin_composing_mission(state: &mut AppState, swarm: &SwarmRuntime) {
    if state.agents.chat_input.is_empty() {
        state.agents.swarm_draft_target = None;
    } else if state.agents.swarm_draft_target.is_none()
        && follows_selection(&state.agents.chat_input)
    {
        if let Some(mid) = followup_mission(state, swarm) {
            state.agents.swarm_draft_target = Some(SwarmDraftTarget {
                mission_id: mid.to_owned(),
                suggested_task: None,
                task_id: None,
                source: None,
            });
        }
    }
}

pub(crate) fn cycle_followup_target(
    state: &mut AppState,
    swarm: &SwarmRuntime,
    key: &KeyEvent,
) -> bool {
    if key.modifiers != KeyModifiers::ALT || !matches!(key.code, KeyCode::Left | KeyCode::Right) {
        return false;
    }
    let Some(mid) = followup_mission(state, swarm).map(str::to_owned) else {
        return false;
    };
    let ids = swarm.operator_followup_tasks(&mid);
    if ids.is_empty() {
        state.status = Some("No live swarm tasks to select. Draft kept.".into());
        return true;
    }
    let target = state
        .agents
        .swarm_draft_target
        .get_or_insert_with(|| SwarmDraftTarget {
            mission_id: mid,
            suggested_task: None,
            task_id: None,
            source: None,
        });
    let current = target
        .task_id
        .as_ref()
        .and_then(|id| ids.iter().position(|item| item == id));
    let idx = match key.code {
        KeyCode::Left => current.unwrap_or(0).checked_sub(1).unwrap_or(ids.len() - 1),
        _ => current.map_or(0, |idx| (idx + 1) % ids.len()),
    };
    target.task_id = Some(ids[idx].clone());
    state.status = Some(format!(
        "To task {} · {} · Enter sends",
        ids[idx], target.mission_id
    ));
    true
}

pub(crate) fn submit_active_followup(
    state: &mut AppState,
    swarm: &mut SwarmRuntime,
    raw: &str,
) -> Option<bool> {
    let command = raw.trim_start().split_whitespace().next()?;
    let addressed = matches!(command, "@task" | "@agent");
    if !follows_selection(raw) {
        return None;
    }
    let Some(mission) = followup_mission(state, swarm).map(str::to_owned) else {
        if addressed {
            state.status = Some("Select an active swarm mission first. Draft kept.".into());
            return Some(false);
        }
        return None;
    };
    let pinned = state.agents.swarm_draft_target.clone();
    let mut target = pinned.as_ref().and_then(|p| p.task_id.as_deref());
    let mut body = raw.trim();
    if addressed {
        let tail = body[command.len()..].trim_start();
        let Some((id, msg)) = tail
            .split_once(char::is_whitespace)
            .filter(|(_, msg)| !msg.trim().is_empty())
        else {
            state.status = Some(format!("Use {command} ID MESSAGE. Draft kept."));
            return Some(false);
        };
        target = Some(id);
        body = msg.trim_start();
    }
    let source = pinned.as_ref().and_then(|p| p.source.as_ref());
    if target.is_none() {
        if let Some((source, expected)) =
            source.zip(pinned.as_ref().and_then(|p| p.suggested_task.as_deref()))
        {
            let owner =
                swarm.operator_followup_task_for_path(state, &mission, source.path.as_ref());
            if owner.as_deref() != Ok(expected) {
                state.status =
                    Some("File owner changed. Alt+←/→ to choose a task. Draft kept.".into());
                return Some(false);
            }
        }
    }
    match swarm.queue_operator_followup(state, &mission, target, body, source) {
        Ok(receipt) => {
            let at = timestamp_label(state);
            state.agents.messages.push(AgentMessage {
                at,
                channel: nit_core::AgentChannel::Agent,
                agent_id: None,
                mission_id: Some(mission.clone()),
                text: body.to_owned(),
                kind: None,
                prompt_msg_idx: None,
            });
            push_system_alert_to_mission(state, &mission, receipt.clone());
            state.status = Some(receipt);
            chat_history_remember(state, raw);
            reset_chat_input_and_history_nav(state);
            state.agents.console_scroll = nit_core::CONSOLE_SCROLL_BOTTOM;
            Some(true)
        }
        Err(error) => {
            let msg = format!("Draft kept. {error}");
            if !state.agents.messages.last().is_some_and(|last| {
                last.mission_id.as_deref() == Some(&mission) && last.text == msg
            }) {
                push_system_alert_to_mission(state, &mission, msg.clone());
            }
            state.status = Some(msg);
            pin_composing_mission(state, swarm);
            Some(false)
        }
    }
}
