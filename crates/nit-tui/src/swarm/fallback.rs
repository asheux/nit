use super::{
    ParsedSwarmPlan, SwarmMissionKind, SwarmTask, SwarmTaskState, SwarmTemplate,
    COMPUTATIONAL_RESEARCH_ROLE,
};

pub(super) fn fallback_tasks(
    template: SwarmTemplate,
    mission_kind: SwarmMissionKind,
    _root_prompt: &str,
    available_agents: &[String],
    plan_error: Option<&str>,
    integrator_hint: Option<&str>,
) -> ParsedSwarmPlan {
    if matches!(template, SwarmTemplate::Bulk) {
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
        let judge_agent = available_agents
            .iter()
            .find(|id| integrator.as_ref() != Some(*id))
            .cloned()
            .or_else(|| integrator.clone());
        let review_agent = available_agents
            .iter()
            .find(|id| integrator.as_ref() != Some(*id) && judge_agent.as_ref() != Some(*id))
            .cloned()
            .or_else(|| judge_agent.clone())
            .or_else(|| integrator.clone());

        let mut proposer_ids = available_agents
            .iter()
            .filter(|id| integrator.as_ref() != Some(*id))
            .filter(|id| judge_agent.as_ref() != Some(*id))
            .cloned()
            .collect::<Vec<_>>();
        if proposer_ids.is_empty() {
            if let Some(judge) = judge_agent.clone() {
                proposer_ids.push(judge);
            } else if let Some(integrator) = integrator.clone() {
                proposer_ids.push(integrator);
            }
        }
        proposer_ids.truncate(8);

        let proposer_lenses = [
            "minimal diff / safest change",
            "correctness & edge cases",
            "UX/TUI clarity",
            "performance & scalability",
            "testing & verification",
            "docs & maintainability",
            "security & failure modes",
        ];

        let mut tasks = Vec::new();
        let mut proposer_task_ids = Vec::new();
        for (idx, agent_id) in proposer_ids.into_iter().enumerate() {
            let id = format!("propose-{:02}", idx + 1);
            let lens = proposer_lenses
                .get(idx)
                .copied()
                .unwrap_or("alternative approach");
            proposer_task_ids.push(id.clone());
            tasks.push(SwarmTask {
                id,
                agent_id,
                role: Some("propose".into()),
                title: format!("Proposal ({lens})"),
                task_prompt: format!(
                    "Propose an end-to-end solution candidate.\n\nLens: {lens}\n\nConstraints:\n- Do NOT edit the workspace (read-only).\n- Be concrete: file paths, key symbols, and exact commands.\n- If helpful, include a small unified diff (but do not apply it).\n"
                ),
                deps: Vec::new(),
                writes: false,
                artifacts: vec!["options".into(), "files".into(), "commands".into(), "risks".into()],
                done_when: Some(
                    "We have a concrete, repo-grounded candidate solution with tradeoffs."
                        .into(),
                ),
                state: SwarmTaskState::Pending,
                output: None,
                parsed_artifacts: None,
                expected_artifacts_missing: false,
                failed: false,
                retries: 0,
            });
        }

        if let Some(agent_id) = judge_agent.clone() {
            tasks.push(SwarmTask {
                id: "judge".into(),
                agent_id,
                role: Some("judge".into()),
                title: "Judge + select approach".into(),
                task_prompt: "Compare the proposer outputs and pick the best approach. Provide:\n- Decision (which proposal / why)\n- A step-by-step integration plan for the integrator\n- Acceptance criteria\n- Exact verification commands\n\nConstraints:\n- Do NOT edit the workspace (read-only).\n"
                    .into(),
                deps: proposer_task_ids.clone(),
                writes: false,
                artifacts: vec!["decision".into(), "plan".into(), "commands".into(), "risks".into()],
                done_when: Some("Integrator has a clear, actionable plan to implement.".into()),
                state: SwarmTaskState::Pending,
                output: None,
                parsed_artifacts: None,
                expected_artifacts_missing: false,
                failed: false,
                retries: 0,
            });
        }

        if let Some(agent_id) = integrator.clone() {
            tasks.push(SwarmTask {
                id: "integrate".into(),
                agent_id,
                role: Some("integrate".into()),
                title: "Integrate selected approach".into(),
                task_prompt: "Implement the selected approach using the judge output.\n\nConstraints:\n- You are the ONLY agent allowed to edit the workspace.\n- Prefer small, safe diffs.\n- For verification, follow the TEST DISCIPLINE in the role contract above (targeted only — no workspace-wide commands unless the operator explicitly asked).\n"
                    .into(),
                deps: vec!["judge".into()],
                writes: true,
                artifacts: Vec::new(),
                done_when: Some("Changes are implemented cleanly with validations passing.".into()),
                state: SwarmTaskState::Pending,
                output: None,
                parsed_artifacts: None,
                expected_artifacts_missing: false,
                failed: false,
                retries: 0,
            });
        }

        if let Some(agent_id) = review_agent {
            tasks.push(SwarmTask {
                id: "review".into(),
                agent_id,
                role: Some("review".into()),
                title: "Review final diff".into(),
                task_prompt: "Review the integrated changes for correctness, UX, and maintainability. Suggest follow-ups and edge cases.\n\nConstraints:\n- Do NOT edit the workspace (read-only).\n"
                    .into(),
                deps: vec!["integrate".into()],
                writes: false,
                artifacts: vec!["risks".into(), "commands".into()],
                done_when: Some("We have confidence in correctness and know remaining risks.".into()),
                state: SwarmTaskState::Pending,
                output: None,
                parsed_artifacts: None,
                expected_artifacts_missing: false,
                failed: false,
                retries: 0,
            });
        }

        let synth = plan_error.map(|err| {
            format!(
                "Note: planner output could not be used; fallback prompts were used. Reason: {err}"
            )
        });

        return ParsedSwarmPlan {
            tasks,
            synthesis_prompt: synth,
            integrator_agent_id: integrator,
            warnings: Vec::new(),
        };
    }
    if matches!(template, SwarmTemplate::Lab) {
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
        let recon_agent = available_agents
            .iter()
            .find(|id| integrator.as_ref() != Some(*id))
            .cloned()
            .or_else(|| integrator.clone());
        let design_agent = available_agents
            .iter()
            .find(|id| integrator.as_ref() != Some(*id) && recon_agent.as_ref() != Some(*id))
            .cloned()
            .or_else(|| recon_agent.clone())
            .or_else(|| integrator.clone());
        let review_agent = available_agents
            .iter()
            .find(|id| {
                integrator.as_ref() != Some(*id)
                    && recon_agent.as_ref() != Some(*id)
                    && design_agent.as_ref() != Some(*id)
            })
            .cloned()
            .or_else(|| design_agent.clone())
            .or_else(|| recon_agent.clone())
            .or_else(|| integrator.clone());

        let mut tasks = Vec::new();
        let research_mission = mission_kind.allows_research_roles();
        if let Some(agent_id) = recon_agent {
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
            tasks.push(SwarmTask {
                id: "recon".into(),
                agent_id,
                role,
                title,
                task_prompt,
                deps: Vec::new(),
                writes: false,
                artifacts,
                done_when,
                state: SwarmTaskState::Pending,
                output: None,
                parsed_artifacts: None,
                expected_artifacts_missing: false,
                failed: false,
                retries: 0,
            });
        }
        if let Some(agent_id) = design_agent {
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
            tasks.push(SwarmTask {
                id: "design".into(),
                agent_id,
                role,
                title,
                task_prompt,
                deps: Vec::new(),
                writes: false,
                artifacts,
                done_when,
                state: SwarmTaskState::Pending,
                output: None,
                parsed_artifacts: None,
                expected_artifacts_missing: false,
                failed: false,
                retries: 0,
            });
        }
        if let Some(agent_id) = integrator.clone() {
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
            tasks.push(SwarmTask {
                id: "implement".into(),
                agent_id,
                role: Some("integrate".into()),
                title,
                task_prompt,
                deps: vec!["recon".into(), "design".into()],
                writes,
                artifacts,
                done_when,
                state: SwarmTaskState::Pending,
                output: None,
                parsed_artifacts: None,
                expected_artifacts_missing: false,
                failed: false,
                retries: 0,
            });
        }
        if let Some(agent_id) = review_agent {
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
            tasks.push(SwarmTask {
                id: "review".into(),
                agent_id,
                role: Some("review".into()),
                title: "Review & verification".into(),
                task_prompt,
                deps: vec!["implement".into()],
                writes: false,
                artifacts,
                done_when,
                state: SwarmTaskState::Pending,
                output: None,
                parsed_artifacts: None,
                expected_artifacts_missing: false,
                failed: false,
                retries: 0,
            });
        }

        let synth = plan_error.map(|err| {
            format!(
                "Note: planner output could not be used; fallback prompts were used. Reason: {err}"
            )
        });

        return ParsedSwarmPlan {
            tasks,
            synthesis_prompt: synth,
            integrator_agent_id: integrator,
            warnings: Vec::new(),
        };
    }

    let mut tasks = Vec::new();
    let mut idx = 0usize;
    for (agent_idx, agent_id) in available_agents.iter().enumerate() {
        idx = idx.saturating_add(1);
        let (role, title, prompt, deps, writes) = match mission_kind {
            SwarmMissionKind::Research => match agent_idx {
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
            },
            SwarmMissionKind::ComputationalResearch => match agent_idx {
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
            },
            SwarmMissionKind::General => match (template, agent_idx) {
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
            },
        };

        let task_id = format!("task-{idx:02}");
        tasks.push(SwarmTask {
            id: task_id,
            agent_id: agent_id.clone(),
            role,
            title: title.into(),
            task_prompt: prompt.into(),
            deps,
            writes,
            artifacts: Vec::new(),
            done_when: None,
            state: SwarmTaskState::Pending,
            output: None,
            parsed_artifacts: None,
            expected_artifacts_missing: false,
            failed: false,
            retries: 0,
        });
    }

    let synth = plan_error.map(|err| {
        format!("Note: planner output could not be used; fallback prompts were used. Reason: {err}")
    });

    ParsedSwarmPlan {
        tasks,
        synthesis_prompt: synth,
        integrator_agent_id: None,
        warnings: Vec::new(),
    }
}
