#![allow(unused_imports)]
#![allow(clippy::too_many_arguments)]
use std::collections::{BTreeSet, HashSet};
use std::fs;
use std::io::{self, Stdout};
use std::path::{Path, PathBuf};
use std::sync::{
    mpsc::{self, Receiver, Sender},
    Arc, Mutex, Weak,
};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::swarm::{
    chat_clone_base_id, normalize_role_label, GateReport, GateReportGate, SwarmArtifactFocus,
    SwarmRuntime,
};
use crate::{
    claude_runner::{ClaudeRunner, ClaudeRunnerConfig},
    codex_runner::{CodexCommand, CodexRunner, CodexRunnerConfig, CodexRuntimeMode},
    file_tree,
    file_tree_runner::{FileTreeCommand, FileTreeEvent, FileTreeRunner},
    file_watcher::FileWatcher,
    fuzzy_preview_runner::{PreviewEvent, PreviewModel, PreviewRunner},
    fuzzy_search_runner::{
        ContentEvent, ContentSearchRunner, FileIndexRunner, FuzzyCommand, FuzzyEvent,
        FuzzyMatcherRunner, IndexEvent,
    },
    games_petri_dish::GamesPetriDishRuntime,
    layout,
    petri_dish::PetriDishRuntime,
    seed_runtime::SeedRuntime,
    syntax::SyntaxRuntime,
    system_stats::SystemStats,
    theme::Theme,
    vitals::{AgentVitalsState, DiagSeverity, LabVitalsSnapshot, VitalsState},
    widgets::{
        agent_console_view, agent_ops_view, artifacts_history_popup, artifacts_popup, bottom_bar,
        editor_view, file_tree_view, fuzzy_search_popup, games_analysis_popup, games_ca_sim_popup,
        games_match_history_popup, games_replay_popup, games_run_browser_popup,
        games_strategy_popup, games_tm_sim_popup, games_visualizer_view, gate_monitor_view,
        help_overlay, protocol_picker, rule_picker, substrate_overlay, top_bar, visualizer_view,
    },
};
use arboard::Clipboard;
use crossterm::{
    cursor::{SetCursorStyle, Show},
    event::{
        self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
        Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, KeyboardEnhancementFlags, MouseEvent,
        MouseEventKind, PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
    },
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ctrlc::Error as CtrlcError;
use nit_core::{
    actions::Action, apply_action, io as core_io, AgentAlert, AgentAlertSeverity, AgentBusEvent,
    AgentChannel, AgentDiagnosticEvent, AgentMessage, AgentOpsTab, AgentStatus, AppKind, AppState,
    McpConnectionState, MissionPhase, MissionRecord, Mode, PaneId, PatchProposal, PatchStatus,
    Prompt, SavedRunHistoryFilter, SearchMode, UiSelection, UiSelectionPane, YankKind,
    CONSOLE_SCROLL_BOTTOM,
};
use nit_games::config::GamesConfig;
use ratatui::{
    backend::CrosstermBackend,
    layout::Rect,
    style::Style,
    widgets::{Block, BorderType, Borders, Clear, Paragraph},
    Terminal,
};

use super::*;

pub(super) const VERIFY_OUTPUT_MD_MAX_CHARS: usize = 4_000;

pub(super) fn save_notes_on_exit(state: &AppState) -> core_io::Result<()> {
    let buffer = state.notes_buffer();
    if buffer.path().is_none() {
        return Ok(());
    }
    if !buffer.is_dirty() {
        return Ok(());
    }
    core_io::save_buffer(buffer)
}

pub(super) fn flush_agent_run_provenance(
    state: &mut AppState,
    swarm: &SwarmRuntime,
) -> io::Result<()> {
    let pending = std::mem::take(&mut state.agents.pending_provenance_mission_ids);
    let pending_agents = std::mem::take(&mut state.agents.pending_provenance_agent_ids);
    if !pending.is_empty() {
        let unique = pending.into_iter().collect::<BTreeSet<_>>();
        for mission_id in unique {
            write_agent_run_provenance(state, swarm, &mission_id)?;
        }
    }
    if !pending_agents.is_empty() {
        let unique = pending_agents.into_iter().collect::<BTreeSet<_>>();
        for agent_id in unique {
            write_ad_hoc_run_provenance(state, &agent_id)?;
        }
    }
    Ok(())
}

pub(super) fn write_agent_run_provenance(
    state: &AppState,
    swarm: &SwarmRuntime,
    mission_id: &str,
) -> io::Result<()> {
    let Some(mission) = state
        .agents
        .missions
        .iter()
        .find(|mission| mission.id == mission_id)
    else {
        return Ok(());
    };
    let run_dir = state
        .workspace_root
        .join(".nit")
        .join("agents")
        .join("runs")
        .join(mission_id);
    let patches_dir = run_dir.join("patches");
    fs::create_dir_all(&patches_dir)?;

    let messages = state
        .agents
        .messages
        .iter()
        .filter(|msg| msg.mission_id.as_deref() == Some(mission_id))
        .cloned()
        .collect::<Vec<_>>();
    let patches = state
        .agents
        .patches
        .iter()
        .filter(|patch| patch.mission_id.as_deref() == Some(mission_id))
        .cloned()
        .collect::<Vec<_>>();
    let evidence = state
        .agents
        .evidence
        .iter()
        .filter(|item| item.mission_id.as_deref() == Some(mission_id))
        .cloned()
        .collect::<Vec<_>>();
    let run_payload = serde_json::json!({
            "id": mission.id,
            "title": mission.title,
            "phase": mission.phase.label(),
        "status": mission.status,
        "swarm": mission.swarm,
            "assigned_agents": mission.assigned_agents,
            "updated_at": mission.updated_at,
            "selected_agent": state.agents.selected_context_agent(),
            "codex_thread_id": state
                .agents
                .selected_context_agent()
                .and_then(|agent| state.agents.codex_mission_thread_ids.get(mission_id)?.get(agent)),
            "codex_thread_ids": state.agents.codex_mission_thread_ids.get(mission_id),
            "mcp": {
                "state": state.agents.mcp.state.label(),
                "endpoint": state.agents.mcp.endpoint,
                "latency_ms": state.agents.mcp.latency_ms,
            "last_error": state.agents.mcp.last_error,
        },
        "messages": messages.clone(),
        "patches": patches.clone(),
        "evidence": evidence,
    });
    let run_json = serde_json::to_string_pretty(&run_payload)
        .map_err(|err| io::Error::other(format!("serde run.json: {err}")))?;
    fs::write(run_dir.join("run.json"), run_json)?;

    let mut thread_md = String::new();
    thread_md.push_str(&format!("# Mission {}\n\n", mission.id));
    thread_md.push_str(&format!("Title: {}\n\n", mission.title));
    thread_md.push_str("## Thread\n\n");
    for msg in messages.iter() {
        let channel = match msg.channel {
            AgentChannel::Agent => "",
            AgentChannel::Broadcast => "@all ",
        };
        let src = msg.agent_id.as_deref().unwrap_or("user");
        thread_md.push_str(&format!(
            "- [{}] {}{}: {}\n",
            msg.at, channel, src, msg.text
        ));
    }
    fs::write(run_dir.join("thread.md"), thread_md)?;

    for patch in patches.iter() {
        let filename = format!("{}.diff", sanitize_for_filename(&patch.id));
        fs::write(patches_dir.join(filename), &patch.diff)?;
    }
    write_swarm_run_provenance(state, swarm, mission_id)?;
    Ok(())
}

pub(super) fn write_ad_hoc_run_provenance(state: &AppState, agent_id: &str) -> io::Result<()> {
    let run_dir = state
        .workspace_root
        .join(".nit")
        .join("agents")
        .join("ad-hoc")
        .join(sanitize_for_filename(agent_id));
    let patches_dir = run_dir.join("patches");
    fs::create_dir_all(&patches_dir)?;

    let is_own_or_clone = |id: Option<&str>| -> bool {
        id == Some(agent_id) || id.is_some_and(|id| chat_clone_base_id(id) == Some(agent_id))
    };
    let messages = state
        .agents
        .messages
        .iter()
        .filter(|message| {
            message.mission_id.is_none()
                && (message.agent_id.is_none() || is_own_or_clone(message.agent_id.as_deref()))
        })
        .cloned()
        .collect::<Vec<_>>();
    let patches = state
        .agents
        .patches
        .iter()
        .filter(|patch| {
            patch.mission_id.is_none()
                && (patch.agent_id == agent_id
                    || chat_clone_base_id(&patch.agent_id) == Some(agent_id))
        })
        .cloned()
        .collect::<Vec<_>>();
    let evidence = state
        .agents
        .evidence
        .iter()
        .filter(|item| item.mission_id.is_none() && is_own_or_clone(item.agent_id.as_deref()))
        .cloned()
        .collect::<Vec<_>>();

    let run_payload = serde_json::json!({
        "agent_id": agent_id,
        "context": "ad-hoc",
        "updated_at": timestamp_label(state),
        "codex_thread_id": state.agents.codex_thread_ids.get(agent_id),
        "messages": messages.clone(),
        "patches": patches.clone(),
        "evidence": evidence,
    });
    let run_json = serde_json::to_vec_pretty(&run_payload)
        .map_err(|err| io::Error::other(format!("serde ad-hoc run.json: {err}")))?;
    write_file_atomic(&run_dir.join("run.json"), &run_json)?;

    let mut thread_md = String::new();
    thread_md.push_str(&format!("# Ad-hoc thread for {agent_id}\n\n"));
    thread_md.push_str("## Thread\n\n");
    for msg in messages.iter() {
        let channel = match msg.channel {
            AgentChannel::Agent => "",
            AgentChannel::Broadcast => "@all ",
        };
        let src = msg.agent_id.as_deref().unwrap_or("user");
        thread_md.push_str(&format!(
            "- [{}] {}{}: {}\n",
            msg.at, channel, src, msg.text
        ));
    }
    write_file_atomic(&run_dir.join("thread.md"), thread_md.as_bytes())?;

    for patch in patches.iter() {
        let filename = format!("{}.diff", sanitize_for_filename(&patch.id));
        write_file_atomic(&patches_dir.join(filename), patch.diff.as_bytes())?;
    }

    Ok(())
}

pub(super) fn write_swarm_run_provenance(
    state: &AppState,
    swarm: &SwarmRuntime,
    mission_id: &str,
) -> io::Result<()> {
    let Some(mission) = state
        .agents
        .missions
        .iter()
        .find(|mission| mission.id == mission_id)
    else {
        return Ok(());
    };
    if !mission.swarm {
        return Ok(());
    }
    let Some(view) = swarm.swarm_persistence(mission_id) else {
        return Ok(());
    };

    let run_dir = state
        .workspace_root
        .join(".nit")
        .join("swarm")
        .join(mission_id);
    let tasks_dir = run_dir.join("tasks");
    let gates_dir = run_dir.join("gates");
    let report_dir = run_dir.join("report");
    fs::create_dir_all(&tasks_dir)?;
    fs::create_dir_all(&gates_dir)?;
    fs::create_dir_all(&report_dir)?;

    let run_payload = serde_json::json!({
        "id": mission.id,
        "title": mission.title,
        "phase": mission.phase.label(),
        "status": mission.status,
        "template": view.template,
        "swarm": mission.swarm,
        "updated_at": mission.updated_at,
        "gate_bundle": view.gate_bundle,
        "gate_selection": view.gate_selection,
        "report_status": view.report_status,
        "report_agent_id": view.report_agent_id,
        "report_present": view.report_output.is_some(),
        "task_count": view.tasks.len(),
        "tasks": view.tasks.iter().map(|task| {
            serde_json::json!({
                "id": task.id,
                "agent_id": task.agent_id,
                "role": task.role,
                "title": task.title,
                "state": task.state,
                "deps": task.deps,
                "blocked_on": task.blocked_on,
                "writes": task.writes,
                "expected_artifacts": task.expected_artifacts,
                "expected_artifacts_missing": task.expected_artifacts_missing,
                "output_present": task.output_present
            })
        }).collect::<Vec<_>>()
    });
    let run_json = serde_json::to_vec_pretty(&run_payload)
        .map_err(|err| io::Error::other(format!("serde swarm run.json: {err}")))?;
    write_file_atomic(&run_dir.join("run.json"), &run_json)?;

    let mut summary_entries = Vec::new();
    for task in view.tasks.iter() {
        let task_dir = tasks_dir.join(sanitize_for_filename(&task.id));
        fs::create_dir_all(&task_dir)?;

        if let Some(artifacts) = task.artifacts.as_ref() {
            let artifacts_json = serde_json::to_vec_pretty(artifacts)
                .map_err(|err| io::Error::other(format!("serde artifacts.json: {err}")))?;
            write_file_atomic(&task_dir.join("artifacts.json"), &artifacts_json)?;
            if let Some(summary) = artifacts.summary.as_deref().map(str::trim) {
                if !summary.is_empty() {
                    summary_entries.push(serde_json::json!({
                        "task_id": task.id,
                        "summary": summary
                    }));
                }
            }
        }

        if let Some(output) = task.output.as_deref() {
            write_file_atomic(&task_dir.join("output.md"), output.as_bytes())?;
        }
    }

    if !summary_entries.is_empty() {
        let summary_json = serde_json::to_vec_pretty(&serde_json::json!({
            "mission_id": mission_id,
            "summaries": summary_entries
        }))
        .map_err(|err| io::Error::other(format!("serde summary.json: {err}")))?;
        write_file_atomic(&run_dir.join("summary.json"), &summary_json)?;
    }

    // Phase 8: index this mission for cross-mission retrieval. Best-effort —
    // provenance writes must not be broken by memory-index failures.
    let _ = nit_core::mission_memory::upsert_mission(state.workspace_root.as_path(), mission_id);

    if let Some(report) = view.gate_report.as_ref() {
        let report_json = serde_json::to_vec_pretty(report)
            .map_err(|err| io::Error::other(format!("serde gate report: {err}")))?;
        write_file_atomic(&gates_dir.join("report.json"), &report_json)?;
    }
    if let Some(output) = view.gate_output.as_deref() {
        write_file_atomic(&gates_dir.join("output.txt"), output.as_bytes())?;
    }
    if view.gate_bundle.is_some() || view.gate_report.is_some() || view.gate_output.is_some() {
        let verify_md = render_verify_markdown(
            mission_id,
            view.gate_bundle.as_deref(),
            view.gate_selection.as_str(),
            view.gate_report.as_ref(),
            view.gate_output.as_deref(),
        );
        write_file_atomic(&gates_dir.join("verify.md"), verify_md.as_bytes())?;
    }
    if let Some(report_output) = view.report_output.as_deref() {
        write_file_atomic(&report_dir.join("final.md"), report_output.as_bytes())?;
    }

    Ok(())
}

pub(super) const MAX_SAVED_RUN_HISTORY_PER_CONTEXT: usize = 200;

pub(super) fn prune_saved_run_history(history_root: &Path, keep_latest: usize) -> io::Result<()> {
    let read_dir = match fs::read_dir(history_root) {
        Ok(read_dir) => read_dir,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(err),
    };
    let mut archive_dirs = read_dir
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .collect::<Vec<_>>();
    archive_dirs.sort_by(|left, right| right.cmp(left));
    for archive_dir in archive_dirs.into_iter().skip(keep_latest) {
        fs::remove_dir_all(archive_dir)?;
    }
    Ok(())
}

pub(super) fn archive_saved_run_snapshot(run_dir: &Path) -> io::Result<Option<PathBuf>> {
    let run_json = run_dir.join("run.json");
    if !run_json.exists() {
        return Ok(None);
    }

    let archive_id_base = format!(
        "{:020}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_micros()
    );
    let history_root = run_dir.join("history");
    fs::create_dir_all(&history_root)?;

    let mut archive_dir = history_root.join(&archive_id_base);
    let mut suffix = 1usize;
    while archive_dir.exists() {
        archive_dir = history_root.join(format!("{archive_id_base}-{suffix}"));
        suffix = suffix.saturating_add(1);
    }
    fs::create_dir_all(archive_dir.join("patches"))?;

    for file_name in ["run.json", "thread.md"] {
        let src = run_dir.join(file_name);
        if !src.is_file() {
            continue;
        }
        let contents = fs::read(&src)?;
        write_file_atomic(&archive_dir.join(file_name), &contents)?;
    }

    let patches_src = run_dir.join("patches");
    if patches_src.is_dir() {
        for entry in fs::read_dir(&patches_src)? {
            let entry = match entry {
                Ok(entry) => entry,
                Err(_) => continue,
            };
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let Some(file_name) = path.file_name() else {
                continue;
            };
            let contents = fs::read(&path)?;
            write_file_atomic(&archive_dir.join("patches").join(file_name), &contents)?;
        }
    }

    prune_saved_run_history(&history_root, MAX_SAVED_RUN_HISTORY_PER_CONTEXT)?;

    Ok(Some(archive_dir))
}

pub(super) fn verify_gate_status_label(gate: &GateReportGate) -> &'static str {
    if let Some(status) = gate.status.as_deref() {
        if status.eq_ignore_ascii_case("pass")
            || status.eq_ignore_ascii_case("ok")
            || status.eq_ignore_ascii_case("success")
        {
            return "PASS";
        }
        if status.eq_ignore_ascii_case("skip") || status.eq_ignore_ascii_case("skipped") {
            return "SKIP";
        }
        if status.eq_ignore_ascii_case("fail") || status.eq_ignore_ascii_case("failed") {
            return "FAIL";
        }
    }
    if gate.ok {
        "PASS"
    } else {
        "FAIL"
    }
}

pub(super) fn truncate_for_markdown(text: &str, max_chars: usize) -> String {
    let total = text.chars().count();
    if total <= max_chars {
        return text.to_string();
    }
    let truncated = text.chars().take(max_chars).collect::<String>();
    format!("{truncated}\n\n... [truncated {} chars]", total - max_chars)
}

pub(super) fn render_verify_markdown(
    mission_id: &str,
    gate_bundle: Option<&str>,
    gate_selection: &str,
    report: Option<&GateReport>,
    output: Option<&str>,
) -> String {
    let mut out = String::new();
    out.push_str("# Verify\n\n");
    out.push_str(&format!("Mission: `{mission_id}`\n\n"));
    out.push_str(&format!("Bundle: `{}`\n\n", gate_bundle.unwrap_or("none")));
    out.push_str(&format!("Selection: `{}`\n\n", gate_selection.trim()));

    let status = if let Some(report) = report {
        if report.overall_ok {
            "PASS"
        } else {
            "FAIL"
        }
    } else if gate_bundle.is_some() {
        "PENDING"
    } else {
        "NONE"
    };
    out.push_str(&format!("Status: `{status}`\n\n"));

    if let Some(report) = report {
        out.push_str("## Gates\n\n");
        for gate in report.gates.iter() {
            out.push_str(&format!(
                "- `{}`: `{}`\n",
                gate.name,
                verify_gate_status_label(gate)
            ));
            out.push_str(&format!("  - Command: `{}`\n", gate.command));
            if let Some(notes) = gate
                .notes
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
            {
                out.push_str(&format!("  - Notes: {notes}\n"));
            }
        }
        out.push('\n');
    }

    out.push_str("## Files\n\n");
    if report.is_some() {
        out.push_str("- `report.json`\n");
    }
    if output.is_some() {
        out.push_str("- `output.txt`\n");
    }
    if gate_bundle.is_some() || report.is_some() || output.is_some() {
        out.push_str("- `verify.md`\n");
    }
    out.push('\n');

    if let Some(output) = output.map(str::trim).filter(|s| !s.is_empty()) {
        out.push_str("## Output Excerpt\n\n```text\n");
        out.push_str(&truncate_for_markdown(output, VERIFY_OUTPUT_MD_MAX_CHARS));
        out.push_str("\n```\n");
    }

    out
}

pub(super) fn write_file_atomic(path: &Path, contents: &[u8]) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let file_name = path
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| "data".into());
    let tmp = path.with_file_name(format!(".{file_name}.nit.tmp"));
    fs::write(&tmp, contents)?;
    fs::rename(tmp, path)?;
    Ok(())
}

pub(super) fn sanitize_for_filename(input: &str) -> String {
    input
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .collect()
}
