//! Per-event side-effect pipeline shared by the single-pane runner and
//! the multipane runtime. Centralising the pipeline here is the only
//! practical way to keep the two surfaces converged after a multipane
//! regression that left swarms stuck post-planner because every step
//! after `event.apply(state)` was being skipped.
//!
//! Callers hand events in one at a time after `super::event_coalesce`
//! has dropped dominated heartbeats from the per-tick batch. The
//! pipeline ordering is load-bearing — see inline comments. The
//! `genome_worker` argument is `None` for multipane (no genome retries
//! in v1).
use nit_core::{AgentBusEvent, AgentChannel, AppState};

use super::dispatch::{
    apply_claude_event, apply_swarm_task_role, augment_dispatch_prompt_with_landscape,
    claude_session_context_not_found, clear_claude_session_context_for_agent,
    clear_codex_thread_context_for_agent, codex_thread_context_not_found, dispatch_agent_prompt,
    enqueue_claude_turn, enqueue_codex_turn, maybe_dispatch_next_queued_claude_turn,
    maybe_dispatch_next_queued_codex_turn,
};
use super::genome_retry::{
    dispatch_shadow_outcome, dispatch_turn_genome_evals, drain_pending_claim_retries,
    drain_pending_interventions,
};
use super::popup_keys::maybe_follow_swarm_artifact_in_popup;
use super::vitals_log::record_agent_bus_vitals;
use crate::claude_runner::ClaudeRunner;
use crate::codex_runner::CodexRunner;
use crate::compile_gate_worker::CompileGateWorker;
use crate::genome_worker::GenomeWorker;
use crate::intake::{self, IntakeResume};
use crate::shadow::ShadowRuntime;
use crate::swarm::{create_chat_clone, is_agent_busy, is_agent_family_busy, SwarmRuntime};
use crate::vitals::VitalsState;
use crate::widgets::agent_ops_view;

/// Whether the caller should mark the next render dirty after this
/// drain. Today every drain returns `redraw: true`; the field exists so
/// future variants (e.g. a no-op event class) can opt out without
/// changing the signature.
#[derive(Clone, Copy, Debug, Default)]
pub struct EventDrainOutcome {
    pub redraw: bool,
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn drain_codex_event(
    state: &mut AppState,
    vitals: &mut VitalsState,
    codex: &CodexRunner,
    claude: &ClaudeRunner,
    swarm: &mut SwarmRuntime,
    shadow: &mut ShadowRuntime,
    genome_worker: Option<&GenomeWorker>,
    compile_gate: Option<&CompileGateWorker>,
    event: AgentBusEvent,
) -> EventDrainOutcome {
    drain_event_inner(
        state,
        vitals,
        codex,
        claude,
        swarm,
        shadow,
        genome_worker,
        compile_gate,
        event,
        pending_codex_thread_clear,
        |state, event| event.apply(state),
        clear_codex_thread_context_for_agent,
    )
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn drain_claude_event(
    state: &mut AppState,
    vitals: &mut VitalsState,
    codex: &CodexRunner,
    claude: &ClaudeRunner,
    swarm: &mut SwarmRuntime,
    shadow: &mut ShadowRuntime,
    genome_worker: Option<&GenomeWorker>,
    compile_gate: Option<&CompileGateWorker>,
    event: AgentBusEvent,
) -> EventDrainOutcome {
    drain_event_inner(
        state,
        vitals,
        codex,
        claude,
        swarm,
        shadow,
        genome_worker,
        compile_gate,
        event,
        pending_claude_session_clear,
        apply_claude_event,
        clear_claude_session_context_for_agent,
    )
}

#[allow(clippy::too_many_arguments)]
fn drain_event_inner(
    state: &mut AppState,
    vitals: &mut VitalsState,
    codex: &CodexRunner,
    claude: &ClaudeRunner,
    swarm: &mut SwarmRuntime,
    shadow: &mut ShadowRuntime,
    genome_worker: Option<&GenomeWorker>,
    compile_gate: Option<&CompileGateWorker>,
    event: AgentBusEvent,
    pending_clear: impl FnOnce(&AgentBusEvent) -> Option<(String, Option<String>)>,
    apply_fn: impl FnOnce(&mut AppState, &AgentBusEvent),
    do_clear: impl FnOnce(&mut AppState, &str, Option<&str>),
) -> EventDrainOutcome {
    record_agent_bus_vitals(vitals, &event);
    let finished = matches!(
        event,
        AgentBusEvent::TurnCompleted { .. } | AgentBusEvent::TurnFailed { .. }
    );
    let pinned_popup_ref = snapshot_pinned_popup_ref(state, swarm);
    let pending = pending_clear(&event);

    apply_fn(state, &event);

    if let Some((agent_id, mission_id)) = pending {
        do_clear(state, agent_id.as_str(), mission_id.as_deref());
    }

    drain_pending_claim_retries(state, vitals, codex, claude);
    drain_pending_interventions(state, vitals, codex, claude);

    let swarm_outcome = swarm.handle_event_outcome(state, &event);
    maybe_follow_swarm_artifact_in_popup(state, swarm, swarm_outcome.artifact_focus.as_ref());
    for mut dispatch in swarm_outcome.dispatches {
        augment_dispatch_prompt_with_landscape(state, swarm, &mut dispatch);
        apply_swarm_task_role(state, &dispatch);
        dispatch_agent_prompt(
            state,
            vitals,
            Some(codex),
            Some(claude),
            dispatch.agent_id,
            Some(dispatch.mission_id),
            dispatch.prompt,
        );
    }

    drain_intake_outcome(state, vitals, codex, claude, &event);
    drain_shadow_outcome(state, vitals, codex, claude, shadow, &event);

    if finished {
        maybe_dispatch_next_queued_codex_turn(state, vitals, Some(codex));
        maybe_dispatch_next_queued_claude_turn(state, vitals, Some(claude));
        cleanup_chat_clone_for_finished_event(state, &event);
        if let Some(genome) = genome_worker {
            maybe_dispatch_genome_evals(state, genome, &event);
        }
        if let Some(gate) = compile_gate {
            maybe_dispatch_compile_check_on_event(state, gate, &event);
        }
    }

    re_resolve_pinned_popup_ref(state, swarm, pinned_popup_ref.as_ref());
    EventDrainOutcome { redraw: true }
}

fn snapshot_pinned_popup_ref(
    state: &AppState,
    swarm: &SwarmRuntime,
) -> Option<agent_ops_view::ArtifactsPopupRef> {
    if !state.agents.artifacts_popup_open {
        return None;
    }
    agent_ops_view::artifacts_popup_ref(state, swarm, state.agents.ops_viewport_width)
}

fn re_resolve_pinned_popup_ref(
    state: &mut AppState,
    swarm: &SwarmRuntime,
    pinned: Option<&agent_ops_view::ArtifactsPopupRef>,
) {
    let Some(pinned) = pinned else {
        return;
    };
    if let Some(idx) = agent_ops_view::artifacts_card_index_for_popup_ref(
        state,
        Some(swarm),
        state.agents.ops_viewport_width,
        pinned,
    ) {
        state.agents.artifacts_selected = idx;
    }
}

fn pending_codex_thread_clear(event: &AgentBusEvent) -> Option<(String, Option<String>)> {
    match event {
        AgentBusEvent::TurnFailed {
            agent_id,
            mission_id,
            message,
            ..
        } if codex_thread_context_not_found(message) => {
            Some((agent_id.clone(), mission_id.clone()))
        }
        _ => None,
    }
}

fn pending_claude_session_clear(event: &AgentBusEvent) -> Option<(String, Option<String>)> {
    match event {
        AgentBusEvent::TurnFailed {
            agent_id,
            mission_id,
            message,
            ..
        } if claude_session_context_not_found(message) => {
            Some((agent_id.clone(), mission_id.clone()))
        }
        _ => None,
    }
}

fn drain_shadow_outcome(
    state: &mut AppState,
    vitals: &mut VitalsState,
    codex: &CodexRunner,
    claude: &ClaudeRunner,
    shadow: &mut ShadowRuntime,
    event: &AgentBusEvent,
) {
    let shadow_outcome = shadow.handle_event_outcome(state, event);
    for dispatch in shadow_outcome.dispatches {
        dispatch_shadow_outcome(state, vitals, codex, claude, dispatch);
    }
}

/// On `TurnCompleted` / `TurnFailed` for an in-flight intake lane, parse
/// the JSON decision (or fall back to passthrough) and replay the
/// deferred operator dispatch with either the augmented or raw prompt.
/// Bails when `pending_intake` is already cleared (operator-driven abort
/// path) so we never re-fire a dispatch the operator just cancelled.
fn drain_intake_outcome(
    state: &mut AppState,
    vitals: &mut VitalsState,
    codex: &CodexRunner,
    claude: &ClaudeRunner,
    event: &AgentBusEvent,
) {
    let Some(resume) = intake::handle_event_outcome(state, event) else {
        return;
    };
    resume_intake_dispatch(state, vitals, codex, claude, resume);
}

fn resume_intake_dispatch(
    state: &mut AppState,
    vitals: &mut VitalsState,
    codex: &CodexRunner,
    claude: &ClaudeRunner,
    resume: IntakeResume,
) {
    if matches!(resume.channel, AgentChannel::Broadcast) {
        // Defensive: intake gate is `AgentChannel::Agent` only, but the
        // pending struct is permissive. A broadcast channel here means
        // someone changed the gate without updating this resume — fall
        // back to a single dispatch against `target_agent_id` rather
        // than fanning out unexpectedly.
    }
    let base_id = crate::swarm::resolve_base_agent_id(&resume.target_agent_id).to_string();
    let lane_kind = state
        .agents
        .agents
        .iter()
        .find(|lane| lane.id == base_id)
        .map(|lane| lane.kind);
    let is_claude = matches!(lane_kind, Some(nit_core::AgentLaneKind::Claude));
    let is_codex = matches!(lane_kind, Some(nit_core::AgentLaneKind::Codex));
    if !is_claude && !is_codex {
        return;
    }

    if resume.force_new && is_agent_family_busy(state, &base_id) {
        if let Some(clone_id) = create_chat_clone(state, &base_id) {
            apply_resume_prompt_idx(state, &clone_id, resume.prompt_msg_idx, is_claude);
            dispatch_agent_prompt(
                state,
                vitals,
                Some(codex),
                Some(claude),
                clone_id,
                resume.mission_id,
                resume.prompt,
            );
            maybe_dispatch_next_queued_codex_turn(state, vitals, Some(codex));
            maybe_dispatch_next_queued_claude_turn(state, vitals, Some(claude));
            return;
        }
    }

    if is_agent_busy(state, &base_id) {
        if is_claude {
            enqueue_claude_turn(
                state,
                vitals,
                Some(base_id),
                resume.mission_id,
                resume.prompt,
                Some(resume.prompt_msg_idx),
            );
        } else {
            enqueue_codex_turn(
                state,
                vitals,
                Some(base_id),
                resume.mission_id,
                resume.prompt,
                Some(resume.prompt_msg_idx),
            );
        }
        return;
    }

    apply_resume_prompt_idx(state, &base_id, resume.prompt_msg_idx, is_claude);
    dispatch_agent_prompt(
        state,
        vitals,
        Some(codex),
        Some(claude),
        base_id,
        resume.mission_id,
        resume.prompt,
    );
    maybe_dispatch_next_queued_codex_turn(state, vitals, Some(codex));
    maybe_dispatch_next_queued_claude_turn(state, vitals, Some(claude));
}

fn apply_resume_prompt_idx(state: &mut AppState, agent_id: &str, idx: usize, is_claude: bool) {
    if is_claude {
        state
            .agents
            .claude_turn_prompt_idx
            .insert(agent_id.to_string(), idx);
    } else {
        state
            .agents
            .codex_turn_prompt_idx
            .insert(agent_id.to_string(), idx);
    }
}

fn cleanup_chat_clone_for_finished_event(state: &mut AppState, event: &AgentBusEvent) {
    if let AgentBusEvent::TurnCompleted { agent_id, .. }
    | AgentBusEvent::TurnFailed { agent_id, .. } = event
    {
        crate::swarm::cleanup_idle_chat_clone(state, agent_id);
    }
}

// Failed-but-wrote runs still queue genome work: integrators routinely
// hit max-turns or exit non-zero after the real edits already landed,
// and skipping evals there silently disables the retry pipeline.
fn maybe_dispatch_genome_evals(state: &mut AppState, genome: &GenomeWorker, event: &AgentBusEvent) {
    match event {
        AgentBusEvent::TurnCompleted {
            agent_id,
            mission_id,
            ..
        } => {
            dispatch_turn_genome_evals(state, genome, agent_id, mission_id);
        }
        AgentBusEvent::TurnFailed {
            agent_id,
            mission_id,
            ..
        } if state
            .genome_turn_modified
            .get(agent_id)
            .is_some_and(|s| !s.is_empty()) =>
        {
            dispatch_turn_genome_evals(state, genome, agent_id, mission_id);
        }
        _ => {}
    }
}

// Compile-gate trigger: same shape as `maybe_dispatch_genome_evals`.
// Fires on integrate-role TurnCompleted / TurnFailed when the agent
// has at least one tracked file in `genome_turn_modified`. Failure
// turns are gated too because a writer that hit max_turns mid-edit
// still left a half-applied diff that needs to compile.
fn maybe_dispatch_compile_check_on_event(
    state: &AppState,
    gate: &CompileGateWorker,
    event: &AgentBusEvent,
) {
    let (agent_id, mission_id) = match event {
        AgentBusEvent::TurnCompleted {
            agent_id,
            mission_id,
            ..
        } => (agent_id, mission_id),
        AgentBusEvent::TurnFailed {
            agent_id,
            mission_id,
            ..
        } if state
            .genome_turn_modified
            .get(agent_id)
            .is_some_and(|s| !s.is_empty()) =>
        {
            (agent_id, mission_id)
        }
        _ => return,
    };
    super::compile_gate::maybe_dispatch_compile_check(state, gate, agent_id, mission_id);
}
