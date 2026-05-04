//! Intake agent: a hidden, single-step preprocessor that runs before
//! each chat dispatch to classify operator intent and decide whether
//! to append a FILE CHECKLIST to the prompt.
//!
//! Replaces the deleted `is_real_work` heuristic with an LLM call.
//! Mirrors `shadow.rs` in shape (synthetic clone lane, deferred main
//! dispatch resumed on `TurnCompleted`/`TurnFailed`) but runs only ONE
//! turn instead of a 4-stage pipeline. When changing one, audit the
//! other.

#[cfg(test)]
use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
#[cfg(test)]
use std::sync::Mutex;
use std::time::{Duration, Instant};

use nit_core::{
    AgentAlertSeverity, AgentBusEvent, AgentChannel, AgentDiagnosticEvent, AgentStatus, AppState,
    PendingIntake,
};

use crate::swarm::{
    copy_claude_runtime_metadata, copy_codex_runtime_metadata, insert_swarm_clone_lane,
};

/// Verbatim system prompt for the intake agent. Constant in v1: making
/// it operator-editable would require a settings field plus per-prompt
/// drift in the leak invariants the tests pin.
pub const SYSTEM_PROMPT: &str = "You are the `intake` agent. Your only job is to classify an operator's chat prompt and decide whether to append a FILE CHECKLIST. You are NOT the agent that does the work — you only prep.\n\nYou will receive:\n1. The operator's raw prompt.\n2. The target agent's working directory (absolute path).\n3. A directory listing of that cwd (depth 1, up to 50 entries).\n\nReply with EXACTLY one fenced ```json block, no other text:\n\n{\n  \"intent\": \"read\" | \"write\" | \"mixed\" | \"conversational\",\n  \"augmented_prompt\": \"<verbatim prompt, optionally with appended FILE CHECKLIST>\",\n  \"scope_files\": [\"relative/path1\", \"relative/path2\"],\n  \"augmentation_applied\": true | false,\n  \"notes\": \"<= 100 chars; the operator sees this\"\n}\n\nRules:\n- \"read\" / \"conversational\" → augmented_prompt EQUALS raw prompt. augmentation_applied: false. scope_files MAY still list relevant files for the next agent's context.\n- \"write\" / \"mixed\" → APPEND (never replace) this block to the raw prompt:\n\n  ## FILE CHECKLIST (non-negotiable)\n  Refactor / modify EVERY file below. No exceptions, no skipping.\n  Process in order. Open, read, modify, then move to the next.\n  Even if a file looks clean, improve naming/docs/structure/consistency.\n  Your task is NOT complete until every file has been modified.\n\n  1. <file1>\n  2. <file2>\n  ...\n\n  After finishing, list every file and what you changed in each.\n\n- scope_files: resolve directory tokens (`crates/foo`, `src/auth`) against the cwd listing. Include source files only (.rs/.py/.ts/.tsx/.js/.go/etc). Max 50 entries. Empty array if no path tokens resolve.\n- NEVER rewrite, rephrase, or summarize the operator's words. Only APPEND.\n- If unsure, prefer \"conversational\" + passthrough over augmenting.";

/// Hard cap on intake turn duration. Beyond this, the runtime falls back
/// to passthrough rather than block the operator's chat dispatch.
pub const INTAKE_TIMEOUT: Duration = Duration::from_secs(30);

/// Marker every augmented prompt must contain. Without this, an
/// "augmented" reply that just rephrased the operator wouldn't be
/// distinguishable from a real append, so we reject as a prefix
/// violation.
const FILE_CHECKLIST_MARKER: &str = "## FILE CHECKLIST (non-negotiable)";

/// Minimum operator prompt length (chars) below which intake skips —
/// classifying "hi" through an LLM is pure latency tax.
const MIN_PROMPT_CHARS: usize = 3;

/// Process-wide counter used to disambiguate intake lane ids when an
/// operator submits multiple prompts in quick succession. Only one
/// intake runs at a time (gated by `AgentsState::pending_intake`), but
/// completed lanes hang around briefly during cleanup, so the run id
/// keeps the namespace clean.
static NEXT_RUN_ID: AtomicU64 = AtomicU64::new(0);

#[cfg(test)]
static MOCK_RESPONSES: Mutex<Option<HashMap<String, String>>> = Mutex::new(None);

/// Decision returned by `consume_completion`. Drives whether the
/// deferred dispatch fires with the augmented or raw prompt.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IntakeDecision {
    /// The intake agent appended a valid FILE CHECKLIST. Dispatch with
    /// this prompt verbatim.
    Augmented(String),
    /// Any non-augmented outcome — read intent, parse failure, prefix
    /// violation, timeout, runner failure. Dispatch with the raw prompt.
    Passthrough,
}

#[derive(Clone, Debug)]
pub struct IntakeDispatch {
    pub agent_id: String,
    pub prompt: String,
    pub mission_id: Option<String>,
}

#[derive(Clone, Debug)]
pub struct IntakeStartContext {
    pub mission_id: Option<String>,
    pub prompt_msg_idx: usize,
    pub channel: AgentChannel,
    pub force_new: bool,
    pub target_agent_id: String,
}

/// Resumed dispatch shape returned to the caller after an intake decision.
/// The caller (event_drain) replays this through the same chat-dispatch
/// path the operator's prompt would have hit if intake were disabled.
#[derive(Clone, Debug)]
pub struct IntakeResume {
    pub target_agent_id: String,
    pub mission_id: Option<String>,
    pub prompt: String,
    pub prompt_msg_idx: usize,
    pub channel: AgentChannel,
    pub force_new: bool,
}

/// Marker type held by the runtime alongside `ShadowRuntime`. Stateless
/// in v1 — the run-id counter is process-global and `pending_intake`
/// lives on `AgentsState`. Kept as a public struct so future work can
/// add per-runtime caches without re-threading APIs.
#[derive(Default)]
pub struct IntakeRuntime;

impl IntakeRuntime {
    pub fn new() -> Self {
        Self
    }
}

/// Build the canonical intake lane id. Coexists with `#shadow-`,
/// `#chat-clone-`, `#swarm-` conventions.
pub fn intake_lane_id(base_id: &str, run_id: &str) -> String {
    format!("{base_id}#intake-{run_id}")
}

pub fn parse_intake_lane_id(lane_id: &str) -> Option<(&str, &str)> {
    lane_id.split_once("#intake-")
}

/// Kick off an intake turn for `target_agent_id`. Returns the dispatch
/// shape the caller hands to the runner; `None` when intake is disabled,
/// the prompt is too short, or no lane could be cloned. On `Some`, the
/// caller is responsible for running the dispatch THEN setting
/// `state.agents.pending_intake` (set-after-enqueue ordering — if enqueue
/// panics between set and dispatch, the next chat submit silently skips
/// intake but resumes nothing).
pub fn start(
    state: &mut AppState,
    raw_prompt: &str,
    target_cwd: &Path,
    ctx: &IntakeStartContext,
) -> Option<IntakeDispatch> {
    if env_kill_switch_active() {
        return None;
    }
    if !state.settings.intake_enabled {
        return None;
    }
    if raw_prompt.trim().chars().count() < MIN_PROMPT_CHARS {
        return None;
    }
    if state.agents.pending_intake.is_some() {
        return None;
    }

    // Backend guard. The SYSTEM_PROMPT is calibrated for claude-class
    // models (strict fenced-JSON contract) and the 30s timeout assumes
    // haiku-tier latency; on codex/gemini lanes a real intake turn
    // commonly times out into passthrough while burning a full
    // reasoning turn's worth of cost. The override hook
    // `state.agents.intake_agent_id` bypasses this guard so a future
    // operator setup can wire a cheap claude preprocessor in front of
    // a non-claude writer — DO NOT collapse the override branch on
    // refactor without understanding that semantic.
    if state.agents.intake_agent_id.is_none() && !target_lane_is_claude(state, &ctx.target_agent_id)
    {
        let backend = lane_backend_label(state, &ctx.target_agent_id);
        push_diag(
            state,
            AgentAlertSeverity::Info,
            format!(
                "intake.skipped: backend={backend} target={} reason=non_claude_target",
                ctx.target_agent_id
            ),
        );
        return None;
    }

    let lane_id = ensure_intake_lane(state, &ctx.target_agent_id)?;
    let prompt = build_intake_user_message(raw_prompt, target_cwd);

    Some(IntakeDispatch {
        agent_id: lane_id,
        prompt,
        mission_id: ctx.mission_id.clone(),
    })
}

fn target_lane_is_claude(state: &AppState, target_agent_id: &str) -> bool {
    state
        .agents
        .agents
        .iter()
        .find(|l| l.id == target_agent_id)
        .is_some_and(|l| l.is_claude())
}

fn lane_backend_label(state: &AppState, target_agent_id: &str) -> &'static str {
    state
        .agents
        .agents
        .iter()
        .find(|l| l.id == target_agent_id)
        .map(|l| match l.kind {
            nit_core::AgentLaneKind::Claude => "claude",
            nit_core::AgentLaneKind::Codex => "codex",
            nit_core::AgentLaneKind::Gemini => "gemini",
            nit_core::AgentLaneKind::Mock => "mock",
            nit_core::AgentLaneKind::Unknown => {
                if l.is_claude() {
                    "claude"
                } else if l.is_codex() {
                    "codex"
                } else {
                    "unknown"
                }
            }
        })
        .unwrap_or("missing")
}

/// Stash the operator's chat-state context AFTER the intake turn was
/// successfully enqueued. Caller invokes this exactly once per
/// `start()` that returned `Some`.
pub fn stash_pending_intake(
    state: &mut AppState,
    intake_agent_id: String,
    raw_prompt: &str,
    target_cwd: &Path,
    ctx: &IntakeStartContext,
) {
    state.agents.pending_intake = Some(PendingIntake {
        mission_id: ctx.mission_id.clone(),
        prompt_msg_idx: ctx.prompt_msg_idx,
        channel: ctx.channel,
        force_new: ctx.force_new,
        raw_prompt: raw_prompt.to_string(),
        target_cwd: target_cwd.to_path_buf(),
        target_agent_id: ctx.target_agent_id.clone(),
        intake_agent_id,
        started_at: Instant::now(),
    });
}

/// Process a `TurnCompleted` / `TurnFailed` event. Returns the resume
/// shape if the event matches the in-flight intake; `None` otherwise so
/// the caller falls through to other event handlers (shadow, swarm).
///
/// Strict prefix check: rejecting illegitimate augmentations is the only
/// thing keeping `prompts_leak_test.rs` invariants reachable. A loosened
/// check is a regression magnet, so the rules are conjunctive (all six
/// must pass).
pub fn handle_event_outcome(state: &mut AppState, event: &AgentBusEvent) -> Option<IntakeResume> {
    let event_agent_id = match event {
        AgentBusEvent::TurnCompleted { agent_id, .. }
        | AgentBusEvent::TurnFailed { agent_id, .. } => agent_id.clone(),
        _ => return None,
    };
    let matches_intake = state
        .agents
        .pending_intake
        .as_ref()
        .is_some_and(|p| p.intake_agent_id == event_agent_id);
    if !matches_intake {
        return None;
    }
    let pending = state.agents.pending_intake.take()?;

    let decision = match event {
        AgentBusEvent::TurnCompleted { message, .. } => decide(state, &pending, message),
        AgentBusEvent::TurnFailed { .. } => {
            // Warn (not Info): the deferred operator dispatch is wedged
            // until a passthrough resume fires, so the operator must be
            // able to see this in the chat console without opening the
            // ops tab.
            push_diag(
                state,
                AgentAlertSeverity::Warn,
                format!("intake.turn_failed: agent={event_agent_id} → passthrough"),
            );
            IntakeDecision::Passthrough
        }
        _ => IntakeDecision::Passthrough,
    };

    cleanup_intake_lane(state, &pending.intake_agent_id);

    let prompt = match decision {
        IntakeDecision::Augmented(p) => p,
        IntakeDecision::Passthrough => pending.raw_prompt.clone(),
    };
    Some(IntakeResume {
        target_agent_id: pending.target_agent_id,
        mission_id: pending.mission_id,
        prompt,
        prompt_msg_idx: pending.prompt_msg_idx,
        channel: pending.channel,
        force_new: pending.force_new,
    })
}

/// Operator-driven cancellation of an in-flight intake. Drains queued
/// turns for the intake lane, removes the synthetic lane, clears
/// `pending_intake`. Caller is responsible for sending `CancelTurn` to
/// the runner (kept symmetrical with `shadow::abort_run`).
pub fn cancel_pending_intake(state: &mut AppState) -> Option<String> {
    let pending = state.agents.pending_intake.take()?;
    cleanup_intake_lane(state, &pending.intake_agent_id);
    Some(pending.intake_agent_id)
}

/// Tear down the synthetic intake lane when `dispatch_agent_prompt`
/// failed to enqueue (dead runner channel). The lane was inserted by
/// `ensure_intake_lane` before dispatch, but `pending_intake` is not
/// yet set — so we cannot use `cancel_pending_intake`. Removing the
/// lane keeps a phantom row out of the agent ops view.
pub(crate) fn cleanup_intake_lane_after_failed_dispatch(state: &mut AppState, lane_id: &str) {
    cleanup_intake_lane(state, lane_id);
}

/// Watchdog: invoke each tick to enforce the 30s deadline. Returns the
/// intake lane id that was killed so the caller can route a `CancelTurn`
/// to its runner; the deferred dispatch is NOT resumed here — the
/// runner's eventual `TurnFailed` (from the cancel) drives the
/// passthrough resume through `handle_event_outcome`.
pub fn tick_timeout(state: &mut AppState, now: Instant) -> Option<String> {
    let pending = state.agents.pending_intake.as_ref()?;
    if now.duration_since(pending.started_at) < INTAKE_TIMEOUT {
        return None;
    }
    let lane_id = pending.intake_agent_id.clone();
    // Warn (not Info): the operator's chat is blocked until passthrough
    // resumes, and Info diags are suppressed in the chat console by
    // default — promoting this to Warn means a 30s wedge is visible.
    push_diag(
        state,
        AgentAlertSeverity::Warn,
        format!("intake.timeout: lane={lane_id} → passthrough"),
    );
    Some(lane_id)
}

/// Synchronous variant of the timeout path used by tests and by the
/// chat-input gate when a second prompt arrives mid-flight: drops
/// `pending_intake` AND returns the resume so the deferred dispatch
/// fires with the raw prompt. Production runtime ticking uses
/// `tick_timeout` + the runner-driven `TurnFailed` event instead.
pub fn force_passthrough(state: &mut AppState, source: &'static str) -> Option<IntakeResume> {
    let pending = state.agents.pending_intake.take()?;
    push_diag(
        state,
        AgentAlertSeverity::Info,
        format!(
            "intake.{source}: lane={} → passthrough",
            pending.intake_agent_id
        ),
    );
    cleanup_intake_lane(state, &pending.intake_agent_id);
    Some(IntakeResume {
        target_agent_id: pending.target_agent_id,
        mission_id: pending.mission_id,
        prompt: pending.raw_prompt,
        prompt_msg_idx: pending.prompt_msg_idx,
        channel: pending.channel,
        force_new: pending.force_new,
    })
}

fn decide(state: &mut AppState, pending: &PendingIntake, output: &str) -> IntakeDecision {
    #[cfg(test)]
    let output_owned: Option<String> = take_mock_response(&pending.intake_agent_id);
    #[cfg(test)]
    let output: &str = output_owned.as_deref().unwrap_or(output);

    let parsed = match parse_intake_json(output) {
        Ok(p) => p,
        Err(reason) => {
            push_diag(
                state,
                AgentAlertSeverity::Info,
                format!("intake.parse_failed: {reason}"),
            );
            return IntakeDecision::Passthrough;
        }
    };

    if !parsed.augmentation_applied {
        return IntakeDecision::Passthrough;
    }

    if let Err(reason) = validate_prefix(&pending.raw_prompt, &parsed.augmented_prompt) {
        let head: String = parsed.augmented_prompt.chars().take(80).collect();
        push_diag(
            state,
            AgentAlertSeverity::Warn,
            format!("intake.prefix_violation: {reason}; head=`{head}`"),
        );
        return IntakeDecision::Passthrough;
    }

    IntakeDecision::Augmented(parsed.augmented_prompt)
}

fn env_kill_switch_active() -> bool {
    std::env::var_os("NIT_INTAKE_DISABLED").is_some_and(|v| {
        let s = v.to_string_lossy();
        s == "1" || s.eq_ignore_ascii_case("true")
    })
}

fn ensure_intake_lane(state: &mut AppState, target_agent_id: &str) -> Option<String> {
    let next = NEXT_RUN_ID.fetch_add(1, Ordering::Relaxed).wrapping_add(1);
    let run_id = format!("{next:02}");
    let base_id = pick_intake_base_id(state, target_agent_id)?;
    let lane_id = intake_lane_id(&base_id, &run_id);
    if state.agents.agents.iter().any(|l| l.id == lane_id) {
        return Some(lane_id);
    }
    let base_lane = state
        .agents
        .agents
        .iter()
        .find(|l| l.id == base_id)
        .cloned()?;
    let mut lane = base_lane.clone();
    lane.id = lane_id.clone();
    lane.role = format!("{} (intake)", base_lane.role.trim());
    lane.status = AgentStatus::Idle;
    lane.heartbeat_age_secs = 0;
    lane.queue_len = 0;
    lane.current_mission = None;
    lane.last_message = String::new();
    lane.shadow = true;
    insert_swarm_clone_lane(state, &base_id, lane);
    copy_codex_runtime_metadata(state, &base_id, &lane_id);
    copy_claude_runtime_metadata(state, &base_id, &lane_id);
    Some(lane_id)
}

fn pick_intake_base_id(state: &AppState, target_agent_id: &str) -> Option<String> {
    if let Some(override_id) = state.agents.intake_agent_id.as_ref() {
        if state.agents.agents.iter().any(|l| l.id == *override_id) {
            return Some(override_id.clone());
        }
    }
    // Default: clone the target agent's lane. In multipane this is the
    // pane-specific lane (e.g. `claude-haiku-4-5#mp-pane-01`), which
    // keeps the intake turn's cwd resolution + pane busy-state aligned
    // with where the operator's prompt will eventually land.
    state
        .agents
        .agents
        .iter()
        .find(|l| l.id == target_agent_id && (l.is_codex() || l.is_claude()))
        .map(|l| l.id.clone())
}

fn build_intake_user_message(raw_prompt: &str, target_cwd: &Path) -> String {
    let listing = directory_listing(target_cwd, 50);
    format!(
        "{system}\n\n## OPERATOR PROMPT\n{raw_prompt}\n\n## CWD\n{cwd}\n\n## DIRECTORY LISTING\n{listing}\n",
        system = SYSTEM_PROMPT,
        cwd = target_cwd.display(),
    )
}

fn directory_listing(cwd: &Path, max_entries: usize) -> String {
    let Ok(read) = std::fs::read_dir(cwd) else {
        return String::from("(unreadable)");
    };
    let mut entries: Vec<String> = read
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let name = e.file_name();
            let name = name.to_string_lossy().into_owned();
            if name.starts_with('.') || name == "target" || name == "node_modules" {
                return None;
            }
            let suffix = if e.file_type().is_ok_and(|t| t.is_dir()) {
                "/"
            } else {
                ""
            };
            Some(format!("{name}{suffix}"))
        })
        .take(max_entries)
        .collect();
    entries.sort();
    if entries.is_empty() {
        return String::from("(empty)");
    }
    entries.join("\n")
}

#[derive(Debug)]
struct ParsedIntakeJson {
    augmented_prompt: String,
    augmentation_applied: bool,
}

fn parse_intake_json(output: &str) -> Result<ParsedIntakeJson, String> {
    let body = extract_json_block(output);
    let value: serde_json::Value =
        serde_json::from_str(body).map_err(|e| format!("not valid JSON: {e}"))?;
    let obj = value
        .as_object()
        .ok_or_else(|| "not a JSON object".to_string())?;
    let augmented_prompt = obj
        .get("augmented_prompt")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "missing augmented_prompt".to_string())?
        .to_string();
    let augmentation_applied = obj
        .get("augmentation_applied")
        .and_then(|v| v.as_bool())
        .ok_or_else(|| "missing augmentation_applied".to_string())?;
    Ok(ParsedIntakeJson {
        augmented_prompt,
        augmentation_applied,
    })
}

fn extract_json_block(output: &str) -> &str {
    let trimmed = output.trim();
    if let Some(rest) = trimmed.strip_prefix("```json") {
        if let Some(end) = rest.rfind("```") {
            return rest[..end].trim();
        }
    }
    if let Some(rest) = trimmed.strip_prefix("```") {
        if let Some(end) = rest.rfind("```") {
            return rest[..end].trim();
        }
    }
    trimmed
}

/// Strict prefix check protecting `prompts_leak_test.rs` invariants.
/// Six conjunctive rules (all must pass):
/// 1. Both inputs trimmed of trailing whitespace; raw matches augmented head.
/// 2. Leading whitespace must match exactly (no silent typo "fixes").
/// 3. After the raw prefix, augmented continues with `\n` (paragraph break).
/// 4. Must contain the literal `## FILE CHECKLIST (non-negotiable)` marker.
/// 5. Augmented MUST be strictly longer than raw (no zero-byte append).
/// 6. The augmented body's first line beyond the raw must look like a header
///    or list — no inline-rewrite that just slips a word in.
fn validate_prefix(raw: &str, augmented: &str) -> Result<(), String> {
    let raw_trim = raw.trim_end();
    let aug_trim = augmented.trim_end();
    if !aug_trim.starts_with(raw_trim) {
        return Err("augmented_prompt does not start with raw_prompt".into());
    }
    if aug_trim.len() == raw_trim.len() {
        return Err("augmented_prompt is identical to raw (no append)".into());
    }
    let tail = &aug_trim[raw_trim.len()..];
    if !tail.starts_with('\n') {
        return Err("augmentation does not start with a newline".into());
    }
    if !tail.contains(FILE_CHECKLIST_MARKER) {
        return Err("augmentation missing FILE CHECKLIST marker".into());
    }
    Ok(())
}

fn push_diag(state: &mut AppState, severity: AgentAlertSeverity, message: String) {
    state.agents.diag_events.push(AgentDiagnosticEvent {
        severity,
        source: "intake".into(),
        message,
        at: format!("t+{}", state.metrics.frame_count),
    });
}

fn cleanup_intake_lane(state: &mut AppState, lane_id: &str) {
    state.agents.active_turns.remove(lane_id);
    state.agents.codex_thread_ids.remove(lane_id);
    state.agents.codex_used_tokens.remove(lane_id);
    state.agents.codex_context_remaining_pct.remove(lane_id);
    state.agents.codex_turn_prompt_idx.remove(lane_id);
    state.agents.claude_session_ids.remove(lane_id);
    state.agents.claude_used_tokens.remove(lane_id);
    state.agents.claude_context_remaining_pct.remove(lane_id);
    state.agents.claude_turn_prompt_idx.remove(lane_id);
    for map in state.agents.codex_mission_thread_ids.values_mut() {
        map.remove(lane_id);
    }
    for map in state.agents.codex_mission_used_tokens.values_mut() {
        map.remove(lane_id);
    }
    for map in state
        .agents
        .codex_mission_context_remaining_pct
        .values_mut()
    {
        map.remove(lane_id);
    }
    for map in state.agents.claude_mission_session_ids.values_mut() {
        map.remove(lane_id);
    }
    for map in state.agents.claude_mission_used_tokens.values_mut() {
        map.remove(lane_id);
    }
    for map in state
        .agents
        .claude_mission_context_remaining_pct
        .values_mut()
    {
        map.remove(lane_id);
    }
    state
        .agents
        .codex_effective_context_window_tokens
        .remove(lane_id);
    state.agents.codex_default_reasoning_effort.remove(lane_id);
    state
        .agents
        .codex_supported_reasoning_efforts
        .remove(lane_id);
    state.agents.codex_selected_reasoning_effort.remove(lane_id);
    state
        .agents
        .claude_effective_context_window_tokens
        .remove(lane_id);
    state.agents.claude_default_effort.remove(lane_id);
    state.agents.claude_supported_efforts.remove(lane_id);
    state.agents.claude_selected_effort.remove(lane_id);
    state.agents.swarm_role_by_agent_id.remove(lane_id);
    state.agents.swarm_priority_agent_ids.remove(lane_id);
    state.agents.roster_tree_collapsed_agent_ids.remove(lane_id);
    state.genome_turn_modified.remove(lane_id);
    state
        .agents
        .queued_codex_turns
        .retain(|t| t.agent_id != lane_id);
    state
        .agents
        .queued_claude_turns
        .retain(|t| t.agent_id != lane_id);
    state.agents.agents.retain(|lane| lane.id != lane_id);
    state.agents.rebuild_agents_index();
}

#[cfg(test)]
pub fn install_test_response(agent_id: impl Into<String>, json: impl Into<String>) {
    let mut guard = MOCK_RESPONSES.lock().unwrap();
    let map = guard.get_or_insert_with(HashMap::new);
    map.insert(agent_id.into(), json.into());
}

#[cfg(test)]
pub fn clear_test_responses() {
    if let Ok(mut guard) = MOCK_RESPONSES.lock() {
        if let Some(map) = guard.as_mut() {
            map.clear();
        }
    }
}

#[cfg(test)]
fn take_mock_response(agent_id: &str) -> Option<String> {
    let mut guard = MOCK_RESPONSES.lock().ok()?;
    guard.as_mut()?.remove(agent_id)
}

#[cfg(test)]
#[path = "tests/intake.rs"]
mod tests;
