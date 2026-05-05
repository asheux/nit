//! Multipane session persistence.
//!
//! On Ctrl+Q (or any clean shutdown after a pane has run a mission), the
//! current `MultipaneState` is serialized to
//! `<state_dir>/multipane/session-<workspace-hash>.json`. On the next
//! launch, `multipane::setup::install_filtered` calls [`load_session`] and
//! prefers any saved per-pane `cwd` / `chat_input` / `chat_prompt_history`
//! / `selected_agent_id` over the defaults.
//!
//! Serialization is best-effort: missing state dir, IO errors, and
//! mismatched schemas all degrade to a fresh layout with a `tracing::warn!`.
//! UI-only fields (`help_open`, ephemeral selection / dir-search /
//! roster latches) are `#[serde(skip)]` on `MultipaneState` /
//! `PaneSession` so the round-trip is lossy on purpose.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use nit_core::{MultipaneState, PaneSession};

const FILE_PREFIX: &str = "session-";
const FILE_SUFFIX: &str = ".json";
const CHAT_INPUT_CAP_BYTES: usize = 4 * 1024;

/// Resolve the on-disk path used to persist multipane state for the
/// given workspace. `None` when the host has no usable state directory
/// (`directories` returns nothing — exotic platforms only).
pub fn session_path(workspace: &Path) -> Option<PathBuf> {
    let state = nit_utils::paths::state_dir()?;
    let hash = workspace_hash(workspace);
    Some(
        state
            .join("multipane")
            .join(format!("{FILE_PREFIX}{hash:016x}{FILE_SUFFIX}")),
    )
}

fn workspace_hash(workspace: &Path) -> u64 {
    nit_utils::hashing::stable_hash_bytes(workspace.to_string_lossy().as_bytes())
}

fn cap_chat_input(pane: &mut PaneSession) {
    if pane.chat_input.len() <= CHAT_INPUT_CAP_BYTES {
        return;
    }
    pane.chat_input.truncate(CHAT_INPUT_CAP_BYTES);
    pane.chat_input_cursor = pane.chat_input_cursor.min(pane.chat_input.chars().count());
}

/// Persist `state` to the workspace's session file. Caps `chat_input`
/// per pane at 4 KB to avoid runaway disk use after a paste of binary
/// content.
pub fn save_session(state: &MultipaneState, workspace: &Path) -> io::Result<()> {
    let Some(path) = session_path(workspace) else {
        return Ok(());
    };
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut snapshot = state.clone();
    for pane in &mut snapshot.panes {
        cap_chat_input(pane);
    }
    let json = serde_json::to_string_pretty(&snapshot)
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;
    // Write-then-rename so a crash mid-write can't corrupt the live
    // session file. A leftover `.tmp` is harmless; load_session only
    // reads the canonical path.
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, json)?;
    fs::rename(&tmp, &path)?;
    Ok(())
}

/// Load a previously persisted session. Returns `None` when the file is
/// missing, corrupt, or the workspace lacks a state dir. A corrupt file
/// is logged via `tracing::warn!` and treated like "no prior session".
pub fn load_session(workspace: &Path) -> Option<MultipaneState> {
    let path = session_path(workspace)?;
    let raw = match fs::read_to_string(&path) {
        Ok(s) => s,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return None,
        Err(err) => {
            tracing::warn!(?path, %err, "multipane session load failed");
            return None;
        }
    };
    match serde_json::from_str::<MultipaneState>(&raw) {
        Ok(state) => Some(state),
        Err(err) => {
            tracing::warn!(?path, %err, "multipane session parse failed; treating as fresh");
            None
        }
    }
}

/// Remove the workspace's session file. Used on Ctrl+Q from a "fresh"
/// session — i.e. no pane has run a mission yet, so persisting an empty
/// layout would just clutter the state dir.
pub fn drop_session(workspace: &Path) {
    let Some(path) = session_path(workspace) else {
        return;
    };
    let Err(err) = fs::remove_file(&path) else {
        return;
    };
    if err.kind() != io::ErrorKind::NotFound {
        tracing::warn!(?path, %err, "multipane session drop failed");
    }
}

/// True when no pane in the snapshot has yet successfully dispatched a
/// prompt. The Ctrl+Q handler uses this to decide whether to overwrite
/// the on-disk session or leave it alone.
pub fn is_fresh(state: &MultipaneState) -> bool {
    state.panes.iter().all(|p| !p.has_run_mission)
}

/// Merge a previously-loaded session onto a freshly-installed
/// `MultipaneState`. The freshly-installed state has the canonical
/// pane count + grid shape + backend filter; this helper lifts only
/// the per-pane "operator-typed" fields so any roster drift between
/// runs (lane added / removed / renamed) doesn't crash the loader.
///
/// Returns `false` when the prior pane count differs from `target` —
/// callers can decide whether to warn or drop the prior file.
pub fn merge_prior(target: &mut MultipaneState, prior: MultipaneState) -> bool {
    if target.panes.len() != prior.panes.len() {
        return false;
    }
    target.focused = prior.focused.min(target.panes.len().saturating_sub(1));
    for (slot, prior) in target.panes.iter_mut().zip(prior.panes.into_iter()) {
        slot.cwd = prior.cwd;
        slot.chat_input = prior.chat_input;
        slot.chat_input_cursor = prior.chat_input_cursor;
        slot.chat_prompt_history = prior.chat_prompt_history;
        slot.swarm_template = prior.swarm_template;
        slot.swarm_mission = prior.swarm_mission;
        slot.has_run_mission = prior.has_run_mission;
        // `selected_agent_id` only carries forward when the agent still
        // exists; otherwise we drop back to roster mode. The caller is
        // expected to validate against the roster before re-rendering.
        slot.selected_agent_id = prior.selected_agent_id;
        // chat_mission_id is a pure function of pane_id; recompute so
        // schema drift in the format never leaves a stale value loaded
        // from disk.
        slot.chat_mission_id = super::agent_id::pane_chat_mission_id(slot.pane_id);
    }
    true
}

#[cfg(test)]
#[path = "../tests/multipane_persistence.rs"]
mod tests;
