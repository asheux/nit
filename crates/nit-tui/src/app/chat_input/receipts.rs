use nit_core::{
    followups, AgentChannel, AgentMessage, AppState, SwarmDraftTarget, CONSOLE_SCROLL_BOTTOM,
};

use super::{
    chat_history_remember, reset_chat_input_and_history_nav, sync_cursor_to_input_end,
    timestamp_label,
};

pub(super) fn handle(state: &mut AppState, raw: &str) -> Option<bool> {
    let mut words = raw.split_whitespace();
    if words.next()? != "/followups" {
        return None;
    }
    let workspace = state
        .multipane
        .as_ref()
        .and_then(|mp| mp.panes.get(mp.focused))
        .map_or(&state.workspace_root, |p| &p.cwd);
    let mission = state.agents.selected_context_mission().map(str::to_owned);
    let agent = state.agents.selected_context_agent().map(str::to_owned);
    let args: Vec<_> = words.collect();
    let result = match args.as_slice() {
        [] => followups::list(workspace, Some(20)),
        ["recover", id] => {
            match followups::find(workspace, id) {
                Ok(note) => {
                    chat_history_remember(state, raw);
                    reset_chat_input_and_history_nav(state);
                    // Restoring a receipt reconstructs a draft only. A new receiver
                    // still passes through normal mission ownership checks on send.
                    if note.task_id.is_empty() {
                        state.agents.chat_input = format!("@agent {} {}", note.agent_id, note.text);
                        sync_cursor_to_input_end(state);
                        state.status = Some(format!("Restored unsent follow-up for {}. Review the recipient before sending.", note.agent_id));
                        return Some(true);
                    }
                    state.agents.chat_input = note.text;
                    state.agents.swarm_draft_target = Some(SwarmDraftTarget {
                        mission_id: note.mission_id.clone(),
                        needs_choice: true,
                        source: note.source,
                        ..Default::default()
                    });
                    sync_cursor_to_input_end(state);
                    state.status = Some(format!(
                    "Restored unsent draft from {} / {}. Choose a live mission and use Alt+→ to select its task.",
                    note.mission_id, note.task_id
                ));
                    return Some(true);
                }
                Err(error) => Err(error),
            }
        }
        [id] if *id != "recover" => {
            followups::find(workspace, id).map(|note| followups::describe(&note))
        }
        _ => Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "Use /followups, /followups ID, or /followups recover ID",
        )),
    };
    let text = match result {
        Ok(text) => text,
        Err(error) => {
            state.status = Some(error.to_string());
            return Some(false);
        }
    };
    chat_history_remember(state, raw);
    reset_chat_input_and_history_nav(state);
    state.agents.messages.push(AgentMessage {
        at: timestamp_label(state),
        channel: AgentChannel::Agent,
        agent_id: agent,
        mission_id: mission,
        text,
        prompt_msg_idx: None,
        turn: None,
        kind: Some("followup-history".into()),
    });
    state.agents.console_scroll = CONSOLE_SCROLL_BOTTOM;
    Some(true)
}
