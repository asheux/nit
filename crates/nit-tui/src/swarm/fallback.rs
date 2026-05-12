use super::{
    ParsedSwarmPlan, SwarmMissionKind, SwarmTask, SwarmTaskState, SwarmTemplate,
    COMPUTATIONAL_RESEARCH_ROLE,
};

// Default lens framings cycled across proposers when the planner LLM call
// fails entirely. Seven axes chosen to be genuinely orthogonal — diversity
// here is what gives the judge meaningful candidates to weigh, not just
// correlated samples of the same prompt.
const PROPOSER_LENSES: [&str; 7] = [
    "minimal diff / safest change",
    "correctness & edge cases",
    "UX/TUI clarity",
    "performance & scalability",
    "testing & verification",
    "docs & maintainability",
    "security & failure modes",
];

// Soft cap on bulk fallback proposers. The runtime hard cap in
// `swarm/limits.rs` is `BULK_PRACTICAL_MAX = 12`; this lower number leaves
// per-dep budget headroom so the judge sees thicker proposals on the
// emergency path than on the LLM-planner happy path.
const BULK_PROPOSER_CAP: usize = 8;

pub(super) fn fallback_tasks(
    template: SwarmTemplate,
    mission_kind: SwarmMissionKind,
    _root_prompt: &str,
    available_agents: &[String],
    plan_error: Option<&str>,
    integrator_hint: Option<&str>,
) -> ParsedSwarmPlan {
    match template {
        SwarmTemplate::Bulk => bulk_fallback(available_agents, plan_error, integrator_hint),
        SwarmTemplate::Lab => {
            lab_fallback(mission_kind, available_agents, plan_error, integrator_hint)
        }
        SwarmTemplate::Parallel => {
            parallel_fallback(template, mission_kind, available_agents, plan_error)
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn pending_task(
    id: String,
    agent_id: String,
    role: Option<String>,
    title: String,
    task_prompt: String,
    deps: Vec<String>,
    writes: bool,
    artifacts: Vec<String>,
    done_when: Option<String>,
) -> SwarmTask {
    SwarmTask {
        id,
        agent_id,
        role,
        title,
        task_prompt,
        deps,
        writes,
        artifacts,
        done_when,
        state: SwarmTaskState::Pending,
        output: None,
        parsed_artifacts: None,
        expected_artifacts_missing: false,
        failed: false,
        retries: 0,
        compliance_missing_files: Vec::new(),
        shard_index: None,
        pre_dispatch_file_state: std::collections::HashMap::new(),
    }
}

fn first_match(agents: &[String], reject: &[Option<&String>]) -> Option<String> {
    agents
        .iter()
        .find(|id| {
            !reject
                .iter()
                .any(|skip| skip.is_some_and(|s| s.as_str() == id.as_str()))
        })
        .cloned()
}

fn synthesis_note(plan_error: Option<&str>) -> Option<String> {
    plan_error.map(|err| {
        format!("Note: planner output could not be used; fallback prompts were used. Reason: {err}")
    })
}

fn bulk_fallback(
    available_agents: &[String],
    plan_error: Option<&str>,
    integrator_hint: Option<&str>,
) -> ParsedSwarmPlan {
    let integrator = integrator_hint
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .and_then(|id| {
            available_agents
                .iter()
                .find(|candidate| candidate.as_str() == id)
                .cloned()
        })
        .or_else(|| available_agents.first().cloned());
    let judge_agent =
        first_match(available_agents, &[integrator.as_ref()]).or_else(|| integrator.clone());
    let review_agent = first_match(
        available_agents,
        &[integrator.as_ref(), judge_agent.as_ref()],
    )
    .or_else(|| judge_agent.clone())
    .or_else(|| integrator.clone());

    let mut proposer_ids: Vec<String> = available_agents
        .iter()
        .filter(|id| integrator.as_ref() != Some(*id))
        .filter(|id| judge_agent.as_ref() != Some(*id))
        .cloned()
        .collect();
    if proposer_ids.is_empty() {
        if let Some(judge) = judge_agent.clone() {
            proposer_ids.push(judge);
        } else if let Some(integrator) = integrator.clone() {
            proposer_ids.push(integrator);
        }
    }
    proposer_ids.truncate(BULK_PROPOSER_CAP);

    let mut tasks = Vec::new();
    let proposer_task_ids = push_bulk_proposer_tasks(&mut tasks, &proposer_ids);
    push_bulk_judge_task(&mut tasks, judge_agent.clone(), &proposer_task_ids);
    push_bulk_integrate_task(&mut tasks, integrator.clone());
    push_bulk_review_task(&mut tasks, review_agent);

    ParsedSwarmPlan {
        tasks,
        synthesis_prompt: synthesis_note(plan_error),
        integrator_agent_id: integrator,
        warnings: Vec::new(),
    }
}

fn push_bulk_proposer_tasks(tasks: &mut Vec<SwarmTask>, proposer_ids: &[String]) -> Vec<String> {
    let mut proposer_task_ids = Vec::new();
    for (idx, agent_id) in proposer_ids.iter().enumerate() {
        let id = format!("propose-{:02}", idx + 1);
        let lens = PROPOSER_LENSES
            .get(idx)
            .copied()
            .unwrap_or("alternative approach");
        proposer_task_ids.push(id.clone());
        tasks.push(pending_task(
            id,
            agent_id.clone(),
            Some("propose".into()),
            format!("Proposal ({lens})"),
            format!(
                "Propose an end-to-end solution candidate.\n\nLens: {lens}\n\nConstraints:\n- Do NOT edit the workspace (read-only).\n- Be concrete: file paths, key symbols, and exact commands.\n- If helpful, include a small unified diff (but do not apply it).\n"
            ),
            Vec::new(),
            false,
            vec!["options".into(), "files".into(), "commands".into(), "risks".into()],
            Some("We have a concrete, repo-grounded candidate solution with tradeoffs.".into()),
        ));
    }
    proposer_task_ids
}

fn push_bulk_judge_task(
    tasks: &mut Vec<SwarmTask>,
    judge_agent: Option<String>,
    proposer_task_ids: &[String],
) {
    let Some(agent_id) = judge_agent else {
        return;
    };
    tasks.push(pending_task(
        "judge".into(),
        agent_id,
        Some("judge".into()),
        "Judge + select approach".into(),
        "Compare the proposer outputs and pick the best approach. Provide:\n- Decision (which proposal / why)\n- A step-by-step integration plan for the integrator\n- Acceptance criteria\n- Exact verification commands\n\nConstraints:\n- Do NOT edit the workspace (read-only).\n"
            .into(),
        proposer_task_ids.to_vec(),
        false,
        vec!["decision".into(), "plan".into(), "commands".into(), "risks".into()],
        Some("Integrator has a clear, actionable plan to implement.".into()),
    ));
}

fn push_bulk_integrate_task(tasks: &mut Vec<SwarmTask>, integrator: Option<String>) {
    let Some(agent_id) = integrator else {
        return;
    };
    tasks.push(pending_task(
        "integrate".into(),
        agent_id,
        Some("integrate".into()),
        "Integrate selected approach".into(),
        "Implement the selected approach using the judge output.\n\nConstraints:\n- You are the ONLY agent allowed to edit the workspace.\n- Prefer small, safe diffs.\n- For verification, follow the TEST DISCIPLINE in the role contract above (targeted only — no workspace-wide commands unless the operator explicitly asked).\n"
            .into(),
        vec!["judge".into()],
        true,
        Vec::new(),
        Some("Changes are implemented cleanly with validations passing.".into()),
    ));
}

fn push_bulk_review_task(tasks: &mut Vec<SwarmTask>, review_agent: Option<String>) {
    let Some(agent_id) = review_agent else {
        return;
    };
    tasks.push(pending_task(
        "review".into(),
        agent_id,
        Some("review".into()),
        "Review final diff".into(),
        "Review the integrated changes for correctness, UX, and maintainability. Suggest follow-ups and edge cases.\n\nConstraints:\n- Do NOT edit the workspace (read-only).\n"
            .into(),
        vec!["integrate".into()],
        false,
        vec!["risks".into(), "commands".into()],
        Some("We have confidence in correctness and know remaining risks.".into()),
    ));
}

fn lab_fallback(
    mission_kind: SwarmMissionKind,
    available_agents: &[String],
    plan_error: Option<&str>,
    integrator_hint: Option<&str>,
) -> ParsedSwarmPlan {
    let integrator = integrator_hint
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .and_then(|id| {
            available_agents
                .iter()
                .find(|candidate| candidate.as_str() == id)
                .cloned()
        })
        .or_else(|| available_agents.first().cloned());
    let recon_agent =
        first_match(available_agents, &[integrator.as_ref()]).or_else(|| integrator.clone());
    let design_agent = first_match(
        available_agents,
        &[integrator.as_ref(), recon_agent.as_ref()],
    )
    .or_else(|| recon_agent.clone())
    .or_else(|| integrator.clone());
    let review_agent = first_match(
        available_agents,
        &[
            integrator.as_ref(),
            recon_agent.as_ref(),
            design_agent.as_ref(),
        ],
    )
    .or_else(|| design_agent.clone())
    .or_else(|| recon_agent.clone())
    .or_else(|| integrator.clone());

    let mut tasks = Vec::new();
    let research_mission = mission_kind.allows_research_roles();
    push_lab_recon_task(&mut tasks, recon_agent, mission_kind);
    push_lab_design_task(&mut tasks, design_agent, mission_kind);
    push_lab_implement_task(&mut tasks, integrator.clone(), mission_kind);
    push_lab_review_task(&mut tasks, review_agent, research_mission);

    ParsedSwarmPlan {
        tasks,
        synthesis_prompt: synthesis_note(plan_error),
        integrator_agent_id: integrator,
        warnings: Vec::new(),
    }
}

fn push_lab_recon_task(
    tasks: &mut Vec<SwarmTask>,
    recon_agent: Option<String>,
    mission_kind: SwarmMissionKind,
) {
    let Some(agent_id) = recon_agent else {
        return;
    };
    let (role, title, task_prompt, artifacts, done_when) = match mission_kind {
        SwarmMissionKind::Research => (
            Some("research".into()),
            "Sources + prior-art survey".into(),
            "Survey papers, docs, web resources, and related references for the operator request. Extract the strongest sources, competing ideas, and the key assumptions or unknowns. Stay read-only and keep the output grounded in evidence.".into(),
            vec!["sources".into(), "notes".into(), "risks".into()],
            Some("We have a grounded map of the best sources, references, and research directions.".into()),
        ),
        SwarmMissionKind::ComputationalResearch => (
            Some("research".into()),
            "Sources + problem framing".into(),
            "Survey papers, docs, datasets, and web resources to frame the problem. Summarize the strongest prior work, data sources, evaluation criteria, and the assumptions the computational lane should test. Stay read-only.".into(),
            vec!["sources".into(), "methods".into(), "risks".into()],
            Some("We have a solid source base and a clear problem framing for computational work.".into()),
        ),
        SwarmMissionKind::General => (
            None,
            "Codebase recon".into(),
            "Scan the repository for the most relevant files/modules and summarize where changes should happen. Provide concrete file paths and key functions/symbols. Avoid proposing large diffs; focus on mapping the terrain and risks.".into(),
            vec!["files".into(), "risks".into()],
            Some("We know exactly where changes should happen and the main risks.".into()),
        ),
    };
    tasks.push(pending_task(
        "recon".into(),
        agent_id,
        role,
        title,
        task_prompt,
        Vec::new(),
        false,
        artifacts,
        done_when,
    ));
}

fn push_lab_design_task(
    tasks: &mut Vec<SwarmTask>,
    design_agent: Option<String>,
    mission_kind: SwarmMissionKind,
) {
    let Some(agent_id) = design_agent else {
        return;
    };
    let (role, title, task_prompt, artifacts, done_when) = match mission_kind {
        SwarmMissionKind::Research => (
            Some("research".into()),
            "Compare directions + rank strategies".into(),
            "Use the strongest sources to compare competing ideas, strategies, or solution paths. Rank the best options, explain tradeoffs, and call out what still needs validation. Stay read-only.".into(),
            vec!["sources".into(), "methods".into(), "options".into()],
            Some("We have ranked strategy options with evidence, tradeoffs, and open questions.".into()),
        ),
        SwarmMissionKind::ComputationalResearch => (
            Some(COMPUTATIONAL_RESEARCH_ROLE.into()),
            "Model + evaluate candidates".into(),
            "Run the computation-heavy lane: use calculations, simulations, modeling, numerical methods, experiments, optimization, or reproducible analysis when helpful. Compare candidate strategies quantitatively, explain methods, and surface assumptions or data gaps. Stay read-only.".into(),
            vec!["methods".into(), "options".into(), "commands".into()],
            Some("We have a computationally grounded ranking of candidate strategies and the methods behind it.".into()),
        ),
        SwarmMissionKind::General => (
            Some("propose".into()),
            "Design options".into(),
            "Propose 2-3 plausible implementation approaches (with tradeoffs) and call out which files/modules each approach touches. Keep it specific and repo-grounded.".into(),
            vec!["options".into(), "files".into()],
            Some("We have 1-2 clear, repo-grounded approaches with tradeoffs.".into()),
        ),
    };
    tasks.push(pending_task(
        "design".into(),
        agent_id,
        role,
        title,
        task_prompt,
        Vec::new(),
        false,
        artifacts,
        done_when,
    ));
}

fn push_lab_implement_task(
    tasks: &mut Vec<SwarmTask>,
    integrator: Option<String>,
    mission_kind: SwarmMissionKind,
) {
    let Some(agent_id) = integrator else {
        return;
    };
    let (title, task_prompt, writes, artifacts, done_when) = match mission_kind {
        SwarmMissionKind::Research => (
            "Synthesize findings + recommendation".into(),
            "Integrate the upstream research into a decisive recommendation for the operator. Produce a concise synthesis, ranked next steps, and any follow-up research gaps. Stay read-only unless the operator explicitly asked for repo changes.".into(),
            false,
            vec!["notes".into(), "sources".into(), "commands".into()],
            Some("We have a clear recommendation backed by sources, assumptions, and ranked follow-ups.".into()),
        ),
        SwarmMissionKind::ComputationalResearch => (
            "Synthesize evidence + next-step plan".into(),
            "Integrate the upstream source survey and computational analysis into a decisive recommendation. Summarize the strongest evidence, methods, assumptions, ranked next steps, and any follow-up experiments. Stay read-only unless the operator explicitly asked for repo changes.".into(),
            false,
            vec!["notes".into(), "methods".into(), "commands".into()],
            Some("We have a computationally grounded recommendation with methods, assumptions, and next experiments.".into()),
        ),
        SwarmMissionKind::General => (
            "Integrate + implement".into(),
            "Implement the best approach using the dependency outputs. You are the only agent allowed to make workspace edits in this swarm. Prefer small, safe diffs. Follow the TEST DISCIPLINE in the role contract above for verification — targeted runs only, no workspace-wide commands unless the operator asked.".into(),
            true,
            Vec::new(),
            Some("Changes are implemented cleanly with validations to run.".into()),
        ),
    };
    tasks.push(pending_task(
        "implement".into(),
        agent_id,
        Some("integrate".into()),
        title,
        task_prompt,
        vec!["recon".into(), "design".into()],
        writes,
        artifacts,
        done_when,
    ));
}

fn push_lab_review_task(
    tasks: &mut Vec<SwarmTask>,
    review_agent: Option<String>,
    research_mission: bool,
) {
    let Some(agent_id) = review_agent else {
        return;
    };
    let (task_prompt, artifacts, done_when) = if research_mission {
        (
            "Review the synthesized findings for weak evidence, missing sources, shaky assumptions, and overlooked follow-up questions. Suggest better references, validation steps, or experiments as text only; do not apply changes.".into(),
            vec!["risks".into(), "sources".into(), "commands".into()],
            Some("We know the main evidence gaps, risks, and the next checks to run.".into()),
        )
    } else {
        (
            "Review the implemented approach for correctness, UX, and maintainability. Suggest verification steps (exact commands) and edge cases. If you propose edits, do so as text/diff; do not apply changes.".into(),
            vec!["risks".into(), "commands".into()],
            Some("We have confidence in correctness and a clear test plan.".into()),
        )
    };
    tasks.push(pending_task(
        "review".into(),
        agent_id,
        Some("review".into()),
        "Review & verification".into(),
        task_prompt,
        vec!["implement".into()],
        false,
        artifacts,
        done_when,
    ));
}

fn parallel_fallback(
    template: SwarmTemplate,
    mission_kind: SwarmMissionKind,
    available_agents: &[String],
    plan_error: Option<&str>,
) -> ParsedSwarmPlan {
    let mut tasks = Vec::new();
    for (agent_idx, agent_id) in available_agents.iter().enumerate() {
        let task_id = format!("task-{:02}", agent_idx + 1);
        let (role, title, prompt, deps, writes) =
            parallel_task_assignment(template, mission_kind, agent_idx);
        tasks.push(pending_task(
            task_id,
            agent_id.clone(),
            role,
            title.into(),
            prompt.into(),
            deps,
            writes,
            Vec::new(),
            None,
        ));
    }

    ParsedSwarmPlan {
        tasks,
        synthesis_prompt: synthesis_note(plan_error),
        integrator_agent_id: None,
        warnings: Vec::new(),
    }
}

fn parallel_task_assignment(
    template: SwarmTemplate,
    mission_kind: SwarmMissionKind,
    agent_idx: usize,
) -> (
    Option<String>,
    &'static str,
    &'static str,
    Vec<String>,
    bool,
) {
    match mission_kind {
        SwarmMissionKind::Research => research_task_assignment(agent_idx),
        SwarmMissionKind::ComputationalResearch => {
            computational_research_task_assignment(agent_idx)
        }
        SwarmMissionKind::General => general_task_assignment(template, agent_idx),
    }
}

fn research_task_assignment(
    agent_idx: usize,
) -> (
    Option<String>,
    &'static str,
    &'static str,
    Vec<String>,
    bool,
) {
    match agent_idx {
        0 => (
            Some("research".to_string()),
            "Source survey",
            "Survey papers, docs, web resources, and related references. Identify the strongest sources, competing ideas, and missing information.",
            Vec::new(),
            false,
        ),
        1 => (
            Some("research".to_string()),
            "Strategy comparison",
            "Compare the best ideas from the sources, rank the strongest strategies, and explain their tradeoffs, assumptions, and open questions.",
            Vec::new(),
            false,
        ),
        2 => (
            Some("review".to_string()),
            "Gap review",
            "Review the research outputs for weak evidence, missing sources, shaky assumptions, and better follow-up directions.",
            Vec::new(),
            false,
        ),
        _ => (
            Some("review".to_string()),
            "Review & pitfalls",
            "Review the proposed research direction for evidence quality, missing citations, and strategic blind spots.",
            Vec::new(),
            false,
        ),
    }
}

fn computational_research_task_assignment(
    agent_idx: usize,
) -> (
    Option<String>,
    &'static str,
    &'static str,
    Vec<String>,
    bool,
) {
    match agent_idx {
        0 => (
            Some("research".to_string()),
            "Source survey",
            "Survey papers, docs, datasets, and related resources. Summarize prior work, evaluation criteria, and the most useful evidence for downstream computational analysis.",
            Vec::new(),
            false,
        ),
        1 => (
            Some(COMPUTATIONAL_RESEARCH_ROLE.to_string()),
            "Model + experiment lane",
            "Use simulations, modeling, numerical methods, optimization, calculations, or reproducible analysis when helpful. Compare candidate strategies and explain methods, commands, and assumptions.",
            Vec::new(),
            false,
        ),
        2 => (
            Some("review".to_string()),
            "Evidence review",
            "Review the research and computational outputs for weak methods, missing baselines, data issues, and follow-up experiments.",
            Vec::new(),
            false,
        ),
        _ => (
            Some("review".to_string()),
            "Review & pitfalls",
            "Review the proposed computational research direction for evidence quality, methodological risks, and better alternatives.",
            Vec::new(),
            false,
        ),
    }
}

fn general_task_assignment(
    template: SwarmTemplate,
    agent_idx: usize,
) -> (
    Option<String>,
    &'static str,
    &'static str,
    Vec<String>,
    bool,
) {
    match (template, agent_idx) {
        (SwarmTemplate::Lab, 0) => (
            Some("research".to_string()),
            "Codebase recon",
            "Scan the repository for the most relevant files/modules and summarize where changes should happen. Provide concrete file paths and key functions/symbols. Avoid proposing large diffs; focus on mapping the terrain and risks.",
            Vec::new(),
            false,
        ),
        (SwarmTemplate::Lab, 1) => (
            Some("research".to_string()),
            "Design options",
            "Propose 2-3 plausible implementation approaches (with tradeoffs) and call out which files/modules each approach touches. Keep it specific and repo-grounded.",
            Vec::new(),
            false,
        ),
        (SwarmTemplate::Lab, 2) => (
            Some("integrate".to_string()),
            "Integrate + implement",
            "Implement the best approach using the dependency outputs. You are the only agent allowed to make workspace edits in this swarm. Prefer small, safe diffs. Follow the TEST DISCIPLINE in the role contract above for verification — targeted runs only, no workspace-wide commands unless the operator asked.",
            vec!["task-01".into(), "task-02".into()],
            true,
        ),
        (SwarmTemplate::Lab, _) => (
            Some("review".to_string()),
            "Review & verification",
            "Review the proposed approach for correctness, UX, and maintainability. Suggest verification steps (exact commands) and edge cases. If you propose edits, do so as text/diff; do not apply changes.",
            vec!["task-03".into()],
            false,
        ),
        (_, 0) => (
            Some("recon".to_string()),
            "Codebase recon",
            "Scan the repository for the most relevant files/modules and summarize where changes should happen. Provide concrete file paths and key functions/symbols. Avoid proposing large diffs; focus on mapping the terrain and risks.",
            Vec::new(),
            false,
        ),
        (_, 1) => (
            Some("plan".to_string()),
            "Implementation plan",
            "Propose an implementation approach and the specific code changes needed. If appropriate, provide a concise unified diff for the most important edits. Call out any concurrency/file-conflict risks with multiple agents.",
            Vec::new(),
            false,
        ),
        (_, 2) => (
            Some("test".to_string()),
            "Tests & verification",
            "Propose how to verify the change (tests, manual checks, edge cases). If tests likely exist, suggest exact commands and where to add/update test coverage.",
            Vec::new(),
            false,
        ),
        (_, _) => (
            Some("review".to_string()),
            "Review & pitfalls",
            "Review the planned approach for correctness, UX, and maintainability. Point out edge cases, failure modes, and simpler alternatives.",
            Vec::new(),
            false,
        ),
    }
}
