pub(crate) mod drafts;
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
    let mut target = SwarmDraftTarget {
        mission_id: mission_id.to_owned(),
        agent_id: None,
        saved_draft: None,
        needs_choice: false,
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
    if !state.agents.chat_input.is_empty() {
        if let Some(old) = &state.agents.swarm_draft_target {
            target.task_id = target.task_id.or_else(|| old.task_id.clone());
            target.agent_id = old.agent_id.clone();
            target.saved_draft = old.saved_draft.clone();
            target.needs_choice = old.needs_choice;
        }
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
                agent_id: None,
                saved_draft: None,
                needs_choice: false,
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
    let recovering = state
        .agents
        .swarm_draft_target
        .as_ref()
        .is_some_and(|t| t.needs_choice || !swarm.is_active_mission(&t.mission_id));
    let mid = if recovering {
        state
            .agents
            .selected_context_mission()
            .filter(|mid| swarm.is_active_mission(mid))
    } else {
        followup_mission(state, swarm)
    };
    let Some(mid) = mid.map(str::to_owned) else {
        if recovering {
            state.status =
                Some("Select a live mission before choosing the task for this draft.".into());
        }
        return recovering;
    };
    let ids = swarm.operator_followup_tasks(&mid);
    if ids.is_empty() {
        state.status = Some("No live swarm tasks to select. Draft kept.".into());
        return true;
    }
    let target = state
        .agents
        .swarm_draft_target
        .get_or_insert(SwarmDraftTarget {
            mission_id: mid.clone(),
            agent_id: None,
            saved_draft: None,
            needs_choice: false,
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
    target.mission_id = mid;
    target.needs_choice = false;
    target.task_id = Some(ids[idx].clone());
    target.agent_id = None;
    // An explicit picker choice takes precedence over an old typed address.
    let draft = &mut state.agents.chat_input;
    let mut words = draft.split_whitespace();
    if matches!(words.next(), Some("@agent" | "@task")) {
        if let Some(old) = words.next() {
            let address_start = draft.find('@').expect("typed address");
            let old_start =
                draft[address_start..].find(char::is_whitespace).unwrap() + address_start;
            let old_start =
                old_start + (draft[old_start..].len() - draft[old_start..].trim_start().len());
            let old_end = old_start + old.len();
            let old_chars = draft[address_start..old_end].chars().count();
            let replacement = format!("@task {}", ids[idx]);
            draft.replace_range(address_start..old_end, &replacement);
            state.agents.chat_input_cursor = state
                .agents
                .chat_input_cursor
                .saturating_sub(old_chars)
                .saturating_add(replacement.chars().count())
                .min(draft.chars().count());
            state.agents.chat_input_selection_anchor = None;
        }
    }
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
    if let Some(result) = drafts::command(state, raw) {
        return Some(result);
    }
    let command = raw.split_whitespace().next()?;
    let addressed = matches!(command, "@task" | "@agent");
    if !follows_selection(raw) {
        return None;
    }
    let Some(mission) = followup_mission(state, swarm).map(str::to_owned) else {
        if addressed {
            let mid = state
                .agents
                .selected_context_mission()
                .unwrap_or_default()
                .to_owned();
            state.agents.swarm_draft_target = Some(SwarmDraftTarget {
                mission_id: mid.clone(),
                needs_choice: true,
                ..Default::default()
            });
            reject_followup(
                state,
                swarm,
                &mid,
                "Select a live mission and choose a task first.".into(),
            );
            return Some(false);
        }
        return None;
    };
    let pinned = state.agents.swarm_draft_target.clone();
    if pinned.as_ref().is_some_and(|t| t.needs_choice) {
        reject_followup(
            state,
            swarm,
            &mission,
            "Select a live mission and press Alt+→ to bind this restored draft.".into(),
        );
        return Some(false);
    }
    let mut target = pinned.as_ref().and_then(|p| p.task_id.as_deref());
    let mut agent = pinned.as_ref().and_then(|p| p.agent_id.as_deref());
    let mut body = raw.trim();
    if addressed {
        let tail = body[command.len()..].trim_start();
        let Some((id, msg)) = tail
            .split_once(char::is_whitespace)
            .filter(|(_, msg)| !msg.trim().is_empty())
        else {
            reject_followup(state, swarm, &mission, format!("Use {command} ID MESSAGE."));
            return Some(false);
        };
        if command == "@agent" {
            agent = Some(id);
            target = None;
        } else {
            target = Some(id);
            agent = None;
        }
        body = msg.trim_start();
    }
    if let Some(agent_id) = agent {
        match swarm.operator_followup_task_for_agent(&mission, agent_id) {
            Ok(id) => target = Some(id),
            Err(error) => {
                reject_followup(state, swarm, &mission, error);
                return Some(false);
            }
        }
    }
    let resolved_target = target.map(str::to_owned);
    let source = pinned.as_ref().and_then(|p| p.source.as_ref());
    if target.is_none() {
        if let Some((source, expected)) =
            source.zip(pinned.as_ref().and_then(|p| p.suggested_task.as_deref()))
        {
            let owner =
                swarm.operator_followup_task_for_path(state, &mission, source.path.as_ref());
            if owner.as_deref() != Ok(expected) {
                reject_followup(
                    state,
                    swarm,
                    &mission,
                    "File owner changed. Alt+←/→ to choose a task.".into(),
                );
                return Some(false);
            }
        }
    }
    match swarm.queue_operator_followup(state, &mission, resolved_target.as_deref(), body, source) {
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
            let cleanup = pinned
                .as_ref()
                .and_then(|t| t.saved_draft.as_ref())
                .and_then(|id| drafts::remove(state, id).err());
            state.status = Some(match cleanup {
                None => receipt,
                Some(err) => {
                    format!("{receipt}. Saved draft could not be removed: {err}. Already queued.")
                }
            });
            chat_history_remember(state, raw);
            reset_chat_input_and_history_nav(state);
            state.agents.console_scroll = nit_core::CONSOLE_SCROLL_BOTTOM;
            Some(true)
        }
        Err(error) => {
            reject_followup(state, swarm, &mission, error);
            Some(false)
        }
    }
}

fn reject_followup(state: &mut AppState, swarm: &SwarmRuntime, mission: &str, error: String) {
    pin_composing_mission(state, swarm);
    let msg = match drafts::save(state) {
        Ok(()) => format!("Draft kept. {error}"),
        Err(err) => format!("Draft only in memory (save failed: {err}). {error}"),
    };
    if !state
        .agents
        .messages
        .last()
        .is_some_and(|last| last.mission_id.as_deref() == Some(mission) && last.text == msg)
    {
        push_system_alert_to_mission(state, mission, msg.clone());
    }
    state.status = Some(msg);
}
