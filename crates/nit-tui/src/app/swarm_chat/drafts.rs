//! Recover rejected attempts locally. A file is neither a provider message nor a queue entry.
use super::*;
use std::{
    fs, io,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

#[derive(serde::Serialize, serde::Deserialize)]
struct Draft {
    text: String,
    cursor: usize,
    target: SwarmDraftTarget,
}

fn slot(state: &AppState) -> String {
    state
        .multipane
        .as_ref()
        .map_or_else(|| "station".into(), |mp| format!("pane-{}", mp.focused))
}

fn directory(workspace: &Path) -> PathBuf {
    workspace.join(".nit").join("followup-drafts")
}

fn paths(workspace: &Path, slot: &str) -> io::Result<Vec<(String, PathBuf)>> {
    let dir = directory(workspace);
    for base in [workspace.join(".nit"), dir.clone()] {
        match fs::symlink_metadata(&base) {
            Ok(meta) if meta.is_dir() => {}
            Ok(_) => return Err(io::Error::other("draft directory is not a directory")),
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(e),
        }
    }
    let mut files = Vec::new();
    let prefix = format!("{slot}-");
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        if !entry.file_type()?.is_file() {
            continue;
        }
        let path = entry.path();
        if path.extension().and_then(|v| v.to_str()) != Some("json") {
            continue;
        }
        if let Some(id) = path
            .file_stem()
            .and_then(|v| v.to_str())
            .and_then(|v| v.strip_prefix(&prefix))
            .filter(|id| valid_id(id))
        {
            files.push((id.to_owned(), path))
        }
    }
    files.sort_unstable();
    Ok(files)
}
fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 48 && id.bytes().all(|b| b.is_ascii_hexdigit() || b == b'-')
}
fn new_id() -> String {
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!(
        "{nanos:x}-{:x}-{:x}",
        std::process::id(),
        SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    )
}

pub(super) fn save(state: &mut AppState) -> io::Result<()> {
    let slot = slot(state);
    let dir =
        nit_core::followups::ensure_directory(&state.workspace_root, &[".nit", "followup-drafts"])?;
    let target = state
        .agents
        .swarm_draft_target
        .get_or_insert_with(Default::default);
    let id = target.saved_draft.get_or_insert_with(new_id).clone();
    if !valid_id(&id) {
        return Err(io::Error::other("invalid saved draft ID"));
    }
    let draft = Draft {
        text: state.agents.chat_input.clone(),
        cursor: state.agents.chat_input_cursor,
        target: target.clone(),
    };
    nit_utils::write_atomic(&dir.join(format!("{slot}-{id}.json")), |out| {
        serde_json::to_writer(out, &draft).map_err(io::Error::other)
    })
}

pub(super) fn remove(state: &AppState, id: &str) -> io::Result<()> {
    let files = paths(&state.workspace_root, &slot(state))?;
    if let Some((_, path)) = files.into_iter().find(|(candidate, _)| candidate == id) {
        fs::remove_file(path)?;
    }
    Ok(())
}

fn load(id: String, path: &Path) -> io::Result<Draft> {
    let mut draft: Draft = serde_json::from_slice(&fs::read(path)?).map_err(io::Error::other)?;
    draft.target.saved_draft = Some(id);
    draft.target.needs_choice = true;
    Ok(draft)
}

fn restore(state: &mut AppState, id: String, path: &Path) -> io::Result<()> {
    let draft = load(id, path)?;
    state.agents.chat_input = draft.text;
    state.agents.chat_input_cursor = draft.cursor.min(state.agents.chat_input.chars().count());
    state.agents.chat_input_selection_anchor = None;
    state.agents.swarm_draft_target = Some(draft.target);
    state.status =
        Some("Restored unsent follow-up. Choose a live mission and Alt+→ to select a task.".into());
    Ok(())
}

pub(crate) fn restore_on_start(state: &mut AppState) {
    let restore_slot = |state: &mut AppState| {
        let files = paths(&state.workspace_root, &slot(state))?;
        if files.len() == 1 && state.agents.chat_input.is_empty() {
            let (id, path) = files.into_iter().next().unwrap();
            restore(state, id, &path)?;
        } else if !files.is_empty() {
            state.status = Some("Unsent follow-ups saved. Type /drafts to list them.".into());
        }
        io::Result::Ok(())
    };
    let restore_one = |state: &mut AppState| {
        if let Err(err) = restore_slot(state) {
            state.status = Some(format!("Could not restore follow-up draft: {err}"));
        }
    };
    if let Some(mp) = &state.multipane {
        let (initial, count) = (mp.focused, mp.panes.len());
        for i in 0..count {
            state.multipane.as_mut().unwrap().focused = i;
            crate::multipane::dispatch::with_pane_aliased(state, i, |state| restore_one(state));
        }
        state.multipane.as_mut().unwrap().focused = initial;
    } else {
        restore_one(state)
    }
}

pub(super) fn command(state: &mut AppState, raw: &str) -> Option<bool> {
    let mut args = raw.split_whitespace();
    if args.next() != Some("/drafts") {
        return None;
    }
    let args: Vec<_> = args.collect();
    let result = (|| -> io::Result<()> {
        let files = paths(&state.workspace_root, &slot(state))?;
        match args.as_slice() {
            [] => {
                let text = if files.is_empty() {
                    "No saved follow-up drafts.".into()
                } else {
                    let mut lines =
                        vec!["Restore with /drafts ID. Remove with /drafts discard ID.".into()];
                    for (id, path) in files {
                        let draft = load(id.clone(), &path)?;
                        lines.push(format!(
                            "{id} · {} · {}",
                            draft.target.mission_id,
                            draft
                                .text
                                .lines()
                                .next()
                                .unwrap_or("")
                                .chars()
                                .take(100)
                                .collect::<String>()
                        ));
                    }
                    lines.join("\n")
                };
                let at = timestamp_label(state);
                let mid = state.agents.selected_context_mission().map(str::to_owned);
                let agent = state.agents.selected_context_agent().map(str::to_owned);
                state.agents.messages.push(AgentMessage {
                    at,
                    channel: nit_core::AgentChannel::Agent,
                    agent_id: agent,
                    mission_id: mid,
                    text,
                    kind: Some("nit-skills".into()),
                    prompt_msg_idx: None,
                });
                reset_chat_input_and_history_nav(state);
                state.agents.console_scroll = nit_core::CONSOLE_SCROLL_BOTTOM;
            }
            [id] | ["discard", id] => {
                let (id, path) = files
                    .into_iter()
                    .find(|(candidate, _)| candidate == id)
                    .ok_or_else(|| io::Error::other("No saved draft with that ID in this pane"))?;
                if args.len() == 1 {
                    restore(state, id, &path)?;
                } else {
                    fs::remove_file(path)?;
                    reset_chat_input_and_history_nav(state);
                    state.status = Some("Saved draft removed.".into());
                }
            }
            _ => {
                return Err(io::Error::other(
                    "Use /drafts, /drafts ID or /drafts discard ID",
                ))
            }
        }
        Ok(())
    })();
    if let Err(err) = result {
        state.status = Some(err.to_string());
        return Some(false);
    }
    Some(true)
}
