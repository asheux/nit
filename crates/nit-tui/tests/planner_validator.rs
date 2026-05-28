use std::collections::HashMap;

use nit_tui::swarm::validator::{validate_plan, Severity, ValidationContext, Violation};
use nit_tui::swarm::{SwarmMissionKind, SwarmTask, SwarmTemplate};

fn task(id: &str, agent: &str, role: &str, deps: &[&str]) -> SwarmTask {
    let role_opt = if role.is_empty() { None } else { Some(role) };
    let dep_strings: Vec<String> = deps.iter().map(|s| (*s).to_string()).collect();
    let writes = role == "integrate";
    SwarmTask::new_for_test(id, agent, role_opt, dep_strings, writes)
}

fn agents(slugs: &[&str]) -> Vec<String> {
    slugs.iter().map(|s| (*s).to_string()).collect()
}

fn has_violation_id(violations: &[Violation], substring: &str) -> bool {
    violations.iter().any(|v| v.id.contains(substring))
}

#[test]
fn role_hint_violation_is_caught() {
    // The roster pinned `agent-judge` to the `judge` role, but the planner
    // assigned that agent a propose task and emitted no judge task at all.
    // The validator must surface a `role_hint_honored` violation naming the
    // agent so the repair prompt knows what to fix.
    let mut hints: HashMap<String, String> = HashMap::new();
    hints.insert("agent-judge".to_string(), "judge".to_string());
    let tasks = vec![
        task("propose-01", "agent-prop", "propose", &[]),
        task("propose-02", "agent-judge", "propose", &[]),
        task(
            "integrate",
            "agent-int",
            "integrate",
            &["propose-01", "propose-02"],
        ),
    ];
    let available = agents(&["agent-prop", "agent-judge", "agent-int"]);
    let ctx = ValidationContext {
        tasks: &tasks,
        available_agents: &available,
        integrator_agent_id: Some("agent-int"),
        role_hints: &hints,
        template: SwarmTemplate::Lab,
        mission_kind: SwarmMissionKind::General,
        root_prompt: "rewrite the swarm planner",
        intent: nit_tui::swarm::intent::OperatorIntent::default(),
    };
    let violations = validate_plan(&ctx);
    let role_hits: Vec<&Violation> = violations
        .iter()
        .filter(|v| v.id.contains("role_hint_honored"))
        .collect();
    assert!(
        role_hits
            .iter()
            .any(|v| v.agent_id.as_deref() == Some("agent-judge")),
        "expected role_hint_honored violation naming agent-judge; got {violations:?}"
    );
    assert!(role_hits
        .iter()
        .all(|v| matches!(v.severity, Severity::MustFix)));
}

#[test]
fn missing_integrate_on_code_change_request_is_caught() {
    // Operator request is a code-change ("implement ..."), mission kind is
    // General, and the plan has propose + judge but NO integrate task. The
    // validator must surface `integrate_present_for_code_change`.
    let tasks = vec![
        task("propose-01", "agent-a", "propose", &[]),
        task("propose-02", "agent-b", "propose", &[]),
        task("judge", "agent-j", "judge", &["propose-01", "propose-02"]),
    ];
    let available = agents(&["agent-a", "agent-b", "agent-j", "agent-int"]);
    let hints: HashMap<String, String> = HashMap::new();
    let ctx = ValidationContext {
        tasks: &tasks,
        available_agents: &available,
        integrator_agent_id: Some("agent-int"),
        role_hints: &hints,
        template: SwarmTemplate::Bulk,
        mission_kind: SwarmMissionKind::General,
        root_prompt: "implement the new planner architecture end-to-end",
        intent: nit_tui::swarm::intent::OperatorIntent::default(),
    };
    let violations = validate_plan(&ctx);
    assert!(
        has_violation_id(&violations, "integrate_present_for_code_change"),
        "expected integrate_present_for_code_change violation; got {violations:?}"
    );
    let viol = violations
        .iter()
        .find(|v| v.id.contains("integrate_present_for_code_change"))
        .unwrap();
    assert!(matches!(viol.severity, Severity::MustFix));
}

#[test]
fn duplicate_judge_tasks_are_caught() {
    // Two judge tasks violates the singleton invariant. The plan also has
    // an integrate task that depends on both judges so the rest of the DAG
    // is still well-formed — the violation should be exactly singleton_judge.
    let tasks = vec![
        task("propose-01", "agent-a", "propose", &[]),
        task("propose-02", "agent-b", "propose", &[]),
        task("judge-1", "agent-j", "judge", &["propose-01", "propose-02"]),
        task("judge-2", "agent-k", "judge", &["propose-01", "propose-02"]),
        task(
            "integrate",
            "agent-int",
            "integrate",
            &["judge-1", "judge-2"],
        ),
    ];
    let available = agents(&["agent-a", "agent-b", "agent-j", "agent-k", "agent-int"]);
    let hints: HashMap<String, String> = HashMap::new();
    let ctx = ValidationContext {
        tasks: &tasks,
        available_agents: &available,
        integrator_agent_id: Some("agent-int"),
        role_hints: &hints,
        template: SwarmTemplate::Bulk,
        mission_kind: SwarmMissionKind::General,
        root_prompt: "rewrite the swarm planner architecture",
        intent: nit_tui::swarm::intent::OperatorIntent::default(),
    };
    let violations = validate_plan(&ctx);
    assert!(
        has_violation_id(&violations, "singleton_judge"),
        "expected singleton_judge violation; got {violations:?}"
    );
}

#[test]
fn clean_bulk_plan_has_no_must_fix_violations() {
    // Sanity check: a well-shaped bulk plan with two proposers, one judge,
    // and one integrate task on the integrator must pass with no MustFix
    // violations.
    let tasks = vec![
        task("propose-01", "agent-a", "propose", &[]),
        task("propose-02", "agent-b", "propose", &[]),
        task("judge", "agent-j", "judge", &["propose-01", "propose-02"]),
        task("integrate", "agent-int", "integrate", &["judge"]),
    ];
    let available = agents(&["agent-a", "agent-b", "agent-j", "agent-int"]);
    let hints: HashMap<String, String> = HashMap::new();
    let ctx = ValidationContext {
        tasks: &tasks,
        available_agents: &available,
        integrator_agent_id: Some("agent-int"),
        role_hints: &hints,
        template: SwarmTemplate::Bulk,
        mission_kind: SwarmMissionKind::General,
        root_prompt: "implement architecture G",
        intent: nit_tui::swarm::intent::OperatorIntent::default(),
    };
    let violations = validate_plan(&ctx);
    let must_fix: Vec<&Violation> = violations
        .iter()
        .filter(|v| matches!(v.severity, Severity::MustFix))
        .collect();
    assert!(
        must_fix.is_empty(),
        "expected no MustFix violations on a clean plan; got {must_fix:?}"
    );
}

#[test]
fn parallel_under_fanout_with_structured_intent_is_must_fix() {
    // Nine-ticket prompt + parallel template + plan with 1 integrator
    // → INV-17 fires as MustFix. This is the production scenario the
    // intent-aware validator was built for.
    let tasks = vec![
        task("recon", "agent-recon", "propose", &[]),
        task("integrate", "agent-int", "integrate", &["recon"]),
    ];
    let available = agents(&[
        "agent-recon",
        "agent-int",
        "agent-c",
        "agent-d",
        "agent-e",
        "agent-f",
        "agent-g",
        "agent-h",
        "agent-i",
        "agent-j",
        "agent-k",
    ]);
    let hints: HashMap<String, String> = HashMap::new();
    let prompt = "Tickets:\n\
        - T1 bracket highlight\n\
        - T2 percent motion\n\
        - T3 jumplist\n\
        - T4 sticky preferred col\n\
        - T5 visual indent\n\
        - T6 python smart enter\n\
        - T7 undo refactor\n\
        - T8 smart backspace\n\
        - T9 ctrl-c copy\n";
    let intent = nit_tui::swarm::intent::detect_intent(prompt);
    assert_eq!(
        intent.ticket_count,
        Some(9),
        "intent detector should find 9"
    );
    let ctx = ValidationContext {
        tasks: &tasks,
        available_agents: &available,
        integrator_agent_id: Some("agent-int"),
        role_hints: &hints,
        template: SwarmTemplate::Parallel,
        mission_kind: SwarmMissionKind::General,
        root_prompt: prompt,
        intent,
    };
    let violations = validate_plan(&ctx);
    let inv17: Vec<&Violation> = violations
        .iter()
        .filter(|v| v.id.contains("parallel_min_integrators"))
        .collect();
    assert!(
        !inv17.is_empty(),
        "expected INV-17 violation; got {violations:?}"
    );
    assert!(matches!(inv17[0].severity, Severity::MustFix));
    // Floor should be min(9, writer_budget=11, hard_cap=12) = 9
    assert!(
        inv17[0].human.contains("≥ 9") || inv17[0].human.contains(">= 9"),
        "violation should cite the floor of 9; got: {}",
        inv17[0].human
    );
}

#[test]
fn parallel_meeting_floor_passes_invariant() {
    // 9 tickets + 11 writers + 9 integrators in the plan → no INV-17.
    let mut tasks = vec![task("recon", "agent-recon", "propose", &[])];
    for n in 1..=9 {
        let id = format!("integrate-{n:02}");
        let agent = format!("agent-int-{n:02}");
        tasks.push(task(&id, &agent, "integrate", &["recon"]));
    }
    let mut available = vec!["agent-recon".to_string()];
    for n in 1..=9 {
        available.push(format!("agent-int-{n:02}"));
    }
    available.push("agent-test".into());
    let hints: HashMap<String, String> = HashMap::new();
    let prompt = "Tickets:\n- a\n- b\n- c\n- d\n- e\n- f\n- g\n- h\n- i\n";
    let intent = nit_tui::swarm::intent::detect_intent(prompt);
    let ctx = ValidationContext {
        tasks: &tasks,
        available_agents: &available,
        integrator_agent_id: Some("agent-int-01"),
        role_hints: &hints,
        template: SwarmTemplate::Parallel,
        mission_kind: SwarmMissionKind::General,
        root_prompt: prompt,
        intent,
    };
    let violations = validate_plan(&ctx);
    assert!(
        !has_violation_id(&violations, "parallel_min_integrators"),
        "expected no INV-17 with 9 integrators; got {violations:?}"
    );
}

#[test]
fn parallel_over_capacity_intent_triggers_bundling_hint() {
    // Operator named 30 tickets but only 8 writer slots. Plan with 1
    // integrator → INV-17 fires; the hint must mention "BUNDLE" so
    // the planner groups tickets instead of generating 30 integrators.
    let mut bullets = String::from("Tickets:\n");
    for n in 1..=30 {
        bullets.push_str(&format!("- ticket {n}\n"));
    }
    let intent = nit_tui::swarm::intent::detect_intent(&bullets);
    assert_eq!(intent.ticket_count, Some(30));

    let tasks = vec![
        task("recon", "agent-recon", "propose", &[]),
        task("integrate", "agent-int", "integrate", &["recon"]),
    ];
    let available = agents(&[
        "agent-recon",
        "agent-int",
        "agent-c",
        "agent-d",
        "agent-e",
        "agent-f",
        "agent-g",
        "agent-h",
    ]);
    let hints: HashMap<String, String> = HashMap::new();
    let ctx = ValidationContext {
        tasks: &tasks,
        available_agents: &available,
        integrator_agent_id: Some("agent-int"),
        role_hints: &hints,
        template: SwarmTemplate::Parallel,
        mission_kind: SwarmMissionKind::General,
        root_prompt: &bullets,
        intent,
    };
    let violations = validate_plan(&ctx);
    let inv17: Vec<&Violation> = violations
        .iter()
        .filter(|v| v.id.contains("parallel_min_integrators"))
        .collect();
    assert!(!inv17.is_empty(), "expected INV-17; got {violations:?}");
    assert!(
        inv17[0].hint.to_ascii_uppercase().contains("BUNDLE"),
        "over-capacity hint must mention BUNDLE; got: {}",
        inv17[0].hint
    );
    // Floor should cap at writer budget (8), not raw ticket count (30).
    assert!(
        inv17[0].human.contains("≥ 8") || inv17[0].human.contains(">= 8"),
        "floor should cap at writer budget of 8; got: {}",
        inv17[0].human
    );
}

#[test]
fn parallel_ambiguous_intent_yields_advisory_not_must_fix() {
    // Prose prompt with no ticket structure → INV-17 still computes a
    // floor but as an Advisory (planner is informed, not forced).
    let tasks = vec![
        task("recon", "agent-recon", "propose", &[]),
        task("integrate", "agent-int", "integrate", &["recon"]),
    ];
    let available = agents(&[
        "agent-recon",
        "agent-int",
        "agent-c",
        "agent-d",
        "agent-e",
        "agent-f",
    ]);
    let hints: HashMap<String, String> = HashMap::new();
    let prompt = "Refactor the editor to be more vim-like. Fix the bugs. \
                  Make it feel smart.";
    let intent = nit_tui::swarm::intent::detect_intent(prompt);
    assert_eq!(intent.ticket_count, None);
    let ctx = ValidationContext {
        tasks: &tasks,
        available_agents: &available,
        integrator_agent_id: Some("agent-int"),
        role_hints: &hints,
        template: SwarmTemplate::Parallel,
        mission_kind: SwarmMissionKind::General,
        root_prompt: prompt,
        intent,
    };
    let violations = validate_plan(&ctx);
    let inv17: Vec<&Violation> = violations
        .iter()
        .filter(|v| v.id.contains("parallel_min_integrators"))
        .collect();
    assert_eq!(inv17.len(), 1);
    assert!(
        matches!(inv17[0].severity, Severity::Advisory),
        "ambiguous-intent violation must be Advisory; got {:?}",
        inv17[0].severity
    );
}

#[test]
fn lab_template_skips_parallel_fanout_invariant() {
    // Lab template intentionally has 1 integrator; INV-17 only applies
    // to Parallel.
    let tasks = vec![
        task("propose-01", "agent-a", "propose", &[]),
        task("judge", "agent-j", "judge", &["propose-01"]),
        task("integrate", "agent-int", "integrate", &["judge"]),
    ];
    let available = agents(&[
        "agent-a",
        "agent-j",
        "agent-int",
        "agent-d",
        "agent-e",
        "agent-f",
    ]);
    let hints: HashMap<String, String> = HashMap::new();
    let prompt = "Tickets:\n- one\n- two\n- three\n- four\n- five\n- six\n";
    let intent = nit_tui::swarm::intent::detect_intent(prompt);
    let ctx = ValidationContext {
        tasks: &tasks,
        available_agents: &available,
        integrator_agent_id: Some("agent-int"),
        role_hints: &hints,
        template: SwarmTemplate::Lab,
        mission_kind: SwarmMissionKind::General,
        root_prompt: prompt,
        intent,
    };
    let violations = validate_plan(&ctx);
    assert!(
        !has_violation_id(&violations, "parallel_min_integrators"),
        "lab plan must not trip INV-17; got {violations:?}"
    );
}

#[test]
fn parallel_single_proposer_no_judge_is_clean() {
    // The small-fanout shape: one recon proposer + N integrators, no judge.
    // INV-18 must NOT fire — it only kicks in once the planner emits ≥2
    // propose tasks under Parallel.
    let mut tasks = vec![task("recon", "agent-recon", "propose", &[])];
    for n in 1..=3 {
        let id = format!("integrate-{n:02}");
        let agent = format!("agent-int-{n:02}");
        tasks.push(task(&id, &agent, "integrate", &["recon"]));
    }
    let available = agents(&[
        "agent-recon",
        "agent-int-01",
        "agent-int-02",
        "agent-int-03",
    ]);
    let hints: HashMap<String, String> = HashMap::new();
    let prompt = "Tickets:\n- a\n- b\n- c\n";
    let intent = nit_tui::swarm::intent::detect_intent(prompt);
    let ctx = ValidationContext {
        tasks: &tasks,
        available_agents: &available,
        integrator_agent_id: Some("agent-int-01"),
        role_hints: &hints,
        template: SwarmTemplate::Parallel,
        mission_kind: SwarmMissionKind::General,
        root_prompt: prompt,
        intent,
    };
    let violations = validate_plan(&ctx);
    assert!(
        !has_violation_id(&violations, "parallel_multi_proposer_needs_judge"),
        "single-proposer parallel plan must not trip INV-18; got {violations:?}"
    );
}

#[test]
fn parallel_multi_proposer_without_judge_is_must_fix() {
    // The planner emitted 3 lens proposers but forgot the judge. INV-18
    // must surface MustFix so the repair loop catches it before dispatch
    // — without a judge, integrators consume raw multi-lens output and
    // the upgraded shape's whole point (synthesised guidance) is lost.
    let tasks = vec![
        task("propose-01", "agent-p1", "propose", &[]),
        task("propose-02", "agent-p2", "propose", &[]),
        task("propose-03", "agent-p3", "propose", &[]),
        task(
            "integrate-01",
            "agent-int-01",
            "integrate",
            &["propose-01", "propose-02", "propose-03"],
        ),
        task(
            "integrate-02",
            "agent-int-02",
            "integrate",
            &["propose-01", "propose-02", "propose-03"],
        ),
        task(
            "integrate-03",
            "agent-int-03",
            "integrate",
            &["propose-01", "propose-02", "propose-03"],
        ),
    ];
    let available = agents(&[
        "agent-p1",
        "agent-p2",
        "agent-p3",
        "agent-int-01",
        "agent-int-02",
        "agent-int-03",
    ]);
    let hints: HashMap<String, String> = HashMap::new();
    let prompt = "Tickets:\n- a\n- b\n- c\n";
    let intent = nit_tui::swarm::intent::detect_intent(prompt);
    let ctx = ValidationContext {
        tasks: &tasks,
        available_agents: &available,
        integrator_agent_id: Some("agent-int-01"),
        role_hints: &hints,
        template: SwarmTemplate::Parallel,
        mission_kind: SwarmMissionKind::General,
        root_prompt: prompt,
        intent,
    };
    let violations = validate_plan(&ctx);
    let inv18: Vec<&Violation> = violations
        .iter()
        .filter(|v| v.id.contains("parallel_multi_proposer_needs_judge"))
        .collect();
    assert_eq!(inv18.len(), 1, "expected INV-18; got {violations:?}");
    assert!(matches!(inv18[0].severity, Severity::MustFix));
    // Hint should mention every proposer id so the planner knows what to wire.
    assert!(
        inv18[0].hint.contains("propose-01")
            && inv18[0].hint.contains("propose-02")
            && inv18[0].hint.contains("propose-03"),
        "hint should enumerate proposer ids; got: {}",
        inv18[0].hint
    );
}

#[test]
fn parallel_multi_proposer_with_judge_is_clean() {
    // The full upgraded shape: 3 lens proposers + judge + N integrators
    // (deps=[judge]). INV-18 must be silent.
    let tasks = vec![
        task("propose-01", "agent-p1", "propose", &[]),
        task("propose-02", "agent-p2", "propose", &[]),
        task("propose-03", "agent-p3", "propose", &[]),
        task(
            "judge",
            "agent-judge",
            "judge",
            &["propose-01", "propose-02", "propose-03"],
        ),
        task("integrate-01", "agent-int-01", "integrate", &["judge"]),
        task("integrate-02", "agent-int-02", "integrate", &["judge"]),
        task("integrate-03", "agent-int-03", "integrate", &["judge"]),
    ];
    let available = agents(&[
        "agent-p1",
        "agent-p2",
        "agent-p3",
        "agent-judge",
        "agent-int-01",
        "agent-int-02",
        "agent-int-03",
    ]);
    let hints: HashMap<String, String> = HashMap::new();
    let prompt = "Tickets:\n- a\n- b\n- c\n";
    let intent = nit_tui::swarm::intent::detect_intent(prompt);
    let ctx = ValidationContext {
        tasks: &tasks,
        available_agents: &available,
        integrator_agent_id: Some("agent-int-01"),
        role_hints: &hints,
        template: SwarmTemplate::Parallel,
        mission_kind: SwarmMissionKind::General,
        root_prompt: prompt,
        intent,
    };
    let violations = validate_plan(&ctx);
    assert!(
        !has_violation_id(&violations, "parallel_multi_proposer_needs_judge"),
        "upgraded parallel shape must not trip INV-18; got {violations:?}"
    );
}

#[test]
fn lab_multi_proposer_without_judge_skips_inv18() {
    // INV-18 is parallel-template-only. A lab plan with 2 proposers
    // and no judge is a different invariant's concern (and lab's own
    // singleton rules), not INV-18.
    let tasks = vec![
        task("propose-01", "agent-p1", "propose", &[]),
        task("propose-02", "agent-p2", "propose", &[]),
        task(
            "integrate",
            "agent-int",
            "integrate",
            &["propose-01", "propose-02"],
        ),
    ];
    let available = agents(&["agent-p1", "agent-p2", "agent-int"]);
    let hints: HashMap<String, String> = HashMap::new();
    let intent = nit_tui::swarm::intent::OperatorIntent::default();
    let ctx = ValidationContext {
        tasks: &tasks,
        available_agents: &available,
        integrator_agent_id: Some("agent-int"),
        role_hints: &hints,
        template: SwarmTemplate::Lab,
        mission_kind: SwarmMissionKind::General,
        root_prompt: "refactor x",
        intent,
    };
    let violations = validate_plan(&ctx);
    assert!(
        !has_violation_id(&violations, "parallel_multi_proposer_needs_judge"),
        "lab plan must not trip INV-18; got {violations:?}"
    );
}
