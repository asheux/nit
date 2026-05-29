use crate::state::{
    AgentAlert, AgentDiagnosticEvent, AgentLane, AgentLaneKind, AgentMessage, AgentStatus,
    AppState, McpStatus, MissionRecord, CONSOLE_SCROLL_BOTTOM,
};

pub use crate::genome_storage::{
    delete_genome_report, gc_genome_cache, load_genome_reports, persist_genome_report,
};

pub mod async_queue;
mod claims_signals;
mod file_ops;
mod helpers;
mod mood_control;
mod token_count;
mod turn_completion;
mod turn_error;
mod turn_lifecycle;
mod upsert;

#[cfg(test)]
pub(crate) use token_count::apply_codex_token_count;

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AgentTokenCount {
    pub total_tokens: u32,
    pub context_window: u32,
}

/// Sentinel `TurnFailed.message` set by the codex/claude runners when the
/// operator triggers `/abort`, Ctrl+C, Esc-Esc, or Mission-tab `x`. The bus
/// handler matches on this exact string to route the event down the soft
/// "cancelled" path (Idle status, Info diag) instead of the error path
/// (Error status, alert banner, substrate Warning). Keep the runner
/// emit-side and bus match-side in sync via this single constant.
pub const OPERATOR_CANCEL_TURN_MESSAGE: &str = "Cancelled by operator";

/// Event protocol for driving the Agent Station UI from an external runtime
/// (Codex, Claude, etc.). Transported as NDJSON over stdio or a socket;
/// each variant maps to a single state mutation.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentBusEvent {
    AgentUpsert {
        agent: AgentLane,
    },
    MissionUpsert {
        mission: MissionRecord,
    },
    MessageAppend {
        message: AgentMessage,
    },
    AlertAppend {
        alert: AgentAlert,
    },
    DiagnosticAppend {
        event: AgentDiagnosticEvent,
    },
    McpStatus {
        status: McpStatus,
    },
    TurnStarted {
        agent_id: String,
        mission_id: Option<String>,
        resume_thread_id: Option<String>,
    },
    TurnHeartbeat {
        agent_id: String,
        mission_id: Option<String>,
    },
    TurnStage {
        agent_id: String,
        mission_id: Option<String>,
        stage: String,
    },
    /// Free-form log line emitted during a turn (routed to diagnostics).
    TurnLog {
        agent_id: String,
        message: String,
    },
    /// An agent wrote to a file during its turn. Emitted by the runner when it
    /// detects tool_use(edit/write/bash) targeting a file path. Authoritative
    /// per-agent file attribution — used by the genome system instead of
    /// filesystem-level tracking. `mission_id` is carried explicitly to avoid
    /// a race with `TurnStarted` setting `agent.current_mission`; an
    /// out-of-order `FileWrite` would otherwise skip the mission-scoped
    /// accumulator and the genome reviewer would miss the file.
    FileWrite {
        agent_id: String,
        mission_id: Option<String>,
        path: std::path::PathBuf,
    },
    TokenCount {
        agent_id: String,
        mission_id: Option<String>,
        token_count: AgentTokenCount,
    },
    TurnFailed {
        agent_id: String,
        mission_id: Option<String>,
        thread_id: Option<String>,
        token_count: Option<AgentTokenCount>,
        message: String,
    },
    TurnCompleted {
        agent_id: String,
        mission_id: Option<String>,
        thread_id: Option<String>,
        token_count: Option<AgentTokenCount>,
        message: String,
    },
    EmitSignal {
        signal: crate::substrate::Signal,
    },
    /// Assert a claim into the substrate. Emits `ClaimViolation` signals on
    /// conflict; no retry is queued (callers of this variant are responsible
    /// for their own back-off strategy).
    AssertClaim {
        claim: crate::substrate::Claim,
    },
    /// Assert an assumption into the substrate. Infallible — assumptions
    /// don't form a lattice. Mirrors AssertClaim as framework plumbing with
    /// no v1 caller.
    AssertAssumption {
        assumption: crate::substrate::Assumption,
    },
    /// Mint-on-apply signal emission: the substrate assigns the id atomically
    /// during `apply()`. Used by the nit-mcp back-channel — external
    /// processes can't safely mint substrate ids because the counter is
    /// mutated only under the single-writer invariant.
    EmitSignalRequest {
        posted_by: String,
        kind: crate::substrate::SignalKind,
        target: crate::substrate::SignalTarget,
        #[serde(default)]
        payload: serde_json::Value,
        initial_strength: Option<f32>,
    },
    /// Mint-on-apply claim assertion. Honors `mood.claim_ttl_multiplier` the
    /// same way the `FileWrite` auto-claim does; conflicts emit
    /// `ClaimViolation` signals targeted at the requester.
    AssertClaimRequest {
        claimed_by: String,
        kind: crate::substrate::ClaimKind,
        target: crate::substrate::ClaimTarget,
        ttl_gens: u64,
        rationale: String,
    },
    /// Mint-on-apply assumption assertion. Infallible.
    AssertAssumptionRequest {
        posted_by: String,
        target: crate::substrate::AssumptionTarget,
        #[serde(default)]
        fact: serde_json::Value,
        ttl_gens: u64,
        rationale: String,
    },
    /// Manually set the system mood. Locks auto-transitions for
    /// `MOOD_OVERRIDE_LOCK_GENS` generations.
    SetMood {
        mood: crate::mood::Mood,
        source: String,
    },
    /// Emitted by the async backend-probe thread spawned during a
    /// cache-miss `init_agents`. The TUI renders a "loading models…"
    /// placeholder in Agent Ops until this lands; on apply it populates
    /// the matching `*_models` / `*_models_error` fields, clears the
    /// `*_models_loading` flag, and re-materializes per-model lanes.
    ///
    /// `metadata` carries the per-model context window / supported
    /// efforts / default effort maps the probe thread populated.
    /// Operator-observed gap before this field existed: after a cache
    /// nuke, the probe thread emitted models but no metadata; the
    /// roster showed model names with blank context-window sizes, and
    /// only a restart (which then hit the freshly written cache that
    /// triggers `populate_claude_model_metadata`) made sizes appear.
    /// `None` preserves the pre-field behaviour for any caller that
    /// hasn't been updated yet.
    BackendModelsLoaded {
        backend: BackendKind,
        models: Vec<String>,
        error: Option<String>,
        #[serde(default)]
        metadata: Option<BackendModelsMetadata>,
    },
}

/// Per-model metadata pushed by the async probe alongside the model
/// list. Replaces the matching maps on `AgentsState` for the backend
/// the event is for (Claude or Gemini). Empty maps are valid — they
/// mean "the probe completed but didn't populate metadata for this
/// backend" (e.g. Gemini, where we don't track effort tiers).
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct BackendModelsMetadata {
    pub effective_context_window_tokens: std::collections::HashMap<String, u32>,
    pub supported_efforts: std::collections::HashMap<String, Vec<String>>,
    pub default_effort: std::collections::HashMap<String, String>,
    pub selected_effort: std::collections::HashMap<String, String>,
}

/// Discriminates the two backends whose model probes run asynchronously.
/// Codex is omitted because its model list comes from a synchronously-read
/// JSON cache (`~/.codex/models_cache.json`), not a subprocess.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackendKind {
    Claude,
    Gemini,
}

impl AgentBusEvent {
    pub fn apply(&self, state: &mut AppState) {
        match self {
            AgentBusEvent::AgentUpsert { agent } => {
                upsert::upsert_agent(state, agent.clone());
            }
            AgentBusEvent::MissionUpsert { mission } => {
                upsert::upsert_mission(state, mission.clone());
            }
            AgentBusEvent::MessageAppend { message } => {
                apply_message_append(state, message);
            }
            AgentBusEvent::AlertAppend { alert } => {
                state.agents.alerts.push(alert.clone());
                state.agents.alert_selected = state
                    .agents
                    .alert_selected
                    .min(state.agents.alerts.len().saturating_sub(1));
            }
            AgentBusEvent::DiagnosticAppend { event } => {
                state.agents.diag_events.push(event.clone());
            }
            AgentBusEvent::McpStatus { status } => {
                state.agents.mcp = status.clone();
            }
            AgentBusEvent::TurnStarted {
                agent_id,
                mission_id,
                resume_thread_id: _,
            } => {
                turn_lifecycle::handle_turn_started(state, agent_id, mission_id);
            }
            AgentBusEvent::TurnHeartbeat {
                agent_id,
                mission_id,
            } => {
                turn_lifecycle::handle_turn_heartbeat(state, agent_id, mission_id);
            }
            AgentBusEvent::TurnStage {
                agent_id,
                mission_id,
                stage,
            } => {
                turn_lifecycle::handle_turn_stage(state, agent_id, mission_id, stage);
            }
            AgentBusEvent::FileWrite {
                agent_id,
                mission_id,
                path,
            } => {
                file_ops::handle_file_write(state, agent_id, mission_id, path);
            }
            AgentBusEvent::TurnLog { agent_id, message } => {
                turn_lifecycle::handle_turn_log(state, agent_id, message);
            }
            AgentBusEvent::TokenCount {
                agent_id,
                mission_id,
                token_count,
            } => {
                token_count::handle_token_count_event(
                    state,
                    agent_id,
                    mission_id.as_deref(),
                    token_count,
                );
            }
            AgentBusEvent::TurnFailed {
                agent_id,
                mission_id,
                thread_id,
                token_count,
                message,
            } => {
                turn_error::handle_turn_failed(
                    state,
                    agent_id,
                    mission_id,
                    thread_id,
                    token_count,
                    message,
                );
            }
            AgentBusEvent::TurnCompleted {
                agent_id,
                mission_id,
                thread_id,
                token_count,
                message,
            } => {
                turn_completion::handle_turn_completed(
                    state,
                    agent_id,
                    mission_id,
                    thread_id,
                    token_count,
                    message,
                );
            }
            AgentBusEvent::EmitSignal { signal } => {
                state.substrate.emit_signal(signal.clone());
            }
            AgentBusEvent::AssertClaim { claim } => {
                claims_signals::handle_assert_claim(state, claim);
            }
            AgentBusEvent::AssertAssumption { assumption } => {
                state.substrate.assert_assumption(assumption.clone());
            }
            AgentBusEvent::EmitSignalRequest {
                posted_by,
                kind,
                target,
                payload,
                initial_strength,
            } => {
                claims_signals::handle_emit_signal_request(
                    state,
                    posted_by,
                    *kind,
                    target,
                    payload,
                    *initial_strength,
                );
            }
            AgentBusEvent::AssertClaimRequest {
                claimed_by,
                kind,
                target,
                ttl_gens,
                rationale,
            } => {
                claims_signals::handle_assert_claim_request(
                    state, claimed_by, *kind, target, *ttl_gens, rationale,
                );
            }
            AgentBusEvent::AssertAssumptionRequest {
                posted_by,
                target,
                fact,
                ttl_gens,
                rationale,
            } => {
                claims_signals::handle_assert_assumption_request(
                    state, posted_by, target, fact, *ttl_gens, rationale,
                );
            }
            AgentBusEvent::SetMood { mood, source } => {
                mood_control::handle_set_mood(state, *mood, source);
            }
            AgentBusEvent::BackendModelsLoaded {
                backend,
                models,
                error,
                metadata,
            } => {
                apply_backend_models_loaded(state, *backend, models, error, metadata.as_ref());
            }
        }

        // Bumps `event_epoch` so the runner's vitals sampler counts this
        // event toward the ECG / criticality histogram.
        state.agents.note_event();
    }
}

fn apply_backend_models_loaded(
    state: &mut AppState,
    backend: BackendKind,
    models: &[String],
    error: &Option<String>,
    metadata: Option<&BackendModelsMetadata>,
) {
    let (display_name, kind) = match backend {
        BackendKind::Claude => {
            state.agents.claude_models = models.to_vec();
            state.agents.claude_models_error = error.clone();
            state.agents.claude_models_loading = false;
            // Replace per-model metadata maps in lock-step with the
            // model list. Pre-fix the async-probe path emitted only
            // models, leaving these maps empty until a restart hit
            // the freshly written cache (`populate_claude_model_metadata`).
            // The roster widget keys context-window sizes off these
            // maps, so an empty map meant blank size cells — exactly
            // the "sizes don't show until I quit and reopen" symptom.
            if let Some(meta) = metadata {
                state.agents.claude_effective_context_window_tokens =
                    meta.effective_context_window_tokens.clone();
                state.agents.claude_supported_efforts = meta.supported_efforts.clone();
                state.agents.claude_default_effort = meta.default_effort.clone();
                state.agents.claude_selected_effort = meta.selected_effort.clone();
            }
            ("Claude", AgentLaneKind::Claude)
        }
        BackendKind::Gemini => {
            state.agents.gemini_models = models.to_vec();
            state.agents.gemini_models_error = error.clone();
            state.agents.gemini_models_loading = false;
            // Gemini has no per-model effort/context map today, so
            // metadata is currently unused for this backend — the
            // field is reserved for symmetry with Claude.
            ("Gemini", AgentLaneKind::Gemini)
        }
    };

    // Drop the placeholder lanes (if any) and re-materialize one row per
    // discovered model. Mirrors `agents::sync_backend_model_lanes` in the
    // `nit` crate; replicated here so the apply path doesn't have to dip
    // back across the crate boundary at event time.
    state.agents.agents.retain(|lane| lane.kind != kind);
    for model_id in models {
        state.agents.agents.push(AgentLane {
            id: model_id.clone(),
            role: model_id.clone(),
            lane: display_name.into(),
            kind,
            status: AgentStatus::Idle,
            heartbeat_age_secs: 0,
            queue_len: 0,
            current_mission: None,
            last_message: String::new(),
            shadow: false,
        });
    }

    // Selection invariant: if the operator was selecting a stale placeholder
    // row that got retained-out, point them at the first lane that still
    // exists. Mirrors `restore_selection_after_expansion`.
    if let Some(selected) = state.agents.selected_agent.clone() {
        if !state.agents.agents.iter().any(|l| l.id == selected) {
            state.agents.selected_agent = state.agents.agents.first().map(|l| l.id.clone());
            state.agents.roster_selected = 0;
        }
    }
}

fn apply_message_append(state: &mut AppState, message: &AgentMessage) {
    update_provenance_for_message(state, message);
    state.agents.messages.push(message.clone());
    state.agents.console_scroll = CONSOLE_SCROLL_BOTTOM;
}

fn update_provenance_for_message(state: &mut AppState, message: &AgentMessage) {
    if let Some(mission_id) = message.mission_id.as_deref() {
        upsert::mark_mission_provenance_dirty(state, mission_id);
        let delta = helpers::estimate_codex_context_tokens(&message.text);
        let entry = state
            .agents
            .codex_estimated_tokens_used_by_mission
            .entry(mission_id.to_string())
            .or_insert(0);
        *entry = entry.saturating_add(delta);
        return;
    }
    if let Some(agent_id) = message.agent_id.as_deref() {
        upsert::mark_ad_hoc_provenance_dirty(state, agent_id);
    }
}

#[cfg(test)]
#[path = "../tests/agent_bus.rs"]
mod tests;
