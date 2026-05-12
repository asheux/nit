use std::collections::HashMap;

use super::budgets::{apply_prompt_budget, parse_override_token, PromptBudgets};
use super::constants::{
    PROMPT_BUDGET_DEFAULT, PROMPT_BUDGET_INTEGRATE, PROMPT_BUDGET_JUDGE, PROMPT_BUDGET_PROPOSE,
    PROMPT_BUDGET_RESEARCH, PROMPT_BUDGET_REVIEW, PROMPT_BUDGET_TEST,
};
use super::graph_exec::effective_dep_count_for_payload;
use super::types::{SwarmMissionKind, SwarmStage, SwarmTemplate};
use super::{SwarmRun, SwarmTask};

fn enabled_budgets() -> PromptBudgets {
    PromptBudgets::default()
}

fn disabled_budgets() -> PromptBudgets {
    PromptBudgets {
        tiers_enabled: false,
        ..PromptBudgets::default()
    }
}

fn empty_run() -> SwarmRun {
    SwarmRun {
        mission_id: "mis-budget".into(),
        root_prompt: String::new(),
        template: SwarmTemplate::Lab,
        mission_kind: SwarmMissionKind::General,
        spawn_cwd: std::path::PathBuf::from("."),
        planner_agent_id: "planner".into(),
        integrator_agent_id: Some("w".into()),
        integrator_locked: false,
        verifier_agent_id: None,
        gate_bundle: None,
        gate_custom: None,
        gate_selection: "auto:none".into(),
        agent_ids: vec!["planner".into(), "w".into()],
        stage: SwarmStage::Executing,
        tasks: Vec::new(),
        synthesis_prompt: None,
        gate_output: None,
        gate_report: None,
        genome_gate_results: None,
        genome_gate_pending: None,
        genome_review_pending: None,
        report_status: None,
        report_output: None,
        scope_files: Vec::new(),
        initial_genome_baselines: HashMap::new(),
        gate_retry_count: 0,
        repair_round: 0,
        last_plan_json: None,
        prior_violations: Vec::new(),
        prompt_budget_defaults: PromptBudgets::default(),
        prompt_budgets: HashMap::new(),
    }
}

fn integrate_task() -> SwarmTask {
    let mut task = SwarmTask::new_for_test("integrate", "w", Some("integrate"), Vec::new(), true);
    task.writes = true;
    task
}

fn integrate_prompt_with_checklist(checklist_files: &[&str], dep_payload_size: usize) -> String {
    let mut prompt = String::new();
    prompt.push_str("EXECUTION MODE: non-interactive\n");
    prompt.push_str("ROLE: integrate\n");
    prompt.push_str("ROLE CONTRACT:\n- Act strictly as the assigned role.\n");
    prompt.push_str("\nOperator request:\nimplement the change\n");
    prompt.push_str("\n## FILE CHECKLIST (non-negotiable)\n");
    for (i, path) in checklist_files.iter().enumerate() {
        prompt.push_str(&format!("{}. {path}\n", i + 1));
    }
    prompt.push_str("\nYour task:\nDo the work.\n");
    prompt.push_str("\n## IMPLEMENTATION PLAN (BINDING — follow verbatim)\n");
    prompt.push_str("\n---\nDEP: propose-01 [DONE] (agent a1)\n");
    prompt.push_str(&"p".repeat(dep_payload_size));
    prompt.push('\n');
    prompt.push_str("\n---\nDEP: propose-02 [DONE] (agent a2)\n");
    prompt.push_str(&"q".repeat(dep_payload_size));
    prompt.push('\n');
    prompt.push_str("\n---\nDEP: judge [DONE] (agent a3)\n");
    prompt.push_str(&"j".repeat(dep_payload_size));
    prompt.push('\n');
    prompt.push_str("\nRespond with:\n- Findings\n");
    prompt.push_str("\n## SIGN-OFF (REQUIRED)\n<SWARM_TASK_COMPLETE>\n");
    prompt
}

#[test]
fn for_role_matrix_covers_all_canonical_roles() {
    let b = enabled_budgets();
    assert_eq!(
        b.for_role(Some("integrate"), false),
        PROMPT_BUDGET_INTEGRATE
    );
    assert_eq!(b.for_role(Some("judge"), false), PROMPT_BUDGET_JUDGE);
    assert_eq!(b.for_role(Some("propose"), false), PROMPT_BUDGET_PROPOSE);
    assert_eq!(b.for_role(Some("review"), false), PROMPT_BUDGET_REVIEW);
    assert_eq!(b.for_role(Some("test"), false), PROMPT_BUDGET_TEST);
    assert_eq!(b.for_role(Some("research"), false), PROMPT_BUDGET_RESEARCH);
    assert_eq!(
        b.for_role(Some("computational-research"), false),
        PROMPT_BUDGET_RESEARCH
    );

    assert_eq!(
        b.for_role(Some("custom-writer"), true),
        PROMPT_BUDGET_INTEGRATE,
        "unknown role with writes inherits the integrate ceiling"
    );
    assert_eq!(
        b.for_role(Some("custom-reader"), false),
        PROMPT_BUDGET_DEFAULT,
        "unknown read-only role falls through to default"
    );
    assert_eq!(b.for_role(None, true), PROMPT_BUDGET_INTEGRATE);
    assert_eq!(b.for_role(None, false), PROMPT_BUDGET_DEFAULT);
}

#[test]
fn effective_budget_consults_per_mission_overrides_first() {
    let b = enabled_budgets();
    let mut overrides: HashMap<String, usize> = HashMap::new();
    overrides.insert("integrate".into(), 99_999);
    assert_eq!(
        b.effective_budget(Some("integrate"), true, &overrides),
        99_999
    );
    assert_eq!(
        b.effective_budget(Some("judge"), false, &overrides),
        PROMPT_BUDGET_JUDGE,
        "unrelated roles fall back to runtime defaults"
    );
}

#[test]
fn apply_prompt_budget_preserves_file_checklist_under_pressure() {
    let checklist: Vec<String> = (0..72)
        .map(|i| format!("crates/foo/file_{i:02}.rs"))
        .collect();
    let checklist_refs: Vec<&str> = checklist.iter().map(String::as_str).collect();
    let mut prompt = integrate_prompt_with_checklist(&checklist_refs, 200_000);
    assert!(prompt.len() > PROMPT_BUDGET_INTEGRATE);

    let task = integrate_task();
    let _ = apply_prompt_budget(&mut prompt, PROMPT_BUDGET_INTEGRATE, &task, 3);

    assert!(prompt.contains("## FILE CHECKLIST"));
    for path in &checklist {
        assert!(
            prompt.contains(path),
            "expected FILE CHECKLIST entry `{path}` to survive truncation"
        );
    }
    assert!(prompt.contains("ROLE CONTRACT"));
    assert!(prompt.contains("Operator request:"));
    assert!(prompt.contains("<SWARM_TASK_COMPLETE>"));
}

#[test]
fn apply_prompt_budget_three_stage_order() {
    let checklist: Vec<String> = (0..5).map(|i| format!("file_{i}.rs")).collect();
    let refs: Vec<&str> = checklist.iter().map(String::as_str).collect();

    let starting = integrate_prompt_with_checklist(&refs, 250_000);
    let task = integrate_task();

    let mut stage1 = starting.clone();
    let _ = apply_prompt_budget(&mut stage1, 200_000, &task, 3);
    assert!(
        stage1.contains("truncated under prompt budget"),
        "Stage 1 should leave per-dep truncation markers"
    );
    assert!(
        stage1.contains("DEP: propose-01"),
        "Stage 1 alone should be enough at 200KB budget — proposer block kept (header), only shrunk"
    );

    let mut stage2 = starting.clone();
    let _ = apply_prompt_budget(&mut stage2, 60_000, &task, 3);
    assert!(
        stage2.contains("Dropped propose dep payload"),
        "Stage 2 should replace proposer payloads with a breadcrumb when Stage 1 alone is insufficient"
    );
    assert!(
        stage2.contains("DEP: judge"),
        "Stage 2 must not drop the judge dep until Stage 1+2A cannot finish the cut"
    );
}

#[test]
fn nit_prompt_tiers_zero_is_byte_identical() {
    let disabled = disabled_budgets();
    let budget = disabled.for_role(Some("integrate"), true);
    assert_eq!(budget, usize::MAX, "disabled tiers must return MAX");

    let task = integrate_task();
    let original = integrate_prompt_with_checklist(&["a.rs", "b.rs"], 100_000);
    let mut prompt = original.clone();
    let diag = apply_prompt_budget(&mut prompt, budget, &task, 0);
    assert!(diag.is_none());
    assert_eq!(
        prompt, original,
        "apply_prompt_budget must be a no-op when budget == usize::MAX"
    );
}

#[test]
fn pool_vanilla_propose_fits_under_budget() {
    let prompt = synthesize_pool_vanilla_propose_prompt();
    assert!(
        prompt.len() <= PROMPT_BUDGET_PROPOSE,
        "pool-eligible vanilla propose prompts must stay under {} bytes (got {})",
        PROMPT_BUDGET_PROPOSE,
        prompt.len()
    );
}

fn synthesize_pool_vanilla_propose_prompt() -> String {
    let mut s = String::new();
    s.push_str("SWARM TASK: propose strategy\nROLE: propose\n");
    s.push_str("EXECUTION MODE: non-interactive\n");
    s.push_str("ROLE CONTRACT:\n- Act strictly as the assigned role.\n");
    s.push_str("Operator request:\ndesign 9 strategies\n");
    s.push_str(&format!(
        "\n## SCOPE — files\n{}\n",
        "- file.rs\n".repeat(50)
    ));
    s.push_str("\nYour task:\npropose\n");
    s.push_str(&"\nrole_contract padding line".repeat(200));
    s.push_str("\n## STRUCTURED ARTIFACTS\n```json\n{}\n```\n");
    s.push_str("\n## SIGN-OFF\n<SWARM_TASK_COMPLETE>\n");
    s
}

#[test]
fn effective_dep_count_respects_judge_skip() {
    let mut run = empty_run();
    let propose_a = SwarmTask::new_for_test("propose-a", "a", Some("propose"), Vec::new(), false);
    let propose_b = SwarmTask::new_for_test("propose-b", "b", Some("propose"), Vec::new(), false);
    let mut judge = SwarmTask::new_for_test(
        "judge",
        "j",
        Some("judge"),
        vec!["propose-a".into(), "propose-b".into()],
        false,
    );
    judge.set_parsed_artifacts_for_test(super::SwarmTaskArtifacts {
        summary: Some("picked".into()),
        ..Default::default()
    });
    let mut integrate = SwarmTask::new_for_test(
        "integrate",
        "w",
        Some("integrate"),
        vec!["propose-a".into(), "propose-b".into(), "judge".into()],
        true,
    );
    integrate.writes = true;
    run.tasks = vec![propose_a, propose_b, judge, integrate];

    let task = run.tasks.iter().find(|t| t.id == "integrate").unwrap();
    let n = effective_dep_count_for_payload(&run, task);
    assert_eq!(
        n, 1,
        "judge-with-artifacts skip must drop both proposer deps from the count"
    );
}

#[test]
fn parse_override_token_handles_decimal_and_kilo_suffix() {
    let (role, n) = parse_override_token("integrate:600000").expect("ok");
    assert_eq!(role, "integrate");
    assert_eq!(n, 600_000);

    let (role, n) = parse_override_token("judge:240k").expect("ok with k suffix");
    assert_eq!(role, "judge");
    assert_eq!(n, 240 * 1024);

    assert!(parse_override_token("budget").is_err());
    assert!(parse_override_token("garbage:foo").is_err());
    assert!(parse_override_token(":100").is_err());

    let (role, n) =
        parse_override_token("customrole:100").expect("unknown role accepted as canonical");
    assert_eq!(role, "customrole");
    assert_eq!(n, 100);
}
