use nit_core::{AgentConsoleTab, AppState, Mode, PaneId};

pub(crate) fn attach(state: &mut AppState, id: &str) {
    if !super::focused(state) || state.terminal_popup.visible {
        return;
    }
    let reference =
        format!("Read NitSkill {id} with `nit skills {id}` or the `nit_skill` MCP tool.");
    let draft = &mut state.agents.chat_input;
    if !draft.lines().any(|line| line.trim() == reference) {
        // Leave the insertion point and selection in the operator's text. Even an
        // empty draft gets an editable first line before the reference.
        if !draft.ends_with('\n') {
            draft.push('\n');
        }
        draft.push_str(&reference);
    }
    state.terminal_pane_active = false;
    state.agents.console_tab = AgentConsoleTab::Thread;
    state.focus = PaneId::Notes;
    state.mode = Mode::Normal;
}

#[cfg(test)]
#[path = "../../../tests/skills_attach.rs"]
mod tests;
