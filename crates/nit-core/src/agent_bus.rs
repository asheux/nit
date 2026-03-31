use crate::state::{
    AgentAlert, AgentAlertSeverity, AgentChannel, AgentDiagnosticEvent, AgentLane, AgentLaneKind,
    AgentMessage, AgentStatus, AgentTurnState, AppState, McpStatus, MissionRecord,
    CONSOLE_SCROLL_BOTTOM,
};
use std::time::Instant;

/// Resolve the backend source label for an agent (used in alerts and diagnostics).
fn backend_source_for_agent(state: &AppState, agent_id: &str) -> &'static str {
    state
        .agents
        .agents
        .iter()
        .find(|a| a.id == agent_id)
        .map(|a| match a.kind {
            AgentLaneKind::Claude => "claude",
            AgentLaneKind::Gemini => "gemini",
            AgentLaneKind::Mock => "local",
            AgentLaneKind::Codex | AgentLaneKind::Unknown => "codex",
        })
        .unwrap_or("codex")
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AgentTokenCount {
    pub total_tokens: u32,
    pub context_window: u32,
}

/// Event protocol for driving the Agent Station UI from an external runtime (Codex, Claude, etc.).
///
/// Transported as NDJSON over stdio or a socket; each variant maps to a single state mutation.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentBusEvent {
    /// Insert or update an agent lane in the roster.
    AgentUpsert { agent: AgentLane },
    /// Insert or update a mission record.
    MissionUpsert { mission: MissionRecord },
    /// Append a chat message to the console log.
    MessageAppend { message: AgentMessage },
    /// Append an operator-visible alert (info, warning, or error).
    AlertAppend { alert: AgentAlert },
    /// Append a diagnostic event for the ops timeline.
    DiagnosticAppend { event: AgentDiagnosticEvent },
    /// Update the MCP connection status.
    McpStatus { status: McpStatus },
    /// Signals that an agent's turn has started processing.
    TurnStarted {
        agent_id: String,
        mission_id: Option<String>,
        resume_thread_id: Option<String>,
    },
    /// Keep-alive heartbeat from a running agent turn.
    TurnHeartbeat {
        agent_id: String,
        mission_id: Option<String>,
    },
    /// Update the current processing stage label for an agent turn.
    TurnStage {
        agent_id: String,
        mission_id: Option<String>,
        stage: String,
    },
    /// Free-form log line emitted during a turn (routed to diagnostics).
    TurnLog { agent_id: String, message: String },
    /// An agent wrote to a file during its turn. Emitted by the runner when
    /// it detects tool_use(edit/write/bash) targeting a file path.
    /// This is the authoritative source for per-agent file attribution —
    /// used by the genome system instead of filesystem-level tracking.
    FileWrite {
        agent_id: String,
        path: std::path::PathBuf,
    },
    /// Report token usage for context-window tracking.
    TokenCount {
        agent_id: String,
        mission_id: Option<String>,
        token_count: AgentTokenCount,
    },
    /// Signals that an agent's turn ended with an error.
    TurnFailed {
        agent_id: String,
        mission_id: Option<String>,
        thread_id: Option<String>,
        token_count: Option<AgentTokenCount>,
        message: String,
    },
    /// Signals that an agent's turn completed successfully.
    TurnCompleted {
        agent_id: String,
        mission_id: Option<String>,
        thread_id: Option<String>,
        token_count: Option<AgentTokenCount>,
        message: String,
    },
}

impl AgentBusEvent {
    pub fn apply(&self, state: &mut AppState) {
        match self {
            AgentBusEvent::AgentUpsert { agent } => {
                upsert_agent(state, agent.clone());
            }
            AgentBusEvent::MissionUpsert { mission } => {
                upsert_mission(state, mission.clone());
            }
            AgentBusEvent::MessageAppend { message } => {
                if let Some(mission_id) = message.mission_id.as_deref() {
                    mark_mission_provenance_dirty(state, mission_id);
                    let delta = estimate_codex_context_tokens(&message.text);
                    let entry = state
                        .agents
                        .codex_estimated_tokens_used_by_mission
                        .entry(mission_id.to_string())
                        .or_insert(0);
                    *entry = entry.saturating_add(delta);
                } else if let Some(agent_id) = message.agent_id.as_deref() {
                    mark_ad_hoc_provenance_dirty(state, agent_id);
                }
                state.agents.messages.push(message.clone());
                // If the operator was following the tail, keep following it.
                state.agents.console_scroll = CONSOLE_SCROLL_BOTTOM;
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
                resume_thread_id: _resume_thread_id,
            } => {
                let now = Instant::now();
                state.agents.active_turns.insert(
                    agent_id.clone(),
                    AgentTurnState {
                        started_at: now,
                        last_heartbeat_at: now,
                        last_output_at: now,
                        stage: None,
                    },
                );
                let at = timestamp_label(state);
                if let Some(agent) = state.agents.agents.iter_mut().find(|a| a.id == *agent_id) {
                    agent.status = AgentStatus::Running;
                    agent.queue_len = agent.queue_len.max(1);
                    agent.heartbeat_age_secs = 0;
                    agent.current_mission = mission_id.clone();
                }

                if let Some(mission_id) = mission_id.as_deref() {
                    if let Some(mission) = state
                        .agents
                        .missions
                        .iter_mut()
                        .find(|mission| mission.id == mission_id)
                    {
                        mission.status = "RUNNING".into();
                        mission.updated_at = at;
                    }
                }

                // Capture genome baselines and activate turn tracking.
                // On a fresh turn (not a retry), reset baselines to current state.
                // During retries (retry_count > 0), keep the original baselines.
                if state.genome_retry_count == 0 {
                    state.genome_baselines = state.genome_reports.clone();
                }
                // Per-agent turn tracking: init this agent's modified set and
                // snapshot git dirty files so TurnCompleted can attribute changes.
                state
                    .genome_turn_modified
                    .entry(agent_id.clone())
                    .or_default()
                    .clear();
                state.genome_shadow_evals.clear();
                state.genome_turn_active.insert(agent_id.clone());
            }
            AgentBusEvent::TurnHeartbeat {
                agent_id,
                mission_id,
            } => {
                if let Some(turn) = state.agents.active_turns.get_mut(agent_id) {
                    turn.last_heartbeat_at = Instant::now();
                }
                if let Some(agent) = state.agents.agents.iter_mut().find(|a| a.id == *agent_id) {
                    agent.heartbeat_age_secs = 0;
                    // Mission context is authoritative (including clearing it).
                    agent.current_mission = mission_id.clone();
                }
            }
            AgentBusEvent::TurnStage {
                agent_id,
                mission_id,
                stage,
            } => {
                if let Some(turn) = state.agents.active_turns.get_mut(agent_id) {
                    turn.last_output_at = Instant::now();
                    turn.stage = Some(stage.clone());
                }
                if let Some(agent) = state.agents.agents.iter_mut().find(|a| a.id == *agent_id) {
                    // Mission context is authoritative (including clearing it).
                    agent.current_mission = mission_id.clone();
                }
            }
            AgentBusEvent::FileWrite { agent_id, path } => {
                // Authoritative per-agent file attribution from the runner.
                state
                    .genome_turn_modified
                    .entry(agent_id.clone())
                    .or_default()
                    .insert(path.clone());
            }
            AgentBusEvent::TurnLog { agent_id, message } => {
                let source = backend_source_for_agent(state, agent_id);
                if let Some(turn) = state.agents.active_turns.get_mut(agent_id) {
                    turn.last_output_at = Instant::now();
                }
                let lowered = message.to_ascii_lowercase();
                let severity = if lowered.contains("error") || lowered.contains("failed") {
                    AgentAlertSeverity::Warn
                } else {
                    AgentAlertSeverity::Info
                };
                state.agents.diag_events.push(AgentDiagnosticEvent {
                    severity,
                    source: source.into(),
                    message: format!("[{agent_id}] {message}"),
                    at: timestamp_label(state),
                });
            }
            AgentBusEvent::TokenCount {
                agent_id,
                mission_id,
                token_count,
            } => {
                if let Some(turn) = state.agents.active_turns.get_mut(agent_id) {
                    turn.last_output_at = Instant::now();
                }
                apply_codex_token_count(state, agent_id, mission_id.as_deref(), token_count);
            }
            AgentBusEvent::TurnFailed {
                agent_id,
                mission_id,
                thread_id,
                token_count,
                message,
            } => {
                let source = backend_source_for_agent(state, agent_id);
                state.agents.active_turns.remove(agent_id);
                if let Some(token_count) = token_count.as_ref() {
                    apply_codex_token_count(state, agent_id, mission_id.as_deref(), token_count);
                }
                let at = timestamp_label(state);
                if let Some(agent) = state.agents.agents.iter_mut().find(|a| a.id == *agent_id) {
                    agent.status = AgentStatus::Error;
                    agent.queue_len = agent.queue_len.saturating_sub(1);
                    agent.heartbeat_age_secs = 0;
                    agent.current_mission = mission_id.clone();
                }

                if let (Some(mission_id), Some(thread_id)) =
                    (mission_id.as_deref(), thread_id.as_deref())
                {
                    state
                        .agents
                        .codex_mission_thread_ids
                        .entry(mission_id.to_string())
                        .or_default()
                        .insert(agent_id.clone(), thread_id.to_string());
                } else if let Some(thread_id) = thread_id.as_deref() {
                    state
                        .agents
                        .codex_thread_ids
                        .insert(agent_id.clone(), thread_id.to_string());
                }
                if let Some(mission_id) = mission_id.as_deref() {
                    if let Some(mission) = state
                        .agents
                        .missions
                        .iter_mut()
                        .find(|mission| mission.id == mission_id)
                    {
                        mission.status = "ERROR".into();
                        mission.updated_at = at.clone();
                    }
                }

                let source_label = match source {
                    "claude" => "Claude",
                    "gemini" => "Gemini",
                    "local" => "Local",
                    _ => "Codex",
                };
                state.agents.alerts.push(AgentAlert {
                    severity: AgentAlertSeverity::Error,
                    source: source.into(),
                    message: format!("[{agent_id}] {message}"),
                    at: at.clone(),
                });
                state.agents.diag_events.push(AgentDiagnosticEvent {
                    severity: AgentAlertSeverity::Error,
                    source: source.into(),
                    message: format!("[{agent_id}] {message}"),
                    at: at.clone(),
                });
                state.agents.console_scroll = CONSOLE_SCROLL_BOTTOM;
                state.status = Some(format!(
                    "{source_label} failed: {}",
                    summarize_agent_error(message)
                ));
            }
            AgentBusEvent::TurnCompleted {
                agent_id,
                mission_id,
                thread_id,
                token_count,
                message,
            } => {
                let source = backend_source_for_agent(state, agent_id);
                state.agents.active_turns.remove(agent_id);
                if let Some(token_count) = token_count.as_ref() {
                    apply_codex_token_count(state, agent_id, mission_id.as_deref(), token_count);
                }
                let at = timestamp_label(state);
                if let Some(agent) = state.agents.agents.iter_mut().find(|a| a.id == *agent_id) {
                    agent.queue_len = agent.queue_len.saturating_sub(1);
                    agent.status = if agent.queue_len > 0 {
                        AgentStatus::Waiting
                    } else {
                        AgentStatus::Idle
                    };
                    agent.heartbeat_age_secs = 0;
                    agent.current_mission = mission_id.clone();
                }

                if let Some(mission_id) = mission_id.as_deref() {
                    mark_mission_provenance_dirty(state, mission_id);
                    if let Some(thread_id) = thread_id.as_deref() {
                        state
                            .agents
                            .codex_mission_thread_ids
                            .entry(mission_id.to_string())
                            .or_default()
                            .insert(agent_id.clone(), thread_id.to_string());
                    }
                    if let Some(mission) = state
                        .agents
                        .missions
                        .iter_mut()
                        .find(|mission| mission.id == mission_id)
                    {
                        mission.status = "LIVE".into();
                        mission.updated_at = at.clone();
                    }
                } else if let Some(thread_id) = thread_id.as_deref() {
                    state
                        .agents
                        .codex_thread_ids
                        .insert(agent_id.clone(), thread_id.to_string());
                    mark_ad_hoc_provenance_dirty(state, agent_id);
                } else {
                    mark_ad_hoc_provenance_dirty(state, agent_id);
                }
                if let Some(mission_id) = mission_id.as_deref() {
                    let delta = estimate_codex_context_tokens(message);
                    let entry = state
                        .agents
                        .codex_estimated_tokens_used_by_mission
                        .entry(mission_id.to_string())
                        .or_insert(0);
                    *entry = entry.saturating_add(delta);
                }
                // Use the dispatch-time prompt index if available; check both
                // Codex and Claude prompt-index maps.
                let parent_prompt_idx = state
                    .agents
                    .codex_turn_prompt_idx
                    .remove(agent_id)
                    .or_else(|| state.agents.claude_turn_prompt_idx.remove(agent_id))
                    .or_else(|| {
                        state
                            .agents
                            .messages
                            .iter()
                            .enumerate()
                            .rev()
                            .find(|(_, msg)| {
                                msg.agent_id.is_none() && msg.mission_id == *mission_id
                            })
                            .map(|(idx, _)| idx)
                    });
                state.agents.messages.push(AgentMessage {
                    at: at.clone(),
                    channel: AgentChannel::Agent,
                    agent_id: Some(agent_id.clone()),
                    mission_id: mission_id.clone(),
                    text: message.clone(),
                    prompt_msg_idx: parent_prompt_idx,
                    kind: None,
                });
                state.agents.console_scroll = CONSOLE_SCROLL_BOTTOM;

                state.agents.diag_events.push(AgentDiagnosticEvent {
                    severity: AgentAlertSeverity::Info,
                    source: source.into(),
                    message: format!("[{agent_id}] turn completed"),
                    at,
                });

                // Reload editor buffer from disk (agent may have written to the file).
                state.editor_buffer_mut().reload_from_disk();

                // Mark genome turn as inactive. Actual evaluation is dispatched
                // to background threads by the TUI event loop (genome_worker)
                // to avoid blocking the main thread.
                state.genome_turn_active.remove(agent_id);
            }
        }

        // Drives the Agent ECG/criticality sampling and makes backend activity visible in the UI.
        state.agents.note_event();
    }
}

fn upsert_agent(state: &mut AppState, agent: AgentLane) {
    let Some(existing_idx) = state.agents.agents.iter().position(|a| a.id == agent.id) else {
        state.agents.agents.push(agent);
        if state.agents.selected_agent.is_none() {
            state.agents.selected_agent = state.agents.agents.last().map(|a| a.id.clone());
            state.agents.roster_selected = state.agents.agents.len().saturating_sub(1);
        }
        return;
    };
    state.agents.agents[existing_idx] = agent;
    if state.agents.selected_agent.is_none() {
        state.agents.selected_agent = state.agents.agents.first().map(|a| a.id.clone());
    }
    state.agents.roster_selected = state
        .agents
        .roster_selected
        .min(state.agents.agents.len().saturating_sub(1));
}

fn upsert_mission(state: &mut AppState, mission: MissionRecord) {
    let Some(existing_idx) = state
        .agents
        .missions
        .iter()
        .position(|m| m.id == mission.id)
    else {
        state.agents.missions.push(mission);
        if state.agents.selected_mission.is_none() {
            state.agents.selected_mission = state.agents.missions.last().map(|m| m.id.clone());
            state.agents.mission_selected = state.agents.missions.len().saturating_sub(1);
        }
        return;
    };
    state.agents.missions[existing_idx] = mission;

    if let Some(selected_id) = state.agents.selected_mission.as_deref() {
        if let Some(idx) = state
            .agents
            .missions
            .iter()
            .position(|m| m.id == selected_id)
        {
            state.agents.mission_selected = idx;
        }
    } else if !state.agents.missions.is_empty() {
        state.agents.selected_mission = state.agents.missions.first().map(|m| m.id.clone());
        state.agents.mission_selected = 0;
    }

    state.agents.mission_selected = state
        .agents
        .mission_selected
        .min(state.agents.missions.len().saturating_sub(1));
}

fn mark_mission_provenance_dirty(state: &mut AppState, mission_id: &str) {
    if state
        .agents
        .pending_provenance_mission_ids
        .iter()
        .all(|id| id != mission_id)
    {
        state
            .agents
            .pending_provenance_mission_ids
            .push(mission_id.to_string());
    }
}

fn mark_ad_hoc_provenance_dirty(state: &mut AppState, agent_id: &str) {
    if state
        .agents
        .pending_provenance_agent_ids
        .iter()
        .all(|id| id != agent_id)
    {
        state
            .agents
            .pending_provenance_agent_ids
            .push(agent_id.to_string());
    }
}

fn estimate_codex_context_tokens(text: &str) -> u32 {
    if text.is_empty() {
        return 0;
    }
    let bytes = text.len() as u32;
    bytes.div_ceil(4)
}

fn timestamp_label(state: &AppState) -> String {
    format!("t+{}", state.metrics.frame_count)
}

fn apply_codex_token_count(
    state: &mut AppState,
    agent_id: &str,
    mission_id: Option<&str>,
    token_count: &AgentTokenCount,
) {
    let is_claude = state
        .agents
        .agents
        .iter()
        .find(|a| a.id == agent_id)
        .is_some_and(|a| a.is_claude());

    if is_claude {
        apply_token_count_claude(state, agent_id, mission_id, token_count);
    } else {
        apply_token_count_codex(state, agent_id, mission_id, token_count);
    }
}

fn apply_token_count_codex(
    state: &mut AppState,
    agent_id: &str,
    mission_id: Option<&str>,
    token_count: &AgentTokenCount,
) {
    if token_count.context_window > 0 {
        state
            .agents
            .codex_effective_context_window_tokens
            .insert(agent_id.to_string(), token_count.context_window);
    }
    let context_window = if token_count.context_window > 0 {
        Some(token_count.context_window)
    } else {
        state
            .agents
            .codex_effective_context_window_tokens
            .get(agent_id)
            .copied()
    };

    let used = context_window
        .map(|window| token_count.total_tokens.min(window.max(1)))
        .unwrap_or(token_count.total_tokens);
    if let Some(mission_id) = mission_id {
        state
            .agents
            .codex_mission_used_tokens
            .entry(mission_id.to_string())
            .or_default()
            .insert(agent_id.to_string(), used);
    } else {
        state
            .agents
            .codex_used_tokens
            .insert(agent_id.to_string(), used);
    }
    let Some(context_window) = context_window else {
        return;
    };
    if context_window == 0 {
        return;
    }

    let remaining = context_window.saturating_sub(used);
    let denom = context_window as u64;
    let pct = (((remaining as u64).saturating_mul(100)).saturating_add(denom / 2) / denom) as u8;

    if let Some(mission_id) = mission_id {
        state
            .agents
            .codex_mission_context_remaining_pct
            .entry(mission_id.to_string())
            .or_default()
            .insert(agent_id.to_string(), pct);
    } else {
        state
            .agents
            .codex_context_remaining_pct
            .insert(agent_id.to_string(), pct);
    }
}

fn apply_token_count_claude(
    state: &mut AppState,
    agent_id: &str,
    mission_id: Option<&str>,
    token_count: &AgentTokenCount,
) {
    if token_count.context_window > 0 {
        state
            .agents
            .claude_effective_context_window_tokens
            .insert(agent_id.to_string(), token_count.context_window);
    }
    let context_window = if token_count.context_window > 0 {
        Some(token_count.context_window)
    } else {
        state
            .agents
            .claude_effective_context_window_tokens
            .get(agent_id)
            .copied()
    };

    let used = context_window
        .map(|window| token_count.total_tokens.min(window.max(1)))
        .unwrap_or(token_count.total_tokens);
    if let Some(mission_id) = mission_id {
        state
            .agents
            .claude_mission_used_tokens
            .entry(mission_id.to_string())
            .or_default()
            .insert(agent_id.to_string(), used);
    } else {
        state
            .agents
            .claude_used_tokens
            .insert(agent_id.to_string(), used);
    }
    let Some(context_window) = context_window else {
        return;
    };
    if context_window == 0 {
        return;
    }

    let remaining = context_window.saturating_sub(used);
    let denom = context_window as u64;
    let pct = (((remaining as u64).saturating_mul(100)).saturating_add(denom / 2) / denom) as u8;

    if let Some(mission_id) = mission_id {
        state
            .agents
            .claude_mission_context_remaining_pct
            .entry(mission_id.to_string())
            .or_default()
            .insert(agent_id.to_string(), pct);
    } else {
        state
            .agents
            .claude_context_remaining_pct
            .insert(agent_id.to_string(), pct);
    }
}

fn summarize_agent_error(message: &str) -> String {
    let trimmed = message.trim();
    if trimmed.is_empty() {
        return "unknown error".into();
    }

    if let Some(value) = parse_error_json(trimmed) {
        if let Some(msg) = extract_error_message(&value) {
            let msg = msg.trim();
            if !msg.is_empty() {
                return msg.to_string();
            }
        }
    }

    trimmed.lines().next().unwrap_or(trimmed).trim().to_string()
}

fn parse_error_json(text: &str) -> Option<serde_json::Value> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }
    if (trimmed.starts_with('{') && trimmed.ends_with('}'))
        || (trimmed.starts_with('[') && trimmed.ends_with(']'))
    {
        return serde_json::from_str::<serde_json::Value>(trimmed).ok();
    }

    let start = trimmed.find('{')?;
    let end = trimmed.rfind('}')?;
    if start >= end {
        return None;
    }
    serde_json::from_str::<serde_json::Value>(&trimmed[start..=end]).ok()
}

fn extract_error_message(value: &serde_json::Value) -> Option<&str> {
    value
        .get("error")
        .and_then(|err| err.get("message"))
        .and_then(|v| v.as_str())
        .or_else(|| value.get("message").and_then(|v| v.as_str()))
}

// ---------------------------------------------------------------------------
// Genome report persistence
// ---------------------------------------------------------------------------

fn genome_dir(workspace_root: &std::path::Path) -> std::path::PathBuf {
    workspace_root.join(".nit").join("genome")
}

fn genome_report_filename(file_path: &std::path::Path) -> String {
    let s = file_path.to_string_lossy();
    format!("{}.json", s.replace('/', "__"))
}

pub fn persist_genome_report(
    workspace_root: &std::path::Path,
    report: &crate::genome_report::GenomeReport,
) {
    let dir = genome_dir(workspace_root);
    let _ = std::fs::create_dir_all(&dir);
    let filename = genome_report_filename(&report.file_path);
    let path = dir.join(filename);
    if let Ok(json) = serde_json::to_string(report) {
        let _ = std::fs::write(path, json);
    }
}

/// Load previously persisted genome reports from `.nit/genome/`.
pub fn load_genome_reports(
    workspace_root: &std::path::Path,
) -> std::collections::HashMap<std::path::PathBuf, crate::genome_report::GenomeReport> {
    let mut map = std::collections::HashMap::new();
    let dir = genome_dir(workspace_root);
    let entries = match std::fs::read_dir(&dir) {
        Ok(e) => e,
        Err(_) => return map,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let data = match std::fs::read_to_string(&path) {
            Ok(d) => d,
            Err(_) => continue,
        };
        let report: crate::genome_report::GenomeReport = match serde_json::from_str(&data) {
            Ok(r) => r,
            Err(_) => continue,
        };
        map.insert(report.file_path.clone(), report);
    }
    map
}

#[cfg(test)]
#[path = "tests/agent_bus.rs"]
mod tests;
