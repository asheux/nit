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

use nit_core::MultipaneState;

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
        if pane.chat_input.len() > CHAT_INPUT_CAP_BYTES {
            pane.chat_input.truncate(CHAT_INPUT_CAP_BYTES);
            pane.chat_input_cursor = pane.chat_input_cursor.min(pane.chat_input.chars().count());
        }
    }
    let json = serde_json::to_string_pretty(&snapshot)
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;
    fs::write(&path, json)?;
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
    if let Err(err) = fs::remove_file(&path) {
        if err.kind() != io::ErrorKind::NotFound {
            tracing::warn!(?path, %err, "multipane session drop failed");
        }
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
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use nit_core::PaneSession;
    use std::path::PathBuf;

    fn fixture_state() -> MultipaneState {
        MultipaneState {
            backend_agent_id: "claude-haiku-4-5".into(),
            panes: vec![
                PaneSession {
                    pane_id: 0,
                    cwd: PathBuf::from("/p0"),
                    chat_input: "draft".into(),
                    chat_input_cursor: 5,
                    swarm_template: "lab".into(),
                    swarm_mission: "auto".into(),
                    has_run_mission: true,
                    ..PaneSession::default()
                },
                PaneSession {
                    pane_id: 1,
                    cwd: PathBuf::from("/p1"),
                    ..PaneSession::default()
                },
            ],
            focused: 1,
            grid_cols: 2,
            grid_rows: 1,
            backend_filter: Some("claude-haiku-4-5".into()),
            help_open: false,
        }
    }

    #[test]
    fn save_then_load_roundtrips_per_pane_cwd_and_chat_input() {
        // Roundtrip through serde_json without touching the shared
        // state dir (ProjectDirs is unmocked in this crate's tests).
        let state = fixture_state();
        let json = serde_json::to_string_pretty(&state).unwrap();
        let loaded: MultipaneState = serde_json::from_str(&json).unwrap();
        assert_eq!(loaded.panes.len(), 2);
        assert_eq!(loaded.panes[0].cwd, PathBuf::from("/p0"));
        assert_eq!(loaded.panes[0].chat_input, "draft");
        assert_eq!(loaded.panes[0].chat_input_cursor, 5);
        assert_eq!(loaded.focused, 1);
        assert_eq!(loaded.backend_filter.as_deref(), Some("claude-haiku-4-5"));
        // help_open is #[serde(skip)] so always defaults.
        assert!(!loaded.help_open);
    }

    #[test]
    fn merge_prior_lifts_per_pane_fields_when_panes_match() {
        let mut target = fixture_state();
        // Reset per-pane fields on the target to simulate a fresh install.
        for p in &mut target.panes {
            p.chat_input.clear();
            p.cwd = PathBuf::from("/fresh");
            p.has_run_mission = false;
        }
        let prior = fixture_state();
        let merged = merge_prior(&mut target, prior);
        assert!(merged);
        assert_eq!(target.panes[0].cwd, PathBuf::from("/p0"));
        assert_eq!(target.panes[0].chat_input, "draft");
        assert_eq!(target.focused, 1);
    }

    #[test]
    fn merge_prior_rejects_when_pane_count_changes() {
        let mut target = fixture_state();
        let mut prior = fixture_state();
        prior.panes.pop();
        let merged = merge_prior(&mut target, prior);
        assert!(!merged);
    }

    #[test]
    fn is_fresh_flips_after_first_dispatch() {
        let mut state = fixture_state();
        for p in &mut state.panes {
            p.has_run_mission = false;
        }
        assert!(is_fresh(&state));
        state.panes[0].has_run_mission = true;
        assert!(!is_fresh(&state));
    }

    #[test]
    fn workspace_hash_is_deterministic() {
        let p = PathBuf::from("/workspace/example");
        assert_eq!(workspace_hash(&p), workspace_hash(&p));
    }
}
