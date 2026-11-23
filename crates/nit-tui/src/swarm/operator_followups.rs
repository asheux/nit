//! Each live DAG owns its operator receipts. Archived receipts are inspectable,
//! but a reused task or mission name can never implicitly replay them.
use super::{
    graph_exec::{
        build_task_prompt, collect_dependency_payload, collect_integrate_scope, collect_shard_files,
    },
    push_system_message_to_mission, SwarmDispatch, SwarmRuntime, SwarmStage, SwarmTaskState,
};
use nit_core::{
    followups::{Receipt, Source, Status},
    substrate::{ClaimKind, ClaimTarget},
    AppState,
};
use std::{
    collections::BTreeSet,
    io,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

pub(super) fn unique_id() -> String {
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!(
        "{nanos:x}-{:x}-{:x}",
        std::process::id(),
        SEQUENCE.fetch_add(1, Ordering::Relaxed)
    )
}

pub(super) struct Inbox {
    id: String,
    pub(super) root: PathBuf,
    pub(super) entries: Vec<Receipt>,
    confirmed_input: BTreeSet<String>,
    pub(super) deferred_writers: Vec<String>,
}

pub(super) fn save(root: &Path, receipt: &Receipt) -> io::Result<()> {
    let dir = nit_core::followups::ensure_directory(
        root,
        &[".nit", "swarm", &receipt.mission_id, "followups"],
    )?;
    nit_utils::write_atomic(&dir.join(format!("{}.json", receipt.id)), |writer| {
        serde_json::to_writer_pretty(writer, receipt).map_err(io::Error::other)
    })
}

pub(super) fn transition(
    dir: &Path,
    receipt: &mut Receipt,
    status: Status,
    detail: &str,
    state: &mut AppState,
) {
    receipt.status = status;
    receipt.detail = Some(match receipt.detail.take() {
        Some(previous) => format!("{previous}. {detail}"),
        None => detail.into(),
    });
    if let Err(error) = save(dir, receipt) {
        push_system_message_to_mission(state, &receipt.mission_id, format!(
            "Follow-up {}: {detail}. Could not update disk receipt: {error}. Check the turn before resending.", receipt.id));
    }
}

/// Emit only when the real provider dequeues a prompt that contains receipt
/// markers. Multiple turns for a lane may be waiting ahead of the continuation.
pub(crate) fn report_provider_followup_input(
    tx: &std::sync::mpsc::Sender<nit_core::AgentBusEvent>,
    agent: &str,
    mid: Option<&str>,
    prompt: &str,
) {
    let Some(mid) = mid else { return };
    if !prompt.contains("[nit-followup ") {
        return;
    }
    let receipt_ids: Vec<_> = prompt
        .lines()
        .filter_map(|line| {
            let id = line.strip_prefix("[nit-followup ")?.strip_suffix(']')?;
            (id.len() < 80
                && !id.is_empty()
                && id.bytes().all(|b| b.is_ascii_hexdigit() || b == b'-'))
            .then(|| id.to_owned())
        })
        .collect();
    if !receipt_ids.is_empty() {
        let _ = tx.send(nit_core::AgentBusEvent::OperatorFollowupInput {
            agent_id: agent.into(),
            mission_id: mid.into(),
            receipt_ids,
        });
    }
}

impl SwarmRuntime {
    #[cfg(test)]
    pub(crate) fn set_test_workspace(&mut self, mid: &str, workspace: &Path) {
        self.runs.get_mut(mid).unwrap().spawn_cwd = workspace.to_owned();
    }

    pub(crate) fn only_operator_followup_task(&self, mid: &str) -> Option<&str> {
        let run = self.runs.get(mid)?;
        if run.stage != SwarmStage::Executing {
            return None;
        }
        let mut tasks = run.tasks.iter().filter(|t| !t.state.is_terminal());
        let first = tasks.next()?;
        tasks.next().is_none().then_some(first.id.as_str())
    }

    pub(crate) fn operator_followup_task_for_agent(
        &self,
        mid: &str,
        agent: &str,
    ) -> Result<&str, String> {
        let run = self
            .runs
            .get(mid)
            .ok_or("No active swarm for that mission.")?;
        let mut tasks = run
            .tasks
            .iter()
            .filter(|t| !t.state.is_terminal() && t.agent_id == agent);
        match (tasks.next(), tasks.next()) {
            (Some(task), None) => Ok(task.id.as_str()),
            _ => Err(format!(
                "No unique live task for {agent}. Choose a task: {}.",
                self.operator_followup_tasks(mid).join(", ")
            )),
        }
    }

    pub(crate) fn operator_followup_tasks(&self, mid: &str) -> Vec<String> {
        let Some(run) = self
            .runs
            .get(mid)
            .filter(|run| run.stage == SwarmStage::Executing)
        else {
            return vec![];
        };
        run.tasks
            .iter()
            .filter(|t| !t.state.is_terminal() || super::finished_followup::available(run, t))
            .map(|t| t.id.clone())
            .collect()
    }

    pub(crate) fn operator_followup_task_for_path(
        &self,
        state: &AppState,
        mid: &str,
        path: &Path,
    ) -> Result<String, String> {
        let run = self
            .runs
            .get(mid)
            .ok_or("No active swarm for that mission.")?;
        let absolute = state.workspace_root.join(path);
        let resolved = super::finished_followup::workspace_path(&run.spawn_cwd, &absolute)
            .ok_or("Choose a task explicitly for a path outside this mission workspace.")?;
        let relative = resolved.as_path();
        let claimants: BTreeSet<_> = state
            .substrate
            .claims_iter()
            .filter(|c| c.kind == ClaimKind::ExclusiveWrite)
            .filter(|c| match &c.target {
                ClaimTarget::File { path } | ClaimTarget::Region { path, .. } => {
                    path == relative
                        || path == &absolute
                        || super::finished_followup::workspace_path(&run.spawn_cwd, path).as_deref()
                            == Some(relative)
                }
                ClaimTarget::Global => false,
            })
            .map(|c| c.claimed_by.as_str())
            .collect();
        let recorded = self
            .task_writes
            .get(mid)
            .and_then(|paths| paths.get(relative));
        let mut owners = vec![];
        for task in run.tasks.iter().filter(|t| {
            t.writes && (!t.state.is_terminal() || super::finished_followup::available(run, t))
        }) {
            if task.state.is_terminal() {
                if recorded.is_some_and(|ids| ids.contains(&task.id)) {
                    owners.push(task);
                }
                continue;
            }
            let is_running = task.state == SwarmTaskState::Running;
            let written = is_running
                && state
                    .genome_turn_modified
                    .get(&task.agent_id)
                    .is_some_and(|paths| paths.contains(relative) || paths.contains(&absolute));
            // The planner's task.artifacts field specifies output *keys* (e.g.
            // "files"), not filenames. Use only the actual assigned file scope.
            let declared = if task.shard_index.is_some() {
                collect_shard_files(run, task)
            } else if task.role.as_deref() == Some("integrate") {
                collect_integrate_scope(run, task)
            } else {
                None
            }
            .is_some_and(|paths| paths.iter().any(|p| Path::new(p) == relative));

            if written
                || declared
                || claimants.contains(task.agent_id.as_str())
                || recorded.is_some_and(|ids| ids.contains(&task.id))
            {
                owners.push(task);
            }
        }
        match owners.as_slice() {
            [task] if claimants.iter().all(|owner| *owner == task.agent_id) => Ok(task.id.clone()),
            _ => Err(format!(
                "No unique live owner for {}. Choose a task explicitly: {}.",
                path.display(),
                self.operator_followup_tasks(mid).join(", ")
            )),
        }
    }

    pub(crate) fn queue_operator_followup(
        &mut self,
        state: &AppState,
        mid: &str,
        target: Option<&str>,
        text: &str,
        source: Option<&Source>,
    ) -> Result<String, String> {
        let run = self.runs.get(mid).ok_or(
            "That mission is no longer running. Choose a live task or start a new mission.",
        )?;
        match run.stage {
            SwarmStage::Planning => return Err("The mission is planning. Keep the draft and choose a task when its DAG appears.".into()),
            SwarmStage::Verifying | SwarmStage::Synthesizing => return Err("The mission is verifying or reporting. Its tasks are closed. Keep the draft for a new mission.".into()),
            SwarmStage::Executing => {}
        }
        if text.trim().is_empty() {
            return Err("Enter a follow-up before sending.".into());
        }
        let path_owner;
        let target = match (target, source) {
            (None, Some(evidence)) => {
                if evidence.revision.is_some() {
                    return Err("Choose a task explicitly for historical evidence.".into());
                }
                path_owner =
                    self.operator_followup_task_for_path(state, mid, Path::new(&evidence.path))?;
                Some(path_owner.as_str())
            }
            _ => target,
        };
        let id = target
            .or_else(|| self.only_operator_followup_task(mid))
            .ok_or_else(|| {
                format!(
                    "Choose a live task: {}.",
                    self.operator_followup_tasks(mid).join(", ")
                )
            })?;
        let task = run.tasks.iter().find(|t| t.id == id).ok_or_else(|| {
            format!(
                "Task {id} is not in this mission. Choose one of: {}.",
                self.operator_followup_tasks(mid).join(", ")
            )
        })?;
        let successor = if task.state.is_terminal() {
            Some(super::finished_followup::successor_tasks(run, task, text)?)
        } else {
            None
        };
        let recipient = successor.as_ref().map_or(task, |tasks| &tasks[0]);
        let inbox = self.followups.entry(mid.into()).or_insert_with(|| Inbox {
            id: unique_id(),
            root: run.spawn_cwd.clone(),
            entries: vec![],
            confirmed_input: BTreeSet::new(),
            deferred_writers: Vec::new(),
        });
        let receipt = Receipt {
            id: unique_id(),
            run_id: inbox.id.clone(),
            mission_id: mid.into(),
            task_id: recipient.id.clone(),
            agent_id: task.agent_id.clone(),
            text: text.into(),
            source: source.cloned(),
            status: Status::Pending,
            detail: successor.as_ref().map(|tasks| {
                format!(
                    "Continuation of {}; review assigned to {}",
                    task.id, tasks[1].agent_id
                )
            }),
        };
        save(&inbox.root, &receipt).map_err(|e| format!("Could not save follow-up: {e}"))?;
        let confirmation = match &successor {
            Some([writer, review]) => format!(
                "Queued {} for {} after existing work · {} reviews · {mid} · receipt {}",
                writer.id, writer.agent_id, review.agent_id, receipt.id
            ),
            None => format!(
                "Queued for {} ({}) · {mid} · receipt {}",
                task.id, task.agent_id, receipt.id
            ),
        };
        inbox.entries.push(receipt);
        if let Some(tasks) = successor {
            inbox.deferred_writers.push(tasks[0].id.clone());
            self.runs
                .get_mut(mid)
                .expect("validated active mission")
                .tasks
                .extend(tasks);
        }
        Ok(confirmation)
    }

    pub(super) fn attach_followups(&mut self, state: &mut AppState, dispatch: &mut SwarmDispatch) {
        let mid = &dispatch.mission_id;
        let Some(run) = self.runs.get(mid) else {
            return;
        };
        if run.stage != SwarmStage::Executing {
            return;
        }
        let Some(task) = run
            .tasks
            .iter()
            .find(|t| t.agent_id == dispatch.agent_id && t.state == SwarmTaskState::Dispatched)
        else {
            return;
        };
        // The runner must see this exact final line in its original prompt.
        // Add it after any note payload so nested prior prompts cannot win.
        let nonce = unique_id();
        self.live_followups
            .bind(mid, &task.id, &task.agent_id, nonce.clone());
        let Some(inbox) = self.followups.get_mut(mid) else {
            dispatch
                .prompt
                .push_str(&format!("\n[nit-task-turn {nonce}]"));
            return;
        };
        for receipt in inbox.entries.iter_mut().filter(|r| {
            r.task_id == task.id && r.agent_id == task.agent_id && r.status == Status::Pending
        }) {
            state
                .agents
                .followup_workspaces
                .entry(mid.clone())
                .or_insert_with(|| run.spawn_cwd.clone());
            dispatch.prompt.push_str(&format!(
                "\n\n[nit-followup {}]\nOperator follow-up for task {}:\n{}",
                receipt.id, task.id, receipt.text
            ));
            if let Some(source) = &receipt.source {
                dispatch
                    .prompt
                    .push_str(&format!("\nSource: {}", source.path));
                if let Some(rev) = &source.revision {
                    dispatch.prompt.push_str(&format!(" at {rev}"));
                }
                if let Some(hunk) = &source.hunk {
                    dispatch.prompt.push_str(&format!("\n{hunk}"));
                }
            }
            transition(
                &inbox.root,
                receipt,
                Status::Submitted,
                "Included in the outbound task turn; awaiting provider completion",
                state,
            );
        }
        dispatch
            .prompt
            .push_str(&format!("\n[nit-task-turn {nonce}]"));
    }

    pub(super) fn provider_input(&mut self, mid: &str, agent: &str, ids: &[String]) {
        if let Some(inbox) = self.followups.get_mut(mid) {
            inbox.confirmed_input.extend(
                inbox
                    .entries
                    .iter()
                    .filter(|r| {
                        r.status == Status::Submitted && r.agent_id == agent && ids.contains(&r.id)
                    })
                    .map(|r| r.id.clone()),
            );
        }
    }

    pub(super) fn awaiting_provider_input(&self, mid: &str, agent: &str) -> bool {
        self.followups.get(mid).is_some_and(|inbox| {
            inbox.entries.iter().any(|r| {
                r.agent_id == agent
                    && r.status == Status::Submitted
                    && !inbox.confirmed_input.contains(&r.id)
            })
        })
    }

    pub(super) fn finish_followup_turn(
        &mut self,
        state: &mut AppState,
        mid: &str,
        agent: &str,
        success: bool,
        message: &str,
    ) -> Option<SwarmDispatch> {
        let inbox = self.followups.get_mut(mid)?;
        let run = self.runs.get_mut(mid)?;
        if run.stage != SwarmStage::Executing {
            return None;
        }
        let task = run.tasks.iter().find(|t| {
            t.agent_id == agent
                && matches!(
                    t.state,
                    SwarmTaskState::Dispatched | SwarmTaskState::Running
                )
        })?;
        let mut pending = false;
        for r in inbox
            .entries
            .iter_mut()
            .filter(|r| r.task_id == task.id && r.agent_id == agent)
        {
            if matches!(r.status, Status::Submitted | Status::Accepted) {
                if success {
                    transition(
                        &inbox.root,
                        r,
                        Status::Delivered,
                        "Provider completed the turn containing this follow-up",
                        state,
                    )
                } else {
                    transition(
                        &inbox.root,
                        r,
                        Status::Failed,
                        "Provider turn failed; inspect the run before resending",
                        state,
                    )
                }
            }
            pending |= r.status == Status::Pending;
        }
        if !pending || !success {
            return None;
        }
        let mut prompt = build_task_prompt(run, task, &collect_dependency_payload(run, task));
        prompt.push_str("\n\nContinue this same task with the operator follow-up. Preserve prior work and return the required final handoff. Previous turn output:\n");
        prompt.push_str(&super::truncate_chars(message, 8000));
        let dispatch = SwarmDispatch {
            mission_id: mid.into(),
            agent_id: agent.into(),
            task_role: task.role.clone(),
            prompt,
        };
        run.tasks
            .iter_mut()
            .find(|t| {
                t.agent_id == agent
                    && matches!(
                        t.state,
                        SwarmTaskState::Dispatched | SwarmTaskState::Running
                    )
            })?
            .state = SwarmTaskState::Dispatched;
        Some(dispatch)
    }

    pub(super) fn close_undelivered(&mut self, state: &mut AppState, mid: &str) {
        let active = self
            .runs
            .get(mid)
            .filter(|run| run.stage == SwarmStage::Executing);
        if active.is_none() {
            self.live_followups.close(mid);
            self.task_writes.remove(mid);
        }
        let Some(inbox) = self.followups.get_mut(mid) else {
            return;
        };
        for r in inbox.entries.iter_mut().filter(|r| {
            matches!(
                r.status,
                Status::Pending | Status::Submitted | Status::Steering | Status::Accepted
            )
        }) {
            if !active.is_some_and(|run| {
                run.tasks.iter().any(|t| {
                    t.id == r.task_id && t.agent_id == r.agent_id && !t.state.is_terminal()
                })
            }) {
                let status = if r.status == Status::Steering {
                    Status::Unknown
                } else {
                    Status::Failed
                };
                transition(&inbox.root,r,status,"Target task ended before delivery was confirmed; recover the receipt to resend",state);
                push_system_message_to_mission(
                    state,
                    mid,
                    format!(
                        "Follow-up {} to {} was not confirmed. See /followups.",
                        r.id, r.task_id
                    ),
                );
            }
        }
        if active.is_none() {
            state.agents.followup_workspaces.remove(mid);
            self.followups.remove(mid);
        }
    }
}

#[cfg(test)]
#[path = "../tests/operator_followups.rs"]
mod tests;
