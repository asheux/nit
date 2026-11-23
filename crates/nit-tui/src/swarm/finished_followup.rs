//! A finished writer keeps its original result. A subsequent operator request
//! becomes a successor task, followed by an independent check before synthesis.
use super::{SwarmRun, SwarmRuntime, SwarmStage, SwarmTask, SwarmTaskState};
use std::{
    collections::{BTreeSet, HashMap},
    path::{Path, PathBuf},
};

pub(super) type TaskWrites = HashMap<PathBuf, BTreeSet<String>>;

impl SwarmRuntime {
    pub(super) fn record_task_write(&mut self, mid: &str, agent: &str, path: &Path) {
        let Some(run) = self
            .runs
            .get(mid)
            .filter(|r| r.stage == SwarmStage::Executing)
        else {
            return;
        };
        let Some(task) = run.tasks.iter().find(|t| {
            t.writes
                && t.agent_id == agent
                && matches!(
                    t.state,
                    SwarmTaskState::Running | SwarmTaskState::Dispatched
                )
        }) else {
            return;
        };
        self.task_writes
            .entry(mid.into())
            .or_default()
            .entry(run.spawn_cwd.join(path))
            .or_default()
            .insert(task.id.clone());
    }
}

fn reviewer<'a>(run: &'a SwarmRun, writer: &str) -> Option<&'a str> {
    run.tasks
        .iter()
        .filter(|t| matches!(t.role.as_deref(), Some("review" | "test")))
        .map(|t| t.agent_id.as_str())
        .chain(run.verifier_agent_id.as_deref())
        .chain(std::iter::once(run.planner_agent_id.as_str()))
        .find(|a| *a != writer && run.agent_ids.iter().any(|candidate| candidate == a))
}

pub(super) fn available(run: &SwarmRun, task: &SwarmTask) -> bool {
    task.state == SwarmTaskState::Done && task.writes && reviewer(run, &task.agent_id).is_some()
}

pub(super) fn successor_tasks(
    run: &SwarmRun,
    original: &SwarmTask,
    operator_text: &str,
) -> Result<[SwarmTask; 2], String> {
    if original.state != SwarmTaskState::Done || !original.writes {
        return Err(format!(
            "Task {} is finished. Choose a live task or start a new mission.",
            original.id
        ));
    }
    let reviewer = reviewer(run, &original.agent_id).ok_or(
        "No independent reviewer is assigned to this mission. Start a new mission to continue this finished writer's task.",
    )?;
    let mut sequence = 1;
    let (id, check_id) = loop {
        let id = format!("followup-{sequence:02}");
        let check_id = format!("review-{id}");
        if !run.tasks.iter().any(|t| t.id == id || t.id == check_id) {
            break (id, check_id);
        }
        sequence += 1;
    };
    let mut deps: Vec<_> = run
        .tasks
        .iter()
        .filter(|t| !t.state.is_terminal())
        .map(|t| t.id.clone())
        .collect();
    if deps.is_empty() {
        return Err(
            "The mission is finalizing. Start a new mission to continue this completed task."
                .into(),
        );
    }
    deps.push(original.id.clone());
    let mut writer = SwarmTask::pending(
        id.clone(),
        original.agent_id.clone(),
        original.role.clone(),
        deps,
        true,
    );
    writer.title = format!("Follow up on {}", original.id);
    writer.task_prompt = format!(
        "Continue completed task {} ({}). Address the operator follow-up within this role's permissions. Inspect the current workspace, preserve subsequent work and report what changed for this request.",
        original.id, original.title
    );
    writer.artifacts = vec!["diffs".into(), "notes".into()];
    writer.done_when =
        Some("The requested follow-up is addressed or a concrete blocker is documented.".into());
    let mut review = SwarmTask::pending(
        check_id,
        reviewer.into(),
        Some("review".into()),
        vec![id.clone()],
        false,
    );
    review.title = format!("Review {id}");
    review.task_prompt = format!(
        "Independently check the actual follow-up changes from {id} against the operator's request. Cite evidence, relevant checks and any outstanding failure. Keep the review read-only.\nOperator request:\n{operator_text}"
    );
    review.artifacts = vec!["findings".into(), "notes".into()];
    Ok([writer, review])
}
