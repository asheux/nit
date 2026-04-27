use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Instant;

use nit_core::{AgentAlertSeverity, AgentBusEvent, AgentDiagnosticEvent, AgentStatus, AppState};

use crate::claude_runner::{ClaudeCommand, ClaudeRunner};
use crate::codex_runner::{CodexCommand, CodexRunner};
use crate::swarm::{is_agent_busy, SwarmDispatch};
use crate::vitals::VitalsState;

/// Resolve the working directory for a runner dispatch keyed on
/// `agent_id`. In multipane mode, returns the matching pane's `cwd`; in
/// every other mode (and as fallback for unknown ids) returns
/// `state.workspace_root`. The lookup runs at dispatch leaf time, not at
/// enqueue time — so a queued multipane prompt picks up the pane's
/// CURRENT cwd at dequeue, which is the semantics Phase 4 dir-search
/// relies on.
pub(crate) fn resolve_dispatch_cwd(state: &AppState, agent_id: &str) -> PathBuf {
    state
        .multipane
        .as_ref()
        .and_then(|mp| mp.panes.iter().find(|p| p.agent_id == agent_id))
        .map(|p| p.cwd.clone())
        .unwrap_or_else(|| state.workspace_root.clone())
}

fn remove_from_mission_map<V>(
    maps: &mut HashMap<String, HashMap<String, V>>,
    mission_id: &str,
    agent_id: &str,
) {
    if let Some(inner) = maps.get_mut(mission_id) {
        inner.remove(agent_id);
        if inner.is_empty() {
            maps.remove(mission_id);
        }
    }
}

pub(super) fn codex_thread_context_not_found(message: &str) -> bool {
    let lowered = message.to_ascii_lowercase();
    let mentions_thread = lowered.contains("thread_id")
        || lowered.contains("threadid")
        || lowered.contains("thread id");
    if !mentions_thread {
        return false;
    }
    if lowered.contains("session not found") {
        return true;
    }
    lowered.contains("not found") || lowered.contains("unknown session")
}

pub(super) fn clear_codex_thread_context_for_agent(
    state: &mut AppState,
    agent_id: &str,
    mission_id: Option<&str>,
) {
    if let Some(mission_id) = mission_id {
        remove_from_mission_map(
            &mut state.agents.codex_mission_thread_ids,
            mission_id,
            agent_id,
        );
        remove_from_mission_map(
            &mut state.agents.codex_mission_used_tokens,
            mission_id,
            agent_id,
        );
        remove_from_mission_map(
            &mut state.agents.codex_mission_context_remaining_pct,
            mission_id,
            agent_id,
        );
    } else {
        state.agents.codex_thread_ids.remove(agent_id);
        state.agents.codex_used_tokens.remove(agent_id);
        state.agents.codex_context_remaining_pct.remove(agent_id);
    }

    state.agents.diag_events.push(AgentDiagnosticEvent {
        severity: AgentAlertSeverity::Warn,
        source: "codex".into(),
        message: format!("[{agent_id}] cleared invalid thread context (session not found)"),
        at: super::timestamp_label(state),
    });
}

pub(super) fn dispatch_codex_prompt(
    state: &mut AppState,
    vitals: &mut VitalsState,
    codex: Option<&CodexRunner>,
    agent_id: String,
    mission_id: Option<String>,
    prompt: String,
) {
    if is_agent_busy(state, &agent_id) {
        enqueue_codex_turn(state, vitals, Some(agent_id), mission_id, prompt, None);
    } else {
        maybe_dispatch_codex_turn(
            state,
            vitals,
            codex,
            Some(agent_id),
            mission_id,
            prompt,
            true,
        );
    }
}

pub(super) fn enqueue_codex_turn(
    state: &mut AppState,
    vitals: &mut VitalsState,
    model: Option<String>,
    mission_id: Option<String>,
    prompt: String,
    prompt_msg_idx: Option<usize>,
) {
    let Some(model) = model else {
        return;
    };
    // This queue is only used for Codex lanes; if the selected agent isn't Codex, ignore.
    let is_codex = state
        .agents
        .agents
        .iter()
        .find(|lane| lane.id.as_str() == model.as_str())
        .is_some_and(|lane| lane.is_codex());
    if !is_codex {
        return;
    }

    // Increment queue_len only after all validation checks pass, right before
    // the turn is pushed so the two stay in sync.
    if let Some(agent) = state.agents.agents.iter_mut().find(|a| a.id == model) {
        let is_running = matches!(agent.status, AgentStatus::Running);
        agent.queue_len = agent.queue_len.saturating_add(1);
        agent.heartbeat_age_secs = 0;
        agent.last_message = "queued".into();
        if !is_running {
            agent.current_mission = mission_id.clone();
        }
        if !matches!(agent.status, AgentStatus::Running | AgentStatus::Error) {
            agent.status = AgentStatus::Waiting;
        }
    }

    state
        .agents
        .queued_codex_turns
        .push_back(nit_core::QueuedCodexTurn {
            agent_id: model,
            mission_id,
            prompt,
            prompt_msg_idx,
        });

    let now = Instant::now();
    state.agents.note_event();
    vitals.record_agent_event(now);
}

pub(super) fn maybe_dispatch_next_queued_codex_turn(
    state: &mut AppState,
    vitals: &mut VitalsState,
    codex: Option<&CodexRunner>,
) {
    let Some(codex) = codex else {
        // Runner gone -- drain orphaned queued turns so queue_len doesn't stick.
        drain_orphaned_codex_queue(state);
        return;
    };
    if state.agents.queued_codex_turns.is_empty() {
        return;
    }

    // Dispatch at most one queued turn per agent id (multiple agents can advance in parallel).
    let mut remaining = state.agents.queued_codex_turns.len();
    while remaining > 0 {
        remaining = remaining.saturating_sub(1);
        let Some(queued) = state.agents.queued_codex_turns.pop_front() else {
            break;
        };
        let model = queued.agent_id.clone();
        let mission_id = queued.mission_id.clone();

        // Defer when this agent already has an in-flight turn.
        if state.agents.active_turns.contains_key(&model) {
            state.agents.queued_codex_turns.push_back(queued);
            continue;
        }

        // Queue length was incremented when we queued; only dispatch if this is still a Codex lane.
        let is_codex = state
            .agents
            .agents
            .iter()
            .find(|lane| lane.id.as_str() == model.as_str())
            .is_some_and(|lane| lane.is_codex());
        if !is_codex {
            release_queued_slot(state, &model);
            continue;
        }

        // Restore the prompt -> response link from the queued turn so the
        // breather appears after the correct prompt in the chat view.
        if let Some(idx) = queued.prompt_msg_idx {
            state
                .agents
                .codex_turn_prompt_idx
                .insert(model.clone(), idx);
        }

        maybe_dispatch_codex_turn(
            state,
            vitals,
            Some(codex),
            Some(model),
            mission_id,
            queued.prompt,
            false,
        );
    }
}

pub(super) fn maybe_dispatch_codex_turn(
    state: &mut AppState,
    vitals: &mut VitalsState,
    codex: Option<&CodexRunner>,
    model: Option<String>,
    mission_id: Option<String>,
    prompt: String,
    count_new_turn: bool,
) {
    let Some(codex) = codex else {
        return;
    };
    let Some(model) = model else {
        return;
    };
    let is_codex = state
        .agents
        .agents
        .iter()
        .find(|lane| lane.id.as_str() == model.as_str())
        .is_some_and(|lane| lane.is_codex());
    if !is_codex {
        return;
    }

    // Ensure genome context is always included, even when called directly
    // (e.g. from @new clones, artifacts popup, or queued turn dequeue).
    let prompt = match build_genome_context(state, &model) {
        Some(ctx) => format!("{ctx}{prompt}"),
        None => prompt,
    };

    let resume_thread_id = if let Some(mission_id) = mission_id.as_deref() {
        state
            .agents
            .codex_mission_thread_ids
            .get(mission_id)
            .and_then(|threads| threads.get(&model))
            .cloned()
    } else {
        state.agents.codex_thread_ids.get(&model).cloned()
    };
    // Always persist Codex sessions so non-mission chat can resume context across prompts.
    let persist_session = true;

    // Best-effort context remaining percentage for the breather row.
    if let Some(max_tokens) = state
        .agents
        .codex_effective_context_window_tokens
        .get(&model)
        .copied()
    {
        let prompt_tokens_est = estimate_codex_context_tokens(&prompt);
        let baseline_used = if let Some(mission_id) = mission_id.as_deref() {
            state
                .agents
                .codex_mission_used_tokens
                .get(mission_id)
                .and_then(|m| m.get(&model))
                .copied()
        } else {
            state.agents.codex_used_tokens.get(&model).copied()
        };
        let used_tokens = if let Some(baseline) = baseline_used {
            baseline.saturating_add(prompt_tokens_est).min(max_tokens)
        } else if let Some(mission_id) = mission_id.as_deref() {
            estimate_codex_context_tokens_for_mission(state, mission_id).min(max_tokens)
        } else {
            prompt_tokens_est.min(max_tokens)
        };
        if let Some(mission_id) = mission_id.as_deref() {
            state
                .agents
                .codex_mission_used_tokens
                .entry(mission_id.to_string())
                .or_default()
                .insert(model.clone(), used_tokens);
        } else {
            state
                .agents
                .codex_used_tokens
                .insert(model.clone(), used_tokens);
        }
        let remaining = max_tokens.saturating_sub(used_tokens);
        // Round to nearest percent so small prompts on large context windows still show 100%.
        let denom = max_tokens.max(1) as u64;
        let pct =
            (((remaining as u64).saturating_mul(100)).saturating_add(denom / 2) / denom) as u8;
        if let Some(mission_id) = mission_id.as_deref() {
            state
                .agents
                .codex_mission_context_remaining_pct
                .entry(mission_id.to_string())
                .or_default()
                .insert(model.clone(), pct);
        } else {
            state
                .agents
                .codex_context_remaining_pct
                .insert(model.clone(), pct);
        }
    } else if let Some(mission_id) = mission_id.as_deref() {
        if let Some(map) = state
            .agents
            .codex_mission_context_remaining_pct
            .get_mut(mission_id)
        {
            map.remove(&model);
            if map.is_empty() {
                state
                    .agents
                    .codex_mission_context_remaining_pct
                    .remove(mission_id);
            }
        }
    } else {
        state.agents.codex_context_remaining_pct.remove(&model);
    }

    // Immediate UI feedback: mark the model as queued and show the loader/breather row.
    let Some(agent) = state.agents.agents.iter_mut().find(|a| a.id == model) else {
        return;
    };
    // The Codex runner may still be at its global parallel cap, so treat the turn as queued
    // until we receive `TurnStarted` from the backend. This keeps the roster HB column from
    // flagging queued turns as stalled (no heartbeats yet).
    //
    // Do NOT flip Running → Waiting: when a prior turn is still in flight for
    // this agent, overwriting status to Waiting causes the ROSTER to show
    // "WAITING" while the AGENT panel still shows live ELAP/HB/OUT counters
    // (because active_turns[agent] is unchanged). Preserve Running and Error.
    if !matches!(agent.status, AgentStatus::Running | AgentStatus::Error) {
        agent.status = AgentStatus::Waiting;
    }
    if count_new_turn {
        agent.queue_len = agent.queue_len.saturating_add(1).max(1);
    } else {
        agent.queue_len = agent.queue_len.max(1);
    }
    agent.heartbeat_age_secs = 0;
    agent.last_message = "queued".into();
    // Always reflect the active mission context (including clearing it for non-mission chat).
    agent.current_mission = mission_id.clone();

    let now = Instant::now();
    state.agents.active_turns.insert(
        model.clone(),
        nit_core::state::AgentTurnState {
            started_at: now,
            last_heartbeat_at: now,
            last_output_at: now,
            stage: Some("queued".into()),
        },
    );

    state.agents.mcp.last_error = None;
    state.agents.note_event();
    vitals.record_agent_event(now);

    let reasoning_effort = state
        .agents
        .codex_selected_reasoning_effort
        .get(&model)
        .cloned()
        .or_else(|| {
            state
                .agents
                .codex_default_reasoning_effort
                .get(&model)
                .cloned()
        })
        .unwrap_or_else(|| "medium".into());

    let read_only = crate::shadow::parse_shadow_lane_id(&model).is_some();
    let cwd = resolve_dispatch_cwd(state, &model);
    let ok = codex.send(CodexCommand::RunTurn {
        model: model.clone(),
        cwd,
        mission_id: mission_id.clone(),
        resume_thread_id,
        persist_session,
        reasoning_effort: Some(reasoning_effort),
        prompt,
        read_only,
    });
    if !ok {
        // Runner channel is dead -- clean up the optimistic state we just set.
        state.agents.active_turns.remove(&model);
        release_queued_slot(state, &model);
        state
            .agents
            .diag_events
            .push(nit_core::state::AgentDiagnosticEvent {
                severity: nit_core::state::AgentAlertSeverity::Warn,
                source: "codex".into(),
                message: format!("[{model}] Codex runner channel disconnected; turn dropped"),
                at: format!("t+{}", state.metrics.frame_count),
            });
    }
}

pub(super) fn estimate_codex_context_tokens(text: &str) -> u32 {
    // Fast heuristic: ~4 bytes per token for typical English/code mixtures.
    // This keeps the UI responsive and avoids bringing in a tokenizer dependency.
    if text.is_empty() {
        return 0;
    }
    let bytes = text.len() as u32;
    bytes.div_ceil(4)
}

pub(super) fn estimate_codex_context_tokens_for_mission(
    state: &mut AppState,
    mission_id: &str,
) -> u32 {
    if let Some(tokens) = state
        .agents
        .codex_estimated_tokens_used_by_mission
        .get(mission_id)
        .copied()
    {
        return tokens;
    }
    let tokens = state
        .agents
        .messages
        .iter()
        .filter(|msg| msg.mission_id.as_deref() == Some(mission_id))
        .fold(0u32, |acc, msg| {
            acc.saturating_add(estimate_codex_context_tokens(&msg.text))
        });
    state
        .agents
        .codex_estimated_tokens_used_by_mission
        .insert(mission_id.to_string(), tokens);
    tokens
}

// Claude dispatch — mirrors the Codex pipeline above.

pub(super) fn dispatch_claude_prompt(
    state: &mut AppState,
    vitals: &mut VitalsState,
    claude: Option<&ClaudeRunner>,
    agent_id: String,
    mission_id: Option<String>,
    prompt: String,
) {
    if is_agent_busy(state, &agent_id) {
        enqueue_claude_turn(state, vitals, Some(agent_id), mission_id, prompt, None);
    } else {
        maybe_dispatch_claude_turn(
            state,
            vitals,
            claude,
            Some(agent_id),
            mission_id,
            prompt,
            true,
        );
    }
}

pub(super) fn enqueue_claude_turn(
    state: &mut AppState,
    vitals: &mut VitalsState,
    model: Option<String>,
    mission_id: Option<String>,
    prompt: String,
    prompt_msg_idx: Option<usize>,
) {
    let Some(model) = model else {
        return;
    };
    let is_claude = state
        .agents
        .agents
        .iter()
        .find(|lane| lane.id.as_str() == model.as_str())
        .is_some_and(|lane| lane.is_claude());
    if !is_claude {
        return;
    }

    // Increment queue_len only after all validation checks pass, right before
    // the turn is pushed so the two stay in sync.
    if let Some(agent) = state.agents.agents.iter_mut().find(|a| a.id == model) {
        let is_running = matches!(agent.status, AgentStatus::Running);
        agent.queue_len = agent.queue_len.saturating_add(1);
        agent.heartbeat_age_secs = 0;
        agent.last_message = "queued".into();
        if !is_running {
            agent.current_mission = mission_id.clone();
        }
        if !matches!(agent.status, AgentStatus::Running | AgentStatus::Error) {
            agent.status = AgentStatus::Waiting;
        }
    }

    state
        .agents
        .queued_claude_turns
        .push_back(nit_core::QueuedClaudeTurn {
            agent_id: model,
            mission_id,
            prompt,
            prompt_msg_idx,
        });

    let now = Instant::now();
    state.agents.note_event();
    vitals.record_agent_event(now);
}

pub(super) fn maybe_dispatch_next_queued_claude_turn(
    state: &mut AppState,
    vitals: &mut VitalsState,
    claude: Option<&ClaudeRunner>,
) {
    let Some(claude) = claude else {
        // Runner gone -- drain orphaned queued turns so queue_len doesn't stick.
        drain_orphaned_claude_queue(state);
        return;
    };
    if state.agents.queued_claude_turns.is_empty() {
        return;
    }

    let mut remaining = state.agents.queued_claude_turns.len();
    while remaining > 0 {
        remaining = remaining.saturating_sub(1);
        let Some(queued) = state.agents.queued_claude_turns.pop_front() else {
            break;
        };
        let model = queued.agent_id.clone();
        let mission_id = queued.mission_id.clone();

        if state.agents.active_turns.contains_key(&model) {
            state.agents.queued_claude_turns.push_back(queued);
            continue;
        }

        let is_claude = state
            .agents
            .agents
            .iter()
            .find(|lane| lane.id.as_str() == model.as_str())
            .is_some_and(|lane| lane.is_claude());
        if !is_claude {
            release_queued_slot(state, &model);
            continue;
        }

        if let Some(idx) = queued.prompt_msg_idx {
            state
                .agents
                .claude_turn_prompt_idx
                .insert(model.clone(), idx);
        }

        maybe_dispatch_claude_turn(
            state,
            vitals,
            Some(claude),
            Some(model),
            mission_id,
            queued.prompt,
            false,
        );
    }
}

// Decrement an agent's queue_len after a queued turn has been dropped or
// dequeued, flipping Waiting → Idle when the queue empties. Separated so the
// drain paths stay identical across Codex and Claude.
fn release_queued_slot(state: &mut AppState, agent_id: &str) {
    let Some(agent) = state.agents.agents.iter_mut().find(|a| a.id == agent_id) else {
        return;
    };
    agent.queue_len = agent.queue_len.saturating_sub(1);
    if agent.queue_len == 0 && matches!(agent.status, AgentStatus::Waiting) {
        agent.status = AgentStatus::Idle;
    }
}

// Drain all queued turns when the runner is unavailable so queue_len doesn't
// leave lanes stuck in "Waiting" forever.
fn drain_orphaned_codex_queue(state: &mut AppState) {
    while let Some(queued) = state.agents.queued_codex_turns.pop_front() {
        release_queued_slot(state, &queued.agent_id);
    }
}

fn drain_orphaned_claude_queue(state: &mut AppState) {
    while let Some(queued) = state.agents.queued_claude_turns.pop_front() {
        release_queued_slot(state, &queued.agent_id);
    }
}

pub(super) fn maybe_dispatch_claude_turn(
    state: &mut AppState,
    vitals: &mut VitalsState,
    claude: Option<&ClaudeRunner>,
    model: Option<String>,
    mission_id: Option<String>,
    prompt: String,
    count_new_turn: bool,
) {
    let Some(claude) = claude else {
        return;
    };
    let Some(model) = model else {
        return;
    };
    let is_claude = state
        .agents
        .agents
        .iter()
        .find(|lane| lane.id.as_str() == model.as_str())
        .is_some_and(|lane| lane.is_claude());
    if !is_claude {
        return;
    }

    // Ensure genome context is always included, even when called directly
    // (e.g. from @new clones, artifacts popup, or queued turn dequeue).
    let prompt = match build_genome_context(state, &model) {
        Some(ctx) => format!("{ctx}{prompt}"),
        None => prompt,
    };

    let resume_session_id = if let Some(mission_id) = mission_id.as_deref() {
        state
            .agents
            .claude_mission_session_ids
            .get(mission_id)
            .and_then(|sessions| sessions.get(&model))
            .cloned()
    } else {
        state.agents.claude_session_ids.get(&model).cloned()
    };
    let persist_session = true;

    // Best-effort context remaining percentage for the breather row.
    if let Some(max_tokens) = state
        .agents
        .claude_effective_context_window_tokens
        .get(&model)
        .copied()
    {
        let prompt_tokens_est = estimate_codex_context_tokens(&prompt);
        let baseline_used = if let Some(mission_id) = mission_id.as_deref() {
            state
                .agents
                .claude_mission_used_tokens
                .get(mission_id)
                .and_then(|m| m.get(&model))
                .copied()
        } else {
            state.agents.claude_used_tokens.get(&model).copied()
        };
        let used_tokens = if let Some(baseline) = baseline_used {
            baseline.saturating_add(prompt_tokens_est).min(max_tokens)
        } else if let Some(mission_id) = mission_id.as_deref() {
            estimate_claude_context_tokens_for_mission(state, mission_id).min(max_tokens)
        } else {
            prompt_tokens_est.min(max_tokens)
        };
        if let Some(mission_id) = mission_id.as_deref() {
            state
                .agents
                .claude_mission_used_tokens
                .entry(mission_id.to_string())
                .or_default()
                .insert(model.clone(), used_tokens);
        } else {
            state
                .agents
                .claude_used_tokens
                .insert(model.clone(), used_tokens);
        }
        let remaining = max_tokens.saturating_sub(used_tokens);
        let denom = max_tokens.max(1) as u64;
        let pct =
            (((remaining as u64).saturating_mul(100)).saturating_add(denom / 2) / denom) as u8;
        if let Some(mission_id) = mission_id.as_deref() {
            state
                .agents
                .claude_mission_context_remaining_pct
                .entry(mission_id.to_string())
                .or_default()
                .insert(model.clone(), pct);
        } else {
            state
                .agents
                .claude_context_remaining_pct
                .insert(model.clone(), pct);
        }
    } else if let Some(mission_id) = mission_id.as_deref() {
        if let Some(map) = state
            .agents
            .claude_mission_context_remaining_pct
            .get_mut(mission_id)
        {
            map.remove(&model);
            if map.is_empty() {
                state
                    .agents
                    .claude_mission_context_remaining_pct
                    .remove(mission_id);
            }
        }
    } else {
        state.agents.claude_context_remaining_pct.remove(&model);
    }

    let Some(agent) = state.agents.agents.iter_mut().find(|a| a.id == model) else {
        return;
    };
    // Preserve Running/Error — see the matching guard in maybe_dispatch_codex_turn
    // for why flipping Running → Waiting desyncs the ROSTER vs AGENT panel.
    if !matches!(agent.status, AgentStatus::Running | AgentStatus::Error) {
        agent.status = AgentStatus::Waiting;
    }
    if count_new_turn {
        agent.queue_len = agent.queue_len.saturating_add(1).max(1);
    } else {
        agent.queue_len = agent.queue_len.max(1);
    }
    agent.heartbeat_age_secs = 0;
    agent.last_message = "queued".into();
    agent.current_mission = mission_id.clone();

    let now = Instant::now();
    state.agents.active_turns.insert(
        model.clone(),
        nit_core::state::AgentTurnState {
            started_at: now,
            last_heartbeat_at: now,
            last_output_at: now,
            stage: Some("queued".into()),
        },
    );

    state.agents.mcp.last_error = None;
    state.agents.note_event();
    vitals.record_agent_event(now);

    let effort = state
        .agents
        .claude_selected_effort
        .get(&model)
        .cloned()
        .or_else(|| state.agents.claude_default_effort.get(&model).cloned())
        .unwrap_or_else(|| "high".into());

    let read_only = crate::shadow::parse_shadow_lane_id(&model).is_some();
    // Role-aware turn budget:
    // - Integrators run real verify loops (clippy → test → fmt → fix → re-check)
    //   on top of the write work and need the largest envelope.
    // - Every other swarm clone is read-only but still performs deep recon
    //   across the scope — reads, greps, and evidence collection add up. The
    //   plain-chat default of 50 runs out mid-recon on non-trivial scopes
    //   (observed: proposer hitting error_max_turns after 51 turns while
    //   analysing a 13k-line module).
    // - Non-clone agents (regular chat lanes) keep the plain-chat default.
    //
    // Role is set by `apply_swarm_task_role` before dispatch; treat any
    // swarm-clone agent with a non-empty role as at least support-tier so
    // novel planner-generated roles (recon, design, plan, genome-reviewer,
    // etc.) also get the larger envelope without having to enumerate them.
    let max_turns = state
        .agents
        .agents
        .iter()
        .find(|a| a.id == model)
        .and_then(|a| {
            let role = a.role.to_ascii_lowercase();
            match role.as_str() {
                "integrate" | "integrator" | "integration" | "code" | "coding" | "implement"
                | "refactor" | "refactoring" | "fix" | "fixer" | "bugfix" => {
                    Some(crate::claude_runner::INTEGRATOR_MAX_TURNS)
                }
                _ => {
                    // Fallback: any other non-empty role on a swarm clone is
                    // a support/research role that deserves the bigger budget.
                    let is_clone = crate::swarm::is_any_clone_agent_id(&a.id);
                    let has_role = !a.role.trim().is_empty();
                    if is_clone && has_role {
                        Some(crate::claude_runner::SWARM_SUPPORT_MAX_TURNS)
                    } else {
                        None
                    }
                }
            }
        });
    let cwd = resolve_dispatch_cwd(state, &model);
    let ok = claude.send(ClaudeCommand::RunTurn {
        model: model.clone(),
        cwd,
        mission_id: mission_id.clone(),
        resume_session_id,
        persist_session,
        effort: Some(effort),
        prompt,
        read_only,
        max_turns,
    });
    if !ok {
        // Runner channel is dead -- clean up the optimistic state we just set.
        state.agents.active_turns.remove(&model);
        release_queued_slot(state, &model);
        state
            .agents
            .diag_events
            .push(nit_core::state::AgentDiagnosticEvent {
                severity: nit_core::state::AgentAlertSeverity::Warn,
                source: "claude".into(),
                message: format!("[{model}] Claude runner channel disconnected; turn dropped"),
                at: format!("t+{}", state.metrics.frame_count),
            });
    }
}

pub(super) fn estimate_claude_context_tokens_for_mission(
    state: &mut AppState,
    mission_id: &str,
) -> u32 {
    if let Some(tokens) = state
        .agents
        .claude_estimated_tokens_used_by_mission
        .get(mission_id)
        .copied()
    {
        return tokens;
    }
    let tokens = state
        .agents
        .messages
        .iter()
        .filter(|msg| msg.mission_id.as_deref() == Some(mission_id))
        .fold(0u32, |acc, msg| {
            acc.saturating_add(estimate_codex_context_tokens(&msg.text))
        });
    state
        .agents
        .claude_estimated_tokens_used_by_mission
        .insert(mission_id.to_string(), tokens);
    tokens
}

// Build a GENOME LANDSCAPE section for propose/integrate/judge roles so they
// cite concrete numbers (tier, consistency, density) instead of trading
// surface-level opinions. Returns `None` when no scope files have reports yet.
pub(crate) fn build_propose_genome_landscape(
    state: &AppState,
    scope_files: &[String],
    role: Option<&str>,
) -> Option<String> {
    use nit_core::GenomeTier;
    let workspace = state.workspace_root.as_path();
    let mut rows: Vec<(String, &nit_core::GenomeReport)> = Vec::new();
    for rel in scope_files {
        let abs = workspace.join(rel);
        if let Some(report) = state.genome_reports.get(&abs) {
            rows.push((rel.clone(), report));
        }
    }
    if rows.is_empty() {
        return None;
    }
    // Sort worst-first: lowest tier, then lowest consistency.
    rows.sort_by(|a, b| {
        let ta = a.1.tier as u8;
        let tb = b.1.tier as u8;
        ta.cmp(&tb).then(
            a.1.cross_encoder_consistency
                .partial_cmp(&b.1.cross_encoder_consistency)
                .unwrap_or(std::cmp::Ordering::Equal),
        )
    });

    let mut out = String::new();
    let (header, framing) = match role {
        Some("integrate") => (
            "\n## GENOME LANDSCAPE (current state — target these metrics with your edits)\n",
            "Lower tier or consistency means worse structural quality. Cross-check proposer \
             recommendations against these numbers. When you edit a file, target a concrete \
             encoder move (e.g. raise structural density, unlock a parsimony-capped tier, \
             eliminate a zero-entropy block). Report per-file before/after expectations in \
             your final message so reviewers and the genome gate can verify direction.\n\n",
        ),
        Some("judge") => (
            "\n## GENOME LANDSCAPE (current state — weigh proposals against this)\n",
            "Lower tier or consistency means worse structural quality. Prefer proposals that \
             target the worst-scoring files first and cite concrete metric moves. Reject \
             proposals that recommend changes uncorrelated with the landscape (e.g. \
             cosmetic renames on already-tier-IV files while tier-I/II files go untouched).\n\n",
        ),
        Some("review") => (
            "\n## GENOME LANDSCAPE (current state — cite these metrics when flagging issues)\n",
            "Lower tier or consistency means worse structural quality. Ground every critique \
             in a specific encoder (complexity_field, ast_structure, structural, token_spectrum): \
             `parse_config complexity 12, complexity_field target \u{2264}8` is a useful review \
             note; \"this function is too complex\" isn't. Prefer flags tied to the lowest-tier, \
             parsimony-bloated, or low-density files in the landscape below \u{2014} cosmetic \
             nits on already-healthy files aren't useful at this stage. Low-consistency files \
             (cross-encoder consistency < 0.30) usually indicate one encoder is dragging the \
             overall tier down; call out which.\n\n",
        ),
        _ => (
            "\n## GENOME LANDSCAPE (current state — use this to ground your proposal)\n",
            "Lower tier or consistency means worse structural quality. Propose fixes that move \
             lowest-scoring files up first; cite the metric (tier, consistency, parsimony) when \
             you recommend a refactor so the integrator knows what the change is supposed to move.\n\n",
        ),
    };
    out.push_str(header);
    out.push_str(framing);
    // THRESHOLDS BREACHED — files that the role contract requires an
    // explicit structural recommendation for. Keeps proposers from silently
    // skipping mega-files when the rest of the landscape looks healthy.
    let workspace = state.workspace_root.as_path();
    let mut breached: Vec<(String, &nit_core::GenomeReport, u64, Vec<&'static str>)> = Vec::new();
    for (path, report) in &rows {
        let abs = workspace.join(path);
        let line_count = std::fs::read_to_string(&abs)
            .map(|s| s.lines().count() as u64)
            .unwrap_or(0);
        let mut reasons: Vec<&'static str> = Vec::new();
        if line_count > 2000 {
            reasons.push(">2000 lines");
        }
        if report.tier <= GenomeTier::Oscillator {
            reasons.push("tier I/II");
        }
        if report.parsimony.bloat_detected {
            reasons.push("parsimony-bloat");
        }
        if report.cross_encoder_consistency < 0.30 {
            reasons.push("low consistency");
        }
        if !reasons.is_empty() {
            breached.push((path.clone(), *report, line_count, reasons));
        }
    }
    if !breached.is_empty() {
        out.push_str("\n## THRESHOLDS BREACHED (MANDATORY recommendations)\n");
        out.push_str(
            "Every file below MUST appear in your proposal with a concrete structural action \
             (split into named submodules, consolidate bloat, replace zero-entropy block with \
             a templated helper, etc.). Omitting a listed file is a proposal failure.\n",
        );
        for (path, report, lines, reasons) in &breached {
            out.push_str(&format!(
                "- {path}: {} lines, tier {}, consistency {:.2} — {}\n",
                lines,
                report.tier.numeral(),
                report.cross_encoder_consistency,
                reasons.join(" + "),
            ));
        }
        out.push('\n');
    }

    out.push_str("Per-file (worst first):\n");
    for (path, report) in &rows {
        let gen_sum: u32 = report
            .encoder_scores
            .iter()
            .map(|s| s.generations_survived)
            .sum();
        let bloat_tag = if report.parsimony.bloat_detected {
            " [parsimony-bloat]"
        } else {
            ""
        };
        out.push_str(&format!(
            "- {path}: tier {} ({}), consistency {:.2}, gen-sum {gen_sum}{bloat_tag}\n",
            report.tier.numeral(),
            report.tier,
            report.cross_encoder_consistency,
        ));
    }

    // Workspace-wide tier counts for context.
    let mut counts = [0usize; 5];
    for report in state.genome_reports.values() {
        let idx = report.tier as usize;
        if idx < counts.len() {
            counts[idx] = counts[idx].saturating_add(1);
        }
    }
    let total: usize = counts.iter().sum();
    if total > 0 {
        out.push_str(&format!(
            "\nWorkspace tier distribution ({total} files): I={} II={} III={} IV={} V={}\n",
            counts[GenomeTier::StillLife as usize],
            counts[GenomeTier::Oscillator as usize],
            counts[GenomeTier::Spaceship as usize],
            counts[GenomeTier::Methuselah as usize],
            counts[GenomeTier::Replicator as usize],
        ));
    }

    out.push_str(
        "\nRECOMMENDATION FORMAT: when proposing a change, name the target metric and expected direction — \
         e.g. \"split swarm.rs: structural density 0.13 → aim ≥0.25 by per-concern submodules\", \
         \"inline vitals.rs trivial predicates: parsimony-bloat cap → tier IV unlock\".\n",
    );
    Some(out)
}

// Append the genome landscape to propose/integrate/judge/review dispatches.
// Hangs off `task_role` so it applies to every template (parallel / lab /
// bulk). Review is included because reviewer role contracts already require
// citing encoders for each flagged issue — without landscape numbers the
// reviewer can only gesture at the encoder names, not ground critiques in
// real per-file targets.
pub(crate) fn augment_dispatch_prompt_with_landscape(
    state: &AppState,
    swarm: &crate::swarm::SwarmRuntime,
    dispatch: &mut SwarmDispatch,
) {
    let role = dispatch.task_role.as_deref();
    let wants_landscape = matches!(
        role,
        Some("propose") | Some("integrate") | Some("judge") | Some("review")
    );
    if !wants_landscape {
        return;
    }
    let Some(scope_files) = swarm.scope_files_for_mission(&dispatch.mission_id) else {
        return;
    };
    let Some(section) = build_propose_genome_landscape(state, scope_files, role) else {
        return;
    };
    dispatch.prompt.push_str(&section);
}

// Mirror the swarm task role onto the clone's lane for the UI. Original roster
// agents keep their display name — only clones get their role overwritten.
pub(super) fn apply_swarm_task_role(state: &mut AppState, dispatch: &SwarmDispatch) {
    let Some(role) = dispatch.task_role.as_deref() else {
        return;
    };
    // Never overwrite the role of an original roster agent.
    if !crate::swarm::is_any_clone_agent_id(&dispatch.agent_id) {
        return;
    }
    let Some(agent) = state
        .agents
        .agents
        .iter_mut()
        .find(|a| a.id == dispatch.agent_id)
    else {
        return;
    };
    // Capitalise the role for display (e.g. "review" → "Review").
    let display = titlecase_role(role);
    agent.role = display;
}

fn titlecase_role(role: &str) -> String {
    let mut chars = role.chars();
    match chars.next() {
        Some(first) => {
            let mut out = first.to_uppercase().to_string();
            out.extend(chars);
            out
        }
        None => String::new(),
    }
}

fn append_file_genome_context(
    ctx: &mut String,
    file_path: &std::path::Path,
    report: &nit_core::GenomeReport,
) {
    ctx.push_str(&format!("\n--- {} ---\n", file_path.display()));
    ctx.push_str(&format!(
        "Tier: {} ({}), quality: {}, consistency: {:.2}\n",
        report.tier.numeral(),
        report.tier.name(),
        report.quality_level(),
        report.cross_encoder_consistency,
    ));

    if report.parsimony.bloat_detected {
        ctx.push_str(&format!(
            "  ⚠ PARSIMONY BLOAT — tier capped at IV. {} fns, avg {:.1} lines, \
             {:.0}% tiny (<=5 lines), {:.0}% comments. Consolidate over-split \
             functions and remove unnecessary comments. Do NOT add more structure.\n",
            report.parsimony.fn_count,
            report.parsimony.avg_fn_body_lines,
            report.parsimony.tiny_fn_fraction * 100.0,
            report.parsimony.comment_ratio * 100.0,
        ));
    }

    // Show all encoder scores (reports only contain the 4 quality encoders).
    ctx.push_str("  Encoders:\n");
    let scores = &report.encoder_scores;
    let mean_gen: f32 = if scores.is_empty() {
        0.0
    } else {
        scores
            .iter()
            .map(|s| s.generations_survived as f32)
            .sum::<f32>()
            / scores.len() as f32
    };
    for score in scores {
        let gen = score.generations_survived;
        let outlier = if mean_gen > 0.0 && (gen as f32) < mean_gen * 0.5 {
            " ← OUTLIER (dragging consistency down)"
        } else {
            ""
        };
        ctx.push_str(&format!(
            "    {}: density={:.2}, components={}, generations={}, growth={}{}\n",
            score.encoder.label(),
            score.density,
            score.components,
            gen,
            score.growth_class.label(),
            outlier,
        ));
    }

    // Diagnose low consistency: tell the agent exactly what to focus on.
    if report.cross_encoder_consistency < 0.50 && !scores.is_empty() {
        let mut sorted: Vec<_> = scores
            .iter()
            .map(|s| (s.encoder.label(), s.generations_survived))
            .collect();
        sorted.sort_by_key(|(_, g)| std::cmp::Reverse(*g));
        let best = sorted
            .first()
            .map(|(l, g)| format!("{l}={g}"))
            .unwrap_or_default();
        let worst = sorted
            .last()
            .map(|(l, g)| format!("{l}={g}"))
            .unwrap_or_default();
        ctx.push_str(&format!(
            "  ⚠ Low consistency ({:.2}): encoders disagree. Best: {best}, Worst: {worst}.\n\
             Focus improvements on the weakest encoder — that's the fastest path to better quality.\n",
            report.cross_encoder_consistency,
        ));
    }

    if !report.recommendations.is_empty() {
        ctx.push_str("  Recommendations:\n");
        for rec in &report.recommendations {
            ctx.push_str(&format!("  - {}\n", rec.message));
        }
    }
}

// Build the genome context string prepended to a dispatched prompt. Scoped to
// files this specific agent touched so agents don't see each other's turns.
fn build_genome_context(state: &AppState, agent_id: &str) -> Option<String> {
    if !state.settings.genome.genome_context_enabled {
        return None;
    }

    let mut ctx = String::from("\n[genome context]\n");
    let mut has_content = false;

    // Include only files modified by THIS agent during its turn.
    let agent_modified = state.genome_turn_modified.get(agent_id);
    if let Some(modified) = agent_modified {
        if !modified.is_empty() {
            ctx.push_str(&format!("Files modified this turn: {}\n", modified.len()));
        }
        let mut sorted_paths: Vec<_> = modified.iter().collect();
        sorted_paths.sort();
        for file_path in &sorted_paths {
            if let Some(report) = state.genome_reports.get(*file_path) {
                let is_new = !state.genome_baselines.contains_key(*file_path);
                if is_new {
                    ctx.push_str("[NEW FILE] ");
                }
                append_file_genome_context(&mut ctx, file_path, report);
                // Show delta against baseline if available.
                if let Some(base) = state.genome_baselines.get(*file_path) {
                    let gen_base: i32 = base
                        .encoder_scores
                        .iter()
                        .map(|s| s.generations_survived as i32)
                        .sum();
                    let gen_now: i32 = report
                        .encoder_scores
                        .iter()
                        .map(|s| s.generations_survived as i32)
                        .sum();
                    let label = if report.tier > base.tier || gen_now > gen_base {
                        "IMPROVED"
                    } else if report.tier < base.tier || gen_now < gen_base {
                        "DEGRADED"
                    } else {
                        "UNCHANGED"
                    };
                    ctx.push_str(&format!(
                        "  Delta: {label} ({:+} generations vs baseline)\n",
                        gen_now - gen_base
                    ));
                }
                has_content = true;
            }
        }
    }

    // Always include the editor buffer if this agent hasn't modified it.
    let agent_modified_set = agent_modified.cloned().unwrap_or_default();
    if let Some(file_path) = state.editor_buffer().path() {
        if !agent_modified_set.contains(file_path) {
            if let Some(report) = state.genome_reports.get(file_path) {
                ctx.push_str("\n[active buffer]\n");
                append_file_genome_context(&mut ctx, file_path, report);
                has_content = true;
            }
        }
    }

    if !has_content {
        return None;
    }

    // Include real-time shadow evaluation summary if available.
    if !state.genome_shadow_evals.is_empty() {
        ctx.push_str("\n[shadow evaluator — real-time quality]\n");
        let mut sorted: Vec<_> = state.genome_shadow_evals.iter().collect();
        sorted.sort_by_key(|(p, _)| (*p).clone());
        for (path, eval) in &sorted {
            let file_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("?");
            let new_tag = if eval.is_new_file { " [NEW]" } else { "" };
            ctx.push_str(&format!(
                "  {file_name}{new_tag}: {} {} (tier {}, c={:.2})\n",
                eval.quality,
                eval.delta_label,
                eval.tier.numeral(),
                eval.consistency,
            ));
        }
    }

    if let Some(diff) = &state.last_genome_diff {
        ctx.push_str(&format!("\n{diff}\n"));
    }

    ctx.push_str(crate::codex_runner::EVALUATE_GENOME_TOOL_DESCRIPTION);
    ctx.push('\n');
    ctx.push_str(nit_core::GENOME_AGENT_INSTRUCTIONS);
    ctx.push('\n');
    ctx.push_str("[/genome context]\n\n");
    Some(ctx)
}

pub(crate) fn dispatch_agent_prompt(
    state: &mut AppState,
    vitals: &mut VitalsState,
    codex: Option<&CodexRunner>,
    claude: Option<&ClaudeRunner>,
    agent_id: String,
    mission_id: Option<String>,
    prompt: String,
) {
    // NOTE: genome context is prepended inside maybe_dispatch_codex_turn /
    // maybe_dispatch_claude_turn (and their queue-dequeue paths) so that every
    // code path that actually sends a prompt gets it exactly once.  Do NOT add
    // it here — that would duplicate the context.

    let lane_kind = state
        .agents
        .agents
        .iter()
        .find(|lane| lane.id.as_str() == agent_id.as_str())
        .map(|lane| lane.kind);
    match lane_kind {
        Some(nit_core::AgentLaneKind::Claude) => {
            dispatch_claude_prompt(state, vitals, claude, agent_id, mission_id, prompt);
        }
        Some(nit_core::AgentLaneKind::Codex) => {
            dispatch_codex_prompt(state, vitals, codex, agent_id, mission_id, prompt);
        }
        _ => {
            // Unknown or unsupported backend -- try Codex as fallback.
            dispatch_codex_prompt(state, vitals, codex, agent_id, mission_id, prompt);
        }
    }
}

// Apply a Claude bus event, delegating state mutation to the generic
// `event.apply()` and then capturing the Claude session id (the moral
// equivalent of Codex thread id) from `TurnCompleted`.
pub(super) fn apply_claude_event(state: &mut AppState, event: &AgentBusEvent) {
    // First, apply the generic state mutation (status, messages, tokens, etc.).
    event.apply(state);

    // Then, store Claude session IDs from TurnCompleted events.
    if let AgentBusEvent::TurnCompleted {
        agent_id,
        mission_id,
        thread_id: Some(session_id),
        ..
    } = event
    {
        let is_claude = state
            .agents
            .agents
            .iter()
            .find(|lane| lane.id.as_str() == agent_id.as_str())
            .is_some_and(|lane| lane.is_claude());
        if is_claude {
            if let Some(mission_id) = mission_id.as_deref() {
                state
                    .agents
                    .claude_mission_session_ids
                    .entry(mission_id.to_string())
                    .or_default()
                    .insert(agent_id.clone(), session_id.clone());
            } else {
                state
                    .agents
                    .claude_session_ids
                    .insert(agent_id.clone(), session_id.clone());
            }
        }
    }
}

pub(super) fn claude_session_context_not_found(message: &str) -> bool {
    let lower = message.to_lowercase();
    lower.contains("session not found")
        || lower.contains("session_id")
        || lower.contains("invalid session")
}

pub(super) fn clear_claude_session_context_for_agent(
    state: &mut AppState,
    agent_id: &str,
    mission_id: Option<&str>,
) {
    if let Some(mission_id) = mission_id {
        if let Some(map) = state.agents.claude_mission_session_ids.get_mut(mission_id) {
            map.remove(agent_id);
        }
    } else {
        state.agents.claude_session_ids.remove(agent_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nit_core::{Buffer, MultipaneState, PaneSession};
    use std::path::PathBuf;

    fn fixture_state() -> AppState {
        AppState::new(
            PathBuf::from("/workspace"),
            Buffer::empty("scratch", None),
            Buffer::empty("notes", None),
        )
    }

    #[test]
    fn resolve_dispatch_cwd_falls_back_to_workspace_root_when_not_multipane() {
        let state = fixture_state();
        assert_eq!(
            resolve_dispatch_cwd(&state, "any-agent"),
            PathBuf::from("/workspace")
        );
    }

    #[test]
    fn resolve_dispatch_cwd_returns_pane_cwd_in_multipane() {
        let mut state = fixture_state();
        state.multipane = Some(MultipaneState {
            backend_agent_id: "claude-haiku-4-5".into(),
            panes: vec![
                PaneSession {
                    pane_id: 0,
                    agent_id: "claude-haiku-4-5#mp-pane-00".into(),
                    cwd: PathBuf::from("/pane0"),
                    ..PaneSession::default()
                },
                PaneSession {
                    pane_id: 1,
                    agent_id: "claude-haiku-4-5#mp-pane-01".into(),
                    cwd: PathBuf::from("/pane1"),
                    ..PaneSession::default()
                },
            ],
            focused: 0,
            grid_cols: 2,
            grid_rows: 1,
        });
        assert_eq!(
            resolve_dispatch_cwd(&state, "claude-haiku-4-5#mp-pane-00"),
            PathBuf::from("/pane0")
        );
        assert_eq!(
            resolve_dispatch_cwd(&state, "claude-haiku-4-5#mp-pane-01"),
            PathBuf::from("/pane1")
        );
    }

    #[test]
    fn resolve_dispatch_cwd_unknown_agent_falls_back() {
        let mut state = fixture_state();
        state.multipane = Some(MultipaneState {
            backend_agent_id: "claude-haiku-4-5".into(),
            panes: vec![PaneSession {
                pane_id: 0,
                agent_id: "claude-haiku-4-5#mp-pane-00".into(),
                cwd: PathBuf::from("/pane0"),
                ..PaneSession::default()
            }],
            focused: 0,
            grid_cols: 1,
            grid_rows: 1,
        });
        assert_eq!(
            resolve_dispatch_cwd(&state, "non-pane-agent"),
            PathBuf::from("/workspace")
        );
    }
}
