//! Tests for the intake agent — the LLM-based pre-dispatch classifier
//! that replaced the deleted `is_real_work` heuristic.
//!
//! The fixtures mirror `prompts_leak_test.rs`: a real (in-memory)
//! `ClaudeRunner` keeps the post-dispatch queue walker from draining the
//! orphaned queue, so the intake turn AND the resumed operator turn stay
//! inspectable. Mocked JSON is injected via `intake::install_test_response`
//! so no real LLM is invoked.

use std::fs;
use std::path::PathBuf;
use std::time::Instant;

use nit_core::state::AgentTurnState;
use nit_core::{
    AgentBusEvent, AgentLane, AgentLaneKind, AgentStatus, AppState, Buffer, MultipaneState,
    PaneSession,
};

use crate::claude_runner::{ClaudeRunner, ClaudeRunnerConfig};
use crate::intake::{
    self, clear_test_responses, install_test_response, parse_intake_lane_id, IntakeResume,
    IntakeStartContext,
};
use crate::shadow::ShadowRuntime;
use crate::swarm::SwarmRuntime;
use crate::vitals::VitalsState;

const RAW_REAL_WORK: &str = "Update crates/foo to extract the iterator helper";
const RAW_QUESTION: &str = "what does the dispatcher do?";
const RAW_GREETING: &str = "hi there friend";

fn unique_suffix() -> String {
    format!(
        "{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default(),
    )
}

fn fresh_dir(label: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("nit-intake-{label}-{}", unique_suffix()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn cargo_workspace(label: &str) -> PathBuf {
    let dir = fresh_dir(label);
    fs::write(dir.join("Cargo.toml"), "[workspace]\nmembers=[]\n").unwrap();
    fs::create_dir_all(dir.join("crates/foo/src")).unwrap();
    fs::write(dir.join("crates/foo/src/lib.rs"), "// foo\n").unwrap();
    dir
}

fn fresh_state(cwd: PathBuf, intake_on: bool) -> AppState {
    let editor = Buffer::empty("editor", None);
    let notes = Buffer::empty("notes", None);
    let mut state = AppState::new(cwd, editor, notes);
    state.settings.intake_enabled = intake_on;
    state.agents.messages.clear();
    state.agents.agents.clear();
    state
}

fn lane(id: &str, kind: AgentLaneKind, label: &str) -> AgentLane {
    AgentLane {
        id: id.into(),
        role: id.into(),
        lane: label.into(),
        kind,
        status: AgentStatus::Idle,
        heartbeat_age_secs: 0,
        queue_len: 0,
        current_mission: None,
        last_message: String::new(),
        shadow: false,
    }
}

fn install_active_turn(state: &mut AppState, lane_id: &str) {
    let now = Instant::now();
    state.agents.active_turns.insert(
        lane_id.into(),
        AgentTurnState {
            started_at: now,
            last_heartbeat_at: now,
            last_output_at: now,
            stage: None,
        },
    );
}

fn make_state(cwd: PathBuf, intake_on: bool) -> AppState {
    make_state_with_lane(
        cwd,
        intake_on,
        "claude-haiku-4-5",
        AgentLaneKind::Claude,
        "Claude",
    )
}

fn make_state_with_lane(
    cwd: PathBuf,
    intake_on: bool,
    lane_id: &str,
    lane_kind: AgentLaneKind,
    lane_label: &str,
) -> AppState {
    let mut state = fresh_state(cwd, intake_on);
    state
        .agents
        .agents
        .push(lane(lane_id, lane_kind, lane_label));
    state.agents.selected_agent = Some(lane_id.into());
    install_active_turn(&mut state, lane_id);
    state
}

fn intake_lane_id_in_state(state: &AppState) -> Option<String> {
    state
        .agents
        .agents
        .iter()
        .find(|l| parse_intake_lane_id(&l.id).is_some())
        .map(|l| l.id.clone())
}

fn ctx_for(state: &AppState) -> IntakeStartContext {
    IntakeStartContext {
        mission_id: None,
        prompt_msg_idx: 0,
        channel: nit_core::AgentChannel::Agent,
        force_new: false,
        target_agent_id: state.agents.selected_agent.clone().expect("selected agent"),
    }
}

/// Drive the intake decision pipeline end-to-end without going through
/// `submit_chat_input_and_dispatch` — production wiring is covered by the
/// `chat_dispatch_*` tests in `prompts_leak_test.rs`. This exercises only
/// the intake module's contract: start → install mock → synthesize
/// `TurnCompleted` → return the resume.
fn drive_intake_decision(
    state: &mut AppState,
    raw_prompt: &str,
    mock_json: &str,
) -> Option<IntakeResume> {
    let target_cwd = state.workspace_root.clone();
    let ctx = ctx_for(state);
    let dispatch = intake::start(state, raw_prompt, target_cwd.as_path(), &ctx)
        .expect("intake start succeeded");
    let intake_lane = dispatch.agent_id.clone();
    intake::stash_pending_intake(
        state,
        intake_lane.clone(),
        raw_prompt,
        target_cwd.as_path(),
        &ctx,
    );
    install_test_response(intake_lane.clone(), mock_json.to_string());
    let event = AgentBusEvent::TurnCompleted {
        agent_id: intake_lane,
        mission_id: None,
        message: "ignored".into(),
        thread_id: None,
        token_count: None,
    };
    intake::handle_event_outcome(state, &event)
}

fn drive_intake_failed_decision(state: &mut AppState, raw_prompt: &str) -> Option<IntakeResume> {
    let target_cwd = state.workspace_root.clone();
    let ctx = ctx_for(state);
    let dispatch = intake::start(state, raw_prompt, target_cwd.as_path(), &ctx)
        .expect("intake start succeeded");
    let intake_lane = dispatch.agent_id.clone();
    intake::stash_pending_intake(
        state,
        intake_lane.clone(),
        raw_prompt,
        target_cwd.as_path(),
        &ctx,
    );
    let event = AgentBusEvent::TurnFailed {
        agent_id: intake_lane,
        mission_id: None,
        thread_id: None,
        token_count: None,
        message: "intake runner crash".into(),
    };
    intake::handle_event_outcome(state, &event)
}

fn build_augmented(raw: &str, files: &[&str]) -> String {
    let mut out = format!("{raw}\n\n## FILE CHECKLIST (non-negotiable)\n");
    out.push_str("Refactor / modify EVERY file below.\n");
    for (i, p) in files.iter().enumerate() {
        out.push_str(&format!("{}. {p}\n", i + 1));
    }
    out
}

fn json_response(intent: &str, augmented: &str, augmentation_applied: bool) -> String {
    let json = serde_json::json!({
        "intent": intent,
        "augmented_prompt": augmented,
        "scope_files": [],
        "augmentation_applied": augmentation_applied,
        "notes": "test"
    });
    format!("```json\n{json}\n```")
}

fn diag_with_prefix<'a>(
    state: &'a AppState,
    after: usize,
    prefix: &str,
) -> &'a nit_core::state::AgentDiagnosticEvent {
    state
        .agents
        .diag_events
        .iter()
        .skip(after)
        .find(|d| d.message.starts_with(prefix))
        .unwrap_or_else(|| panic!("expected diag with prefix `{prefix}`"))
}

// --------------------------------------------------------------------
// Test 1 — each intent class produces the correct resume prompt
// --------------------------------------------------------------------
#[test]
fn intake_classifies_each_intent_class_lands_correct_prompt_in_queue() {
    let cwd = cargo_workspace("intent_class");

    // 1a — read intent → passthrough.
    {
        let mut state = make_state(cwd.clone(), true);
        let mock = json_response("read", RAW_QUESTION, false);
        let resume = drive_intake_decision(&mut state, RAW_QUESTION, &mock).expect("resume");
        assert_eq!(resume.prompt, RAW_QUESTION);
        assert!(state.agents.pending_intake.is_none());
    }

    // 1b — write intent → augmented prompt with FILE CHECKLIST.
    {
        let mut state = make_state(cwd.clone(), true);
        let augmented = build_augmented(RAW_REAL_WORK, &["crates/foo/src/lib.rs"]);
        let mock = json_response("write", &augmented, true);
        let resume = drive_intake_decision(&mut state, RAW_REAL_WORK, &mock).expect("resume");
        assert!(resume.prompt.starts_with(RAW_REAL_WORK));
        assert!(resume.prompt.contains("## FILE CHECKLIST (non-negotiable)"));
    }

    // 1c — mixed intent → also augmented.
    {
        let mut state = make_state(cwd.clone(), true);
        let augmented = build_augmented(RAW_REAL_WORK, &["crates/foo/src/lib.rs"]);
        let mock = json_response("mixed", &augmented, true);
        let resume = drive_intake_decision(&mut state, RAW_REAL_WORK, &mock).expect("resume");
        assert!(resume.prompt.contains("FILE CHECKLIST"));
    }

    // 1d — conversational → passthrough.
    {
        let mut state = make_state(cwd.clone(), true);
        let mock = json_response("conversational", RAW_GREETING, false);
        let resume = drive_intake_decision(&mut state, RAW_GREETING, &mock).expect("resume");
        assert_eq!(resume.prompt, RAW_GREETING);
    }

    let _ = fs::remove_dir_all(&cwd);
}

// --------------------------------------------------------------------
// Test 2 — JSON parse failure → raw prompt + Info diag
// --------------------------------------------------------------------
#[test]
fn intake_parse_failure_falls_back_to_raw_prompt() {
    let cwd = cargo_workspace("parse_fail");
    let mut state = make_state(cwd.clone(), true);
    let initial = state.agents.diag_events.len();

    let resume = drive_intake_decision(&mut state, RAW_REAL_WORK, "this is not JSON at all")
        .expect("resume");
    assert_eq!(resume.prompt, RAW_REAL_WORK);
    let diag = diag_with_prefix(&state, initial, "intake.parse_failed");
    assert_eq!(diag.severity, nit_core::AgentAlertSeverity::Info);

    let _ = fs::remove_dir_all(&cwd);
}

// --------------------------------------------------------------------
// Test 3 — runner failure (TurnFailed) → raw prompt + Info diag
// --------------------------------------------------------------------
#[test]
fn intake_timeout_falls_back_to_raw_prompt() {
    let cwd = cargo_workspace("timeout");
    let mut state = make_state(cwd.clone(), true);
    let initial = state.agents.diag_events.len();

    let resume = drive_intake_failed_decision(&mut state, RAW_REAL_WORK).expect("resume");
    assert_eq!(resume.prompt, RAW_REAL_WORK);
    // Promoted Info → Warn: deferred dispatch is wedged on this event and
    // the chat console suppresses Info by default.
    let diag = diag_with_prefix(&state, initial, "intake.turn_failed");
    assert_eq!(diag.severity, nit_core::AgentAlertSeverity::Warn);

    let _ = fs::remove_dir_all(&cwd);
}

// --------------------------------------------------------------------
// Test 3b — explicit deadline tick → drops pending and surfaces lane
// --------------------------------------------------------------------
#[test]
fn intake_tick_timeout_kills_pending_after_deadline() {
    let cwd = cargo_workspace("tick_timeout");
    let mut state = make_state(cwd.clone(), true);
    let target_cwd = state.workspace_root.clone();
    let ctx = ctx_for(&state);
    let dispatch =
        intake::start(&mut state, RAW_REAL_WORK, target_cwd.as_path(), &ctx).expect("intake start");
    let intake_lane = dispatch.agent_id.clone();
    intake::stash_pending_intake(
        &mut state,
        intake_lane.clone(),
        RAW_REAL_WORK,
        target_cwd.as_path(),
        &ctx,
    );

    let deadline = state.agents.pending_intake.as_ref().unwrap().started_at
        + intake::INTAKE_TIMEOUT
        + std::time::Duration::from_secs(1);
    let killed = intake::tick_timeout(&mut state, deadline);
    assert_eq!(killed.as_deref(), Some(intake_lane.as_str()));
    assert!(state.agents.pending_intake.is_some());
    let resume = intake::force_passthrough(&mut state, "timeout").expect("resume");
    assert_eq!(resume.prompt, RAW_REAL_WORK);
    assert!(state.agents.pending_intake.is_none());

    let _ = fs::remove_dir_all(&cwd);
}

// --------------------------------------------------------------------
// Test 4 — prefix-check rejection → raw prompt + Warn diag
// --------------------------------------------------------------------
#[test]
fn intake_prefix_violation_falls_back_to_raw_prompt_with_warn() {
    let cwd = cargo_workspace("prefix_violation");
    let mut state = make_state(cwd.clone(), true);
    let initial = state.agents.diag_events.len();
    // Augmented prompt rewrites operator's words by prefixing "Hi! ".
    // Strict prefix check must reject this and fall back to raw.
    let bad_augmented = format!(
        "Hi! {RAW_REAL_WORK}\n\n## FILE CHECKLIST (non-negotiable)\n1. crates/foo/src/lib.rs\n"
    );
    let mock = json_response("write", &bad_augmented, true);
    let resume = drive_intake_decision(&mut state, RAW_REAL_WORK, &mock).expect("resume");
    assert_eq!(resume.prompt, RAW_REAL_WORK);
    let diag = diag_with_prefix(&state, initial, "intake.prefix_violation");
    assert_eq!(diag.severity, nit_core::AgentAlertSeverity::Warn);

    let _ = fs::remove_dir_all(&cwd);
}

// --------------------------------------------------------------------
// Test 5 — intake_enabled = false → no intake turn enqueued
// --------------------------------------------------------------------
#[test]
fn intake_disabled_skips_intake_turn() {
    clear_test_responses();
    let cwd = cargo_workspace("disabled");
    let claude = ClaudeRunner::spawn(ClaudeRunnerConfig::default());

    let mut state = make_state(cwd.clone(), false);
    state.agents.chat_input = RAW_REAL_WORK.into();
    let mut vitals = VitalsState::default();
    let mut swarm = SwarmRuntime::default();
    let mut shadow = ShadowRuntime::default();
    let _ = crate::app::submit_chat_input_and_dispatch(
        &mut state,
        &mut vitals,
        None,
        Some(&claude),
        &mut swarm,
        &mut shadow,
    );

    assert!(intake_lane_id_in_state(&state).is_none());
    assert!(state.agents.pending_intake.is_none());
    // Operator's prompt landed in the queue verbatim.
    let queued: Vec<_> = state.agents.queued_claude_turns.iter().collect();
    assert_eq!(queued.len(), 1);
    assert_eq!(queued[0].prompt, RAW_REAL_WORK);

    let _ = fs::remove_dir_all(&cwd);
}

// --------------------------------------------------------------------
// Test 6 — multipane: per-pane cwd reaches the intake input
// --------------------------------------------------------------------
#[test]
fn intake_uses_per_pane_cwd_in_multipane() {
    use crate::multipane::setup::materialise_pane_lane;

    clear_test_responses();
    let unique = unique_suffix();
    let cwd0 = std::env::temp_dir().join(format!("nit-intake-mp-cwd0-{unique}"));
    let cwd1 = std::env::temp_dir().join(format!("nit-intake-mp-cwd1-{unique}"));
    let _ = fs::remove_dir_all(&cwd0);
    let _ = fs::remove_dir_all(&cwd1);
    fs::create_dir_all(cwd0.join("crates/foo/src")).unwrap();
    fs::create_dir_all(cwd1.join("crates/bar/src")).unwrap();
    fs::write(cwd0.join("crates/foo/src/lib.rs"), "// foo\n").unwrap();
    fs::write(cwd1.join("crates/bar/src/lib.rs"), "// bar\n").unwrap();

    let mut state = fresh_state(PathBuf::from("/workspace"), true);
    state
        .agents
        .agents
        .push(lane("claude-haiku-4-5", AgentLaneKind::Claude, "Claude"));
    state.multipane = Some(MultipaneState {
        backend_agent_id: "claude-haiku-4-5".into(),
        panes: vec![
            PaneSession {
                pane_id: 0,
                cwd: cwd0.clone(),
                ..PaneSession::default()
            },
            PaneSession {
                pane_id: 1,
                cwd: cwd1.clone(),
                ..PaneSession::default()
            },
        ],
        focused: 1,
        grid_cols: 2,
        grid_rows: 1,
        backend_filter: Some("claude-haiku-4-5".into()),
        help_open: false,
    });
    let _ = materialise_pane_lane(&mut state, 0, "claude-haiku-4-5");
    let _ = materialise_pane_lane(&mut state, 1, "claude-haiku-4-5");

    // The pane-aware caller resolves target_cwd via resolve_dispatch_cwd;
    // mirror that here. End-to-end coverage of the chat path lives in
    // tests 1–5; this pins the per-pane cwd plumbing in isolation.
    let pane1_target = "claude-haiku-4-5#mp-pane-01".to_string();
    let target_cwd = crate::app::resolve_dispatch_cwd(&state, &pane1_target);
    assert_eq!(target_cwd, cwd1);
    let ctx = IntakeStartContext {
        mission_id: None,
        prompt_msg_idx: 0,
        channel: nit_core::AgentChannel::Agent,
        force_new: false,
        target_agent_id: pane1_target,
    };
    let dispatch = intake::start(
        &mut state,
        "Update crates/bar to consolidate the helper",
        target_cwd.as_path(),
        &ctx,
    )
    .expect("intake start succeeded");

    let cwd1_str = cwd1.display().to_string();
    assert!(
        dispatch.prompt.contains(&cwd1_str),
        "intake input must carry pane 1's cwd `{cwd1_str}`:\n{}",
        dispatch.prompt
    );
    let cwd0_str = cwd0.display().to_string();
    assert!(
        !dispatch.prompt.contains(&cwd0_str),
        "intake input must NOT carry pane 0's cwd:\n{}",
        dispatch.prompt
    );
    assert!(
        dispatch
            .agent_id
            .starts_with("claude-haiku-4-5#mp-pane-01#intake-"),
        "intake lane base must be the pane 1 lane, got `{}`",
        dispatch.agent_id
    );

    let _ = fs::remove_dir_all(&cwd0);
    let _ = fs::remove_dir_all(&cwd1);
}

// --------------------------------------------------------------------
// Sanity — abort drops pending_intake and does NOT fire deferred dispatch
// --------------------------------------------------------------------
#[test]
fn intake_abort_drops_pending_and_does_not_resume() {
    let cwd = cargo_workspace("abort");
    let mut state = make_state(cwd.clone(), true);
    let target_cwd = state.workspace_root.clone();
    let ctx = ctx_for(&state);
    let dispatch =
        intake::start(&mut state, RAW_REAL_WORK, target_cwd.as_path(), &ctx).expect("intake start");
    let intake_lane = dispatch.agent_id.clone();
    intake::stash_pending_intake(
        &mut state,
        intake_lane.clone(),
        RAW_REAL_WORK,
        target_cwd.as_path(),
        &ctx,
    );

    let cancelled = intake::cancel_pending_intake(&mut state);
    assert_eq!(cancelled.as_deref(), Some(intake_lane.as_str()));
    assert!(state.agents.pending_intake.is_none());
    assert!(intake_lane_id_in_state(&state).is_none());

    // Stale TurnCompleted for the intake lane returns None — no resume.
    let event = AgentBusEvent::TurnCompleted {
        agent_id: intake_lane,
        mission_id: None,
        message: "{}".into(),
        thread_id: None,
        token_count: None,
    };
    assert!(intake::handle_event_outcome(&mut state, &event).is_none());

    let _ = fs::remove_dir_all(&cwd);
}

// --------------------------------------------------------------------
// Backend guard tests — intake's system prompt and 30s timeout are
// claude-tuned, so a codex/gemini/mock target with no override must skip
// intake and surface a diag instead of routing through a misfit classifier.
// --------------------------------------------------------------------

fn intake_skip_diag(state: &AppState, after: usize) -> Option<&str> {
    state
        .agents
        .diag_events
        .iter()
        .skip(after)
        .find(|d| {
            d.severity == nit_core::AgentAlertSeverity::Info
                && d.message.starts_with("intake.skipped")
        })
        .map(|d| d.message.as_str())
}

fn assert_skipped_for_backend(label: &str, lane_id: &str, kind: AgentLaneKind, lane_label: &str) {
    let cwd = cargo_workspace(label);
    let mut state = make_state_with_lane(cwd.clone(), true, lane_id, kind, lane_label);
    let initial = state.agents.diag_events.len();
    let target_cwd = state.workspace_root.clone();
    let ctx = ctx_for(&state);
    let dispatch = intake::start(&mut state, RAW_REAL_WORK, target_cwd.as_path(), &ctx);
    assert!(
        dispatch.is_none(),
        "intake must skip {lane_label} target without an override"
    );
    assert!(state.agents.pending_intake.is_none());
    let msg = intake_skip_diag(&state, initial).expect("intake.skipped Info diag");
    let backend_label = lane_label.to_lowercase();
    assert!(
        msg.contains(&format!("backend={backend_label}")),
        "diag should encode backend label: {msg}"
    );
    assert!(
        msg.contains(&format!("target={lane_id}")),
        "diag should name the target: {msg}"
    );
    assert!(intake_lane_id_in_state(&state).is_none());
    let _ = fs::remove_dir_all(&cwd);
}

#[test]
fn intake_silent_skip_for_codex_target_emits_diag() {
    assert_skipped_for_backend("codex_skip", "gpt-5-codex", AgentLaneKind::Codex, "Codex");
}

#[test]
fn intake_silent_skip_for_gemini_target_emits_diag() {
    assert_skipped_for_backend(
        "gemini_skip",
        "gemini-flash",
        AgentLaneKind::Gemini,
        "Gemini",
    );
}

// Operator pinned a claude lane as the intake source; targeting a codex lane
// for the actual write must still fire intake (override path bypasses the
// backend guard so a future setup can run a cheap claude preprocessor in
// front of a codex writer).
#[test]
fn intake_override_lets_claude_lane_run_for_codex_target() {
    let cwd = cargo_workspace("override_path");
    let mut state = fresh_state(cwd.clone(), true);
    state
        .agents
        .agents
        .push(lane("claude-haiku-4-5", AgentLaneKind::Claude, "Claude"));
    state
        .agents
        .agents
        .push(lane("gpt-5-codex", AgentLaneKind::Codex, "Codex"));
    state.agents.selected_agent = Some("gpt-5-codex".into());
    state.agents.intake_agent_id = Some("claude-haiku-4-5".into());

    let target_cwd = state.workspace_root.clone();
    let ctx = ctx_for(&state);
    let dispatch = intake::start(&mut state, RAW_REAL_WORK, target_cwd.as_path(), &ctx)
        .expect("override path: intake must fire on codex target with claude override");
    assert!(
        dispatch.agent_id.starts_with("claude-haiku-4-5#intake-"),
        "intake lane must be the override claude lane, got {}",
        dispatch.agent_id
    );
    let _ = fs::remove_dir_all(&cwd);
}

// `NIT_INTAKE_DISABLED` is a process-wide env var that every `intake::start`
// call reads. The static LOCK serializes against any other test that might
// race on it (today none — but the lock is cheap insurance).
#[test]
fn intake_kill_switch_takes_precedence() {
    use std::sync::Mutex;
    static LOCK: Mutex<()> = Mutex::new(());
    let _guard = LOCK.lock().unwrap();
    const VAR: &str = "NIT_INTAKE_DISABLED";
    let prior = std::env::var(VAR).ok();

    let cwd = cargo_workspace("kill_switch");
    let mut state = make_state(cwd.clone(), true);
    let target_cwd = state.workspace_root.clone();
    let ctx = ctx_for(&state);

    // Sanity: intake fires when the kill switch is unset.
    std::env::remove_var(VAR);
    let dispatch = intake::start(&mut state, RAW_REAL_WORK, target_cwd.as_path(), &ctx);
    assert!(dispatch.is_some(), "intake must fire without kill switch");
    if let Some(d) = dispatch {
        intake::cleanup_intake_lane_after_failed_dispatch(&mut state, &d.agent_id);
    }

    // With the kill switch on, intake skips even when intake_enabled is true.
    std::env::set_var(VAR, "1");
    let dispatch = intake::start(&mut state, RAW_REAL_WORK, target_cwd.as_path(), &ctx);
    assert!(
        dispatch.is_none(),
        "kill switch must override intake_enabled"
    );

    match prior {
        Some(value) => std::env::set_var(VAR, value),
        None => std::env::remove_var(VAR),
    }
    let _ = fs::remove_dir_all(&cwd);
}

// The dispatch helpers tie `read_only` to `parse_shadow_lane_id ||
// parse_intake_lane_id`. Asserting the parser recognises an intake lane id
// pins the safety contract: the runner-config code path branches on this
// exact predicate.
#[test]
fn intake_lane_is_read_only_via_parse() {
    let lane_id = intake::intake_lane_id("claude-haiku-4-5", "01");
    assert_eq!(
        intake::parse_intake_lane_id(&lane_id),
        Some(("claude-haiku-4-5", "01")),
    );
    let mp_lane = intake::intake_lane_id("claude-haiku-4-5#mp-pane-01", "07");
    assert_eq!(
        intake::parse_intake_lane_id(&mp_lane),
        Some(("claude-haiku-4-5#mp-pane-01", "07")),
    );
}

// Simulates the chat-input path's recovery when `dispatch_agent_prompt`
// fails to enqueue (dead runner channel): the synthetic intake lane was
// inserted by `ensure_intake_lane` but `pending_intake` is not yet stashed.
// The cleanup helper removes the lane so it doesn't surface as a phantom row.
#[test]
fn intake_failed_dispatch_cleanup_removes_phantom_lane() {
    let cwd = cargo_workspace("phantom_lane");
    let mut state = make_state(cwd.clone(), true);
    let target_cwd = state.workspace_root.clone();
    let ctx = ctx_for(&state);
    let dispatch = intake::start(&mut state, RAW_REAL_WORK, target_cwd.as_path(), &ctx)
        .expect("intake start succeeds");
    assert!(intake_lane_id_in_state(&state).is_some());
    intake::cleanup_intake_lane_after_failed_dispatch(&mut state, &dispatch.agent_id);
    assert!(intake_lane_id_in_state(&state).is_none());
    assert!(state.agents.pending_intake.is_none());
    let _ = fs::remove_dir_all(&cwd);
}
