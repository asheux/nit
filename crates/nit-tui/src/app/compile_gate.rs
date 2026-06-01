//! Compile-gate dispatcher + drain. Wires the
//! `CompileGateWorker` results into per-agent retry continuations
//! addressed at the file's writer, not the agent that triggered the
//! check.
//!
//! Trigger path: `event_drain::maybe_dispatch_compile_check` fires on
//! integrate-role `TurnCompleted` events when (a) the workspace is a
//! Cargo workspace and (b) the agent touched `crates/<name>/...`
//! files. Each touched crate is sent through the worker.
//!
//! Drain path: `drain_compile_results` runs every frame next to
//! `drain_genome_results`. For each error the worker returns, we look
//! up the writer of the cited file via the same helpers the genome
//! retry path uses (`genome_turn_modified` → substrate
//! `ExclusiveWrite` claim → triggering agent fallback). The writer
//! gets a continuation dispatched through `dispatch_agent_prompt`
//! describing the error and capped by `COMPILE_GATE_RETRY_LIMIT`.
//!
//! Best-effort: any failure in the gate (cargo binary missing,
//! `--no-compile-gate` env, attribution lookup empty) silently degrades
//! to no-op. The gate is a deterministic backstop, not a contractual
//! one — the test/review lanes remain the operator-facing verifier.

use std::path::Path;

use nit_core::{
    substrate::{ClaimKind, ClaimTarget},
    AgentBusEvent, AgentChannel, AgentDiagnosticEvent, AgentMessage, AgentStatus, AppState,
};

use super::*;
use crate::claude_runner::ClaudeRunner;
use crate::codex_runner::CodexRunner;
use crate::compile_gate_worker::{CompileCheckResult, CompileError, CompileGateWorker};
use crate::vitals::VitalsState;

/// Per-agent retry budget. Matches the genome retry shape (3). After
/// the budget exhausts the gate stops firing for that agent for the
/// current turn — further fixes go through the test/review lanes.
pub(super) const COMPILE_GATE_RETRY_LIMIT: u8 = 3;

const ENV_DISABLE: &str = "NIT_NO_COMPILE_GATE";

/// Fired from `event_drain::maybe_dispatch_genome_evals`'s sibling
/// path on integrate-role `TurnCompleted`. Spawns a `cargo check`
/// thread for every Rust crate the agent touched. No-op when the
/// gate is disabled (`NIT_NO_COMPILE_GATE`), the workspace isn't a
/// Cargo workspace, or the agent touched zero `crates/*/` files.
pub(super) fn maybe_dispatch_compile_check(
    state: &AppState,
    gate: &CompileGateWorker,
    agent_id: &str,
    mission_id: &Option<String>,
) {
    if std::env::var_os(ENV_DISABLE).is_some() {
        return;
    }
    let workspace = state.workspace_root.as_path();
    if !crate::swarm::is_cargo_workspace(workspace) {
        return;
    }
    let Some(paths) = state.genome_turn_modified.get(agent_id) else {
        return;
    };
    let crates = crates_for_paths(paths.iter().map(|p| p.as_path()), workspace);
    if crates.is_empty() {
        return;
    }
    let _ = gate.check(
        workspace.to_path_buf(),
        crates,
        agent_id.to_string(),
        mission_id.clone(),
    );
}

/// Drain pending `CompileCheckResult` events. For each error, route a
/// retry continuation at the writer of the cited file. Called once
/// per frame from `runner::run`.
pub(super) fn drain_compile_results(
    state: &mut AppState,
    gate: &CompileGateWorker,
    vitals: &mut VitalsState,
    codex: &CodexRunner,
    claude: &ClaudeRunner,
) {
    while let Ok(result) = gate.rx.try_recv() {
        apply_compile_result(state, vitals, codex, claude, result);
    }
}

fn apply_compile_result(
    state: &mut AppState,
    vitals: &mut VitalsState,
    codex: &CodexRunner,
    claude: &ClaudeRunner,
    result: CompileCheckResult,
) {
    if let Some(err) = result.spawn_error {
        state.agents.diag_events.push(AgentDiagnosticEvent {
            severity: nit_core::AgentAlertSeverity::Info,
            source: "compile-gate".into(),
            message: format!("[{}] gate skipped: {err}", result.triggering_agent_id),
            at: timestamp_label(state),
        });
        return;
    }
    if result.errors.is_empty() {
        // Clear the triggering agent's compile retry count — they just
        // proved a passing build. Genome retry budget is unaffected.
        state
            .compile_retry_counts
            .remove(&result.triggering_agent_id);
        return;
    }
    let mut errors_by_owner: std::collections::HashMap<String, Vec<CompileError>> =
        std::collections::HashMap::new();
    for err in result.errors.iter() {
        let owner = resolve_writer_for_path(state, &err.rel_path)
            .unwrap_or_else(|| result.triggering_agent_id.clone());
        errors_by_owner.entry(owner).or_default().push(err.clone());
    }
    for (agent_id, errors) in errors_by_owner {
        let count = state
            .compile_retry_counts
            .get(&agent_id)
            .copied()
            .unwrap_or(0);
        if count >= COMPILE_GATE_RETRY_LIMIT {
            push_budget_exhausted_message(state, &agent_id, &errors);
            continue;
        }
        let attempt = count.saturating_add(1);
        state.compile_retry_counts.insert(agent_id.clone(), attempt);
        let prompt = build_retry_prompt(attempt, COMPILE_GATE_RETRY_LIMIT, &errors);
        push_console_message(state, &agent_id, attempt, &errors);
        push_diag_event(state, &agent_id, attempt, &errors);
        dispatch_agent_prompt(
            state,
            vitals,
            Some(codex),
            Some(claude),
            agent_id,
            result.mission_id.clone(),
            prompt,
        );
    }
}

/// Walk `genome_turn_modified` (per-turn, freshest), then the
/// substrate's live `ExclusiveWrite` claims (covers writes from
/// earlier in the same mission whose per-turn entry was cleared by
/// the next `TurnStarted`). Returns `None` when neither knows the
/// path — caller falls back to the triggering agent.
fn resolve_writer_for_path(state: &AppState, rel_path: &str) -> Option<String> {
    let abs = state.workspace_root.join(rel_path);
    for (aid, paths) in state.genome_turn_modified.iter() {
        if paths.iter().any(|p| p == &abs) {
            return Some(aid.clone());
        }
    }
    state
        .substrate
        .claims
        .values()
        .filter(|c| {
            matches!(c.kind, ClaimKind::ExclusiveWrite)
                && matches!(
                    &c.target,
                    ClaimTarget::File { path } if path == &abs
                )
        })
        .max_by_key(|c| c.claimed_at_gen)
        .map(|c| c.claimed_by.clone())
}

/// Extract `crates/<name>` segments from the agent's touched paths.
/// Workspace-relative AND absolute paths under `workspace_root` are
/// both accepted. Returns a sorted, deduped list.
fn crates_for_paths<'a>(paths: impl Iterator<Item = &'a Path>, workspace: &Path) -> Vec<String> {
    use std::collections::BTreeSet;
    let mut set: BTreeSet<String> = BTreeSet::new();
    for p in paths {
        let rel = p.strip_prefix(workspace).unwrap_or(p);
        let Some(crate_rel) = rel.strip_prefix("crates/").ok().or_else(|| {
            rel.components().next().and_then(|c| {
                (c.as_os_str() == "crates").then(|| Path::new(rel).strip_prefix("crates").ok())?
            })
        }) else {
            // Path didn't start with crates/ — skip. Touches outside
            // the workspace tree (e.g. docs/) are not Rust code.
            let s = rel.to_string_lossy();
            let Some(after) = s.strip_prefix("crates/") else {
                continue;
            };
            let Some(name) = after.split('/').next() else {
                continue;
            };
            if !name.is_empty() {
                set.insert(name.to_string());
            }
            continue;
        };
        if let Some(name) = crate_rel.components().next() {
            let s = name.as_os_str().to_string_lossy().into_owned();
            if !s.is_empty() {
                set.insert(s);
            }
        }
    }
    set.into_iter().collect()
}

fn build_retry_prompt(attempt: u8, limit: u8, errors: &[CompileError]) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "[COMPILE GATE \u{2014} automatic retry {attempt}/{limit}]\n\n"
    ));
    out.push_str(
        "Your previous turn left the workspace in a state that does not compile. \
         `cargo check` reported the errors below in files YOU modified. The \
         runtime gates dispatch on a passing compile, so this turn re-runs \
         scoped to fix only the errors listed.\n\n",
    );
    out.push_str("Errors:\n\n");
    for err in errors {
        out.push_str(&format!(
            "  --> {}:{}:{}\n      {}\n\n",
            err.rel_path, err.line, err.col, err.message
        ));
    }
    out.push_str(
        "Fix instructions:\n\
         - Apply the minimum edit that makes the listed sites compile. Either \
           revert the change that introduced the mismatch, or update the \
           adjacent file (function signature, struct field, trait impl) to \
           match.\n\
         - Do NOT add new tests, do NOT refactor unrelated code, do NOT \
           reformat. The genome retry / review lanes will surface those \
           concerns separately.\n\
         - If the fix requires editing a file outside your declared scope \
           (the judge's `swarm_artifacts.files` array), do so anyway when \
           the change is the minimum needed to clear the compile error \u{2014} \
           the compile gate's mandate overrides the per-file scope lock for \
           the duration of THIS retry only.\n\
         - When done, reply briefly listing the files you touched.\n",
    );
    out
}

fn push_console_message(
    state: &mut AppState,
    agent_id: &str,
    attempt: u8,
    errors: &[CompileError],
) {
    let at = timestamp_label(state);
    let mut lines = Vec::new();
    lines.push(format!(
        "\u{21b3} [{}] compile gate {attempt}/{COMPILE_GATE_RETRY_LIMIT}",
        compact_agent_id_for_log(agent_id)
    ));
    for err in errors {
        let trunc = if err.message.len() > 100 {
            format!("{}\u{2026}", &err.message[..100])
        } else {
            err.message.clone()
        };
        lines.push(format!(
            "  \u{2717} {}:{}:{} {}",
            err.rel_path, err.line, err.col, trunc
        ));
    }
    state.agents.messages.push(AgentMessage {
        at: at.clone(),
        channel: AgentChannel::Broadcast,
        agent_id: Some(agent_id.to_string()),
        mission_id: None,
        text: lines.join("\n"),
        prompt_msg_idx: None,
        kind: Some("compile-retry".into()),
    });
    state.agents.console_scroll = nit_core::CONSOLE_SCROLL_BOTTOM;
}

fn push_diag_event(state: &mut AppState, agent_id: &str, attempt: u8, errors: &[CompileError]) {
    let at = timestamp_label(state);
    state.agents.diag_events.push(AgentDiagnosticEvent {
        severity: nit_core::AgentAlertSeverity::Warn,
        source: "compile-gate".into(),
        message: format!(
            "[{}] compile error \u{00d7} {} \u{2014} retry {attempt}/{COMPILE_GATE_RETRY_LIMIT}",
            compact_agent_id_for_log(agent_id),
            errors.len()
        ),
        at,
    });
    state.agents.note_event();
}

fn push_budget_exhausted_message(state: &mut AppState, agent_id: &str, errors: &[CompileError]) {
    let at = timestamp_label(state);
    state.agents.diag_events.push(AgentDiagnosticEvent {
        severity: nit_core::AgentAlertSeverity::Warn,
        source: "compile-gate".into(),
        message: format!(
            "[{}] compile gate budget exhausted ({} retries) \u{2014} {} error(s) remain",
            compact_agent_id_for_log(agent_id),
            COMPILE_GATE_RETRY_LIMIT,
            errors.len()
        ),
        at,
    });
    state.agents.note_event();
    // Also flip the agent to Error status so the roster surfaces the
    // failed state — operator-visible without scrolling diag logs.
    if let Some(agent) = state.agents.agents_get_mut(agent_id) {
        agent.status = AgentStatus::Error;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn crates_for_paths_extracts_unique_crate_names() {
        let workspace = PathBuf::from("/w");
        let paths = [
            PathBuf::from("/w/crates/nit-tui/src/app/fuzzy_runtime.rs"),
            PathBuf::from("/w/crates/nit-tui/src/lib.rs"),
            PathBuf::from("/w/crates/nit-core/src/state.rs"),
            PathBuf::from("/w/docs/README.md"),
        ];
        let crates = crates_for_paths(paths.iter().map(|p| p.as_path()), &workspace);
        assert_eq!(crates, vec!["nit-core".to_string(), "nit-tui".to_string()]);
    }

    #[test]
    fn crates_for_paths_skips_non_crate_paths() {
        let workspace = PathBuf::from("/w");
        let paths = [
            PathBuf::from("/w/docs/x.md"),
            PathBuf::from("/w/scripts/y.sh"),
            PathBuf::from("/elsewhere/z.rs"),
        ];
        let crates = crates_for_paths(paths.iter().map(|p| p.as_path()), &workspace);
        assert!(crates.is_empty());
    }

    #[test]
    fn build_retry_prompt_lists_errors_and_attempt_header() {
        let errors = vec![CompileError {
            rel_path: "crates/x/src/lib.rs".into(),
            line: 12,
            col: 4,
            message: " error[E0061]: bad arity".into(),
        }];
        let prompt = build_retry_prompt(1, 3, &errors);
        assert!(prompt.contains("COMPILE GATE \u{2014} automatic retry 1/3"));
        assert!(prompt.contains("crates/x/src/lib.rs:12:4"));
        assert!(prompt.contains("E0061"));
        assert!(prompt.contains("compile gate's mandate overrides"));
    }
}
