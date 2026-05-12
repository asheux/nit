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
