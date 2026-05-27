use std::collections::{HashMap, HashSet};

use super::intent::OperatorIntent;
use super::{analyze_swarm_dag, normalize_role_label, SwarmMissionKind, SwarmTask, SwarmTemplate};

/// Severity of a structural defect in a parsed plan.
///
/// `MustFix` triggers a bounded LLM repair loop; `Advisory` is recorded but
/// does not block dispatch. New invariants ship as `Advisory` until the field
/// trace shows they fire cleanly on real plans — then they get promoted.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Severity {
    MustFix,
    Advisory,
}

/// A single structural defect found in a parsed plan.
///
/// `id` is the stable invariant identifier and the only field the
/// `RepairOutcome` monotonicity guard keys off. `human` is operator-facing
/// prose; `hint` is what we splice into the repair prompt as the exact
/// instruction to the planner.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Violation {
    pub id: &'static str,
    pub task_id: Option<String>,
    pub agent_id: Option<String>,
    pub severity: Severity,
    pub human: String,
    pub hint: String,
}

impl Violation {
    pub(crate) fn signature(&self) -> (String, Option<String>) {
        (self.id.to_string(), self.task_id.clone())
    }
}

/// Read-only projection of everything the invariants need from a parsed plan.
///
/// Keeping this as a borrowed view means the validator never owns plan data
/// and never mutates it — `runtime_events` builds the context, calls
/// `validate_plan`, and decides what to do based on the returned `Vec`.
pub struct ValidationContext<'a> {
    pub tasks: &'a [SwarmTask],
    pub available_agents: &'a [String],
    pub integrator_agent_id: Option<&'a str>,
    pub role_hints: &'a HashMap<String, String>,
    pub template: SwarmTemplate,
    pub mission_kind: SwarmMissionKind,
    pub root_prompt: &'a str,
    /// What the operator's prompt seems to be asking for (ticket
    /// count / structured-list flag). Drives the parallel-template
    /// `INV-17 parallel_min_integrators` invariant — without this
    /// the planner's consolidation prior wins and operators get
    /// under-fanned plans.
    pub intent: OperatorIntent,
}

const BULK_PROPOSER_HARD_CAP: usize = 12;

/// Runs every invariant once and concatenates their `Violation`s. The order
/// of invariants matches the judge's plan — keep new ones at the bottom of
/// the list so existing field traces remain interpretable.
pub fn validate_plan(ctx: &ValidationContext<'_>) -> Vec<Violation> {
    let mut violations = Vec::new();
    violations.extend(invariant_nonempty_tasks(ctx));
    violations.extend(invariant_unique_task_ids(ctx));
    violations.extend(invariant_agent_ids_allowed(ctx));
    violations.extend(invariant_singleton_judge(ctx));
    violations.extend(invariant_singleton_integrate(ctx));
    violations.extend(invariant_writes_only_on_integrator(ctx));
    violations.extend(invariant_integrator_assignment(ctx));
    violations.extend(invariant_judge_depends_on_all_proposers(ctx));
    violations.extend(invariant_integrate_depends_on_judge(ctx));
    violations.extend(invariant_no_unknown_deps(ctx));
    violations.extend(invariant_acyclic(ctx));
    violations.extend(invariant_integrate_present_for_code_change(ctx));
    violations.extend(invariant_min_proposers(ctx));
    violations.extend(invariant_role_hint_honored(ctx));
    violations.extend(invariant_no_proposer_to_proposer_dep(ctx));
    violations.extend(invariant_artifacts_field_shape(ctx));
    violations.extend(invariant_bulk_max_proposers(ctx));
    violations.extend(invariant_parallel_min_integrators(ctx));
    violations
}

/// Hard upper bound on parallel integrate-task count. Matches
/// `BULK_PROPOSER_HARD_CAP` — beyond ~12 simultaneous writers we hit
/// file-descriptor / prompt-budget ceilings and the marginal value of
/// each additional integrator drops below the dispatch overhead.
/// When operator intent exceeds this, the planner is told to GROUP
/// related tickets into fewer integrators (each handling a bundle).
const PARALLEL_INTEGRATE_HARD_CAP: usize = 12;

/// Minimum integrate-task count for the `parallel` template given an
/// available-agents budget and a (possibly absent) operator intent
/// signal.
///
/// Rule:
///   * Raw floor = operator's ticket_count (or `ceil(writer_budget/2)`
///     if intent is ambiguous).
///   * Capped at `min(writer_budget, HARD_CAP)` so a runaway prompt
///     can't request more integrators than the swarm can physically
///     dispatch in parallel.
///   * When the raw floor is capped, the planner is responsible for
///     grouping related tickets into single integrators — that
///     guidance lives in the planner prompt and the
///     `INV-17 parallel_min_integrators` violation hint.
///
/// `available_writer_count` excludes the planner agent itself.
pub fn parallel_integrate_floor(intent: &OperatorIntent, available_writer_count: usize) -> usize {
    let raw = if let Some(n) = intent.ticket_count {
        n.max(2)
    } else {
        available_writer_count.div_ceil(2).max(2)
    };
    let ceiling = available_writer_count.clamp(1, PARALLEL_INTEGRATE_HARD_CAP);
    raw.min(ceiling)
}

/// `true` when the operator's intent exceeds what we'll cap to in
/// practice — the planner needs to BUNDLE tickets across integrators
/// rather than aim for one-per-ticket. Surfaces in the planner prompt
/// so the planner emits sensible groupings the first time.
pub fn parallel_intent_exceeds_capacity(
    intent: &OperatorIntent,
    available_writer_count: usize,
) -> bool {
    let Some(n) = intent.ticket_count else {
        return false;
    };
    let ceiling = available_writer_count.clamp(1, PARALLEL_INTEGRATE_HARD_CAP);
    n > ceiling
}

fn invariant_parallel_min_integrators(ctx: &ValidationContext<'_>) -> Vec<Violation> {
    if !matches!(ctx.template, SwarmTemplate::Parallel) {
        return Vec::new();
    }
    let writer_budget = ctx.available_agents.len();
    let floor = parallel_integrate_floor(&ctx.intent, writer_budget);
    let over_capacity = parallel_intent_exceeds_capacity(&ctx.intent, writer_budget);
    let integrate_count = integrate_tasks(ctx).len();
    if integrate_count >= floor {
        return Vec::new();
    }
    let intent_hint = match ctx.intent.ticket_count {
        Some(n) if over_capacity => format!(
            "Operator intent: {n} distinct tickets, exceeds writer capacity \
             ({writer_budget}); plan should BUNDLE related tickets into \
             {floor} integrators."
        ),
        Some(n) => format!("Operator intent: {n} distinct tickets detected in the prompt."),
        None => format!(
            "No explicit ticket count detected; floor derived from \
             half the available writer budget ({writer_budget})."
        ),
    };
    // Severity is MustFix only when intent is structurally confident
    // (≥ 3 list items detected). Ambiguous-intent plans get an
    // Advisory so the planner is informed but the dispatch doesn't
    // loop indefinitely on unstructured prompts.
    let severity = if ctx.intent.structured_list {
        Severity::MustFix
    } else {
        Severity::Advisory
    };
    let hint = if over_capacity {
        format!(
            "Produce exactly {floor} `role=integrate` tasks, each with \
             `writes=true`. Operator intent exceeds capacity, so BUNDLE \
             related tickets into integrators: group by shared file scope \
             first, shared module second, shared domain last. Each \
             integrator's `task_prompt` MUST quote all tickets in its \
             bundle. Available writer agents: {writer_budget}."
        )
    } else {
        format!(
            "Produce at least {floor} `role=integrate` tasks, each with \
             `writes=true` and a distinct scope. One integrator per ticket \
             — do not consolidate tickets into a single writer. \
             Available writer agents: {writer_budget}."
        )
    };
    vec![Violation {
        id: "INV-17 parallel_min_integrators",
        task_id: None,
        agent_id: None,
        severity,
        human: format!(
            "Parallel plan has {integrate_count} integrate tasks; \
             template + intent indicate ≥ {floor}. {intent_hint}"
        ),
        hint,
    }]
}

/// Filters a violation set to just the rule-breaking entries (the LLM repair
/// loop trips only on these; advisories surface through other UI paths).
pub fn must_fix(violations: &[Violation]) -> Vec<Violation> {
    violations
        .iter()
        .filter(|v| matches!(v.severity, Severity::MustFix))
        .cloned()
        .collect()
}

fn task_role(task: &SwarmTask) -> Option<String> {
    task.role.as_deref().and_then(normalize_role_label)
}

fn is_role(task: &SwarmTask, want: &str) -> bool {
    task_role(task).as_deref() == Some(want)
}

fn proposer_tasks<'a>(ctx: &ValidationContext<'a>) -> Vec<&'a SwarmTask> {
    ctx.tasks
        .iter()
        .filter(|t| {
            is_role(t, "propose")
                || t.id.to_ascii_lowercase().starts_with("propose-")
                || t.id.eq_ignore_ascii_case("propose")
        })
        .collect()
}

fn judge_tasks<'a>(ctx: &ValidationContext<'a>) -> Vec<&'a SwarmTask> {
    ctx.tasks
        .iter()
        .filter(|t| is_role(t, "judge") || t.id.eq_ignore_ascii_case("judge"))
        .collect()
}

fn integrate_tasks<'a>(ctx: &ValidationContext<'a>) -> Vec<&'a SwarmTask> {
    ctx.tasks
        .iter()
        .filter(|t| is_role(t, "integrate") || t.id.eq_ignore_ascii_case("integrate"))
        .collect()
}

fn invariant_nonempty_tasks(ctx: &ValidationContext<'_>) -> Vec<Violation> {
    if ctx.tasks.is_empty() {
        return vec![Violation {
            id: "INV-01 nonempty_tasks",
            task_id: None,
            agent_id: None,
            severity: Severity::MustFix,
            human: "Plan has no tasks.".into(),
            hint: "Emit at least one task. If no work is required, the swarm should not have been planned.".into(),
        }];
    }
    Vec::new()
}

fn invariant_unique_task_ids(ctx: &ValidationContext<'_>) -> Vec<Violation> {
    let mut seen: HashMap<&str, usize> = HashMap::new();
    let mut violations = Vec::new();
    for task in ctx.tasks.iter() {
        *seen.entry(task.id.as_str()).or_insert(0) += 1;
    }
    for (id, count) in seen.iter() {
        if *count > 1 {
            violations.push(Violation {
                id: "INV-02 unique_task_ids",
                task_id: Some((*id).to_string()),
                agent_id: None,
                severity: Severity::Advisory,
                human: format!("Task id `{id}` appears {count} times."),
                hint: format!(
                    "Give each task a distinct `id`; planner picked `{id}` more than once."
                ),
            });
        }
    }
    violations
}

fn invariant_agent_ids_allowed(ctx: &ValidationContext<'_>) -> Vec<Violation> {
    let allowed: HashSet<&str> = ctx.available_agents.iter().map(|s| s.as_str()).collect();
    let mut violations = Vec::new();
    for task in ctx.tasks.iter() {
        if !allowed.contains(task.agent_id.as_str()) {
            violations.push(Violation {
                id: "INV-03 agent_ids_allowed",
                task_id: Some(task.id.clone()),
                agent_id: Some(task.agent_id.clone()),
                severity: Severity::MustFix,
                human: format!(
                    "Task `{}` is assigned to `{}`, which is not in the available agents list.",
                    task.id, task.agent_id
                ),
                hint: format!(
                    "Reassign task `{}` to one of: {}.",
                    task.id,
                    ctx.available_agents.join(", ")
                ),
            });
        }
    }
    violations
}

fn invariant_singleton_judge(ctx: &ValidationContext<'_>) -> Vec<Violation> {
    let judges = judge_tasks(ctx);
    if judges.len() <= 1 {
        return Vec::new();
    }
    let ids = judges
        .iter()
        .map(|t| t.id.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    vec![Violation {
        id: "INV-04 singleton_judge",
        task_id: None,
        agent_id: None,
        severity: Severity::MustFix,
        human: format!("Plan has {} judge tasks: {ids}.", judges.len()),
        hint: "Collapse to exactly one `role=judge` task that depends on every proposer.".into(),
    }]
}

fn invariant_singleton_integrate(ctx: &ValidationContext<'_>) -> Vec<Violation> {
    // Parallel template intentionally allows multi-writer integrate fan-out
    // for topical sharding; only enforce singleton on lab/bulk.
    if matches!(ctx.template, SwarmTemplate::Parallel) {
        return Vec::new();
    }
    let integrates = integrate_tasks(ctx);
    if integrates.len() <= 1 {
        return Vec::new();
    }
    let ids = integrates
        .iter()
        .map(|t| t.id.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    vec![Violation {
        id: "INV-05 singleton_integrate",
        task_id: None,
        agent_id: None,
        severity: Severity::MustFix,
        human: format!(
            "Plan has {} integrate tasks under {} template: {ids}.",
            integrates.len(),
            ctx.template.label()
        ),
        hint: "Collapse to exactly one `role=integrate` task assigned to the designated integrator agent.".into(),
    }]
}

fn invariant_writes_only_on_integrator(ctx: &ValidationContext<'_>) -> Vec<Violation> {
    // Parallel writers may legitimately be multiple agents — that constraint
    // is enforced elsewhere via role hints; here we only complain when the
    // writer's role isn't `integrate`.
    let mut violations = Vec::new();
    for task in ctx.tasks.iter() {
        if !task.writes {
            continue;
        }
        let role = task_role(task);
        if role.as_deref() != Some("integrate") {
            violations.push(Violation {
                id: "INV-06 writes_only_on_integrator",
                task_id: Some(task.id.clone()),
                agent_id: Some(task.agent_id.clone()),
                severity: Severity::MustFix,
                human: format!(
                    "Task `{}` has `writes=true` but `role={}`.",
                    task.id,
                    role.as_deref().unwrap_or("<none>")
                ),
                hint: format!(
                    "Either set `role=integrate` for task `{}` or set `writes=false`.",
                    task.id
                ),
            });
        }
    }
    violations
}

fn invariant_integrator_assignment(ctx: &ValidationContext<'_>) -> Vec<Violation> {
    let Some(integrator) = ctx.integrator_agent_id else {
        return Vec::new();
    };
    // Parallel may sub-shard writers across agents; only enforce on lab/bulk.
    if matches!(ctx.template, SwarmTemplate::Parallel) {
        return Vec::new();
    }
    let mut violations = Vec::new();
    for task in integrate_tasks(ctx) {
        if task.agent_id != integrator {
            violations.push(Violation {
                id: "INV-07 integrator_assignment",
                task_id: Some(task.id.clone()),
                agent_id: Some(task.agent_id.clone()),
                severity: Severity::MustFix,
                human: format!(
                    "Integrate task `{}` is assigned to `{}` but the designated integrator is `{}`.",
                    task.id, task.agent_id, integrator
                ),
                hint: format!(
                    "Reassign task `{}` to integrator `{integrator}`.",
                    task.id
                ),
            });
        }
    }
    violations
}

fn invariant_judge_depends_on_all_proposers(ctx: &ValidationContext<'_>) -> Vec<Violation> {
    let proposers = proposer_tasks(ctx);
    let judges = judge_tasks(ctx);
    if proposers.is_empty() || judges.is_empty() {
        return Vec::new();
    }
    let mut violations = Vec::new();
    for judge in judges.iter() {
        for proposer in proposers.iter() {
            if proposer.id == judge.id {
                continue;
            }
            if !judge.deps.iter().any(|d| d == &proposer.id) {
                violations.push(Violation {
                    id: "INV-08 judge_depends_on_all_proposers",
                    task_id: Some(judge.id.clone()),
                    agent_id: Some(judge.agent_id.clone()),
                    severity: Severity::MustFix,
                    human: format!(
                        "Judge task `{}` is missing dep on proposer `{}`.",
                        judge.id, proposer.id
                    ),
                    hint: format!(
                        "Add `{}` to `{}`'s `deps` so the judge sees every proposer's output.",
                        proposer.id, judge.id
                    ),
                });
            }
        }
    }
    violations
}

fn invariant_integrate_depends_on_judge(ctx: &ValidationContext<'_>) -> Vec<Violation> {
    let judges = judge_tasks(ctx);
    let integrates = integrate_tasks(ctx);
    if judges.is_empty() || integrates.is_empty() {
        return Vec::new();
    }
    let judge_ids: HashSet<&str> = judges.iter().map(|t| t.id.as_str()).collect();
    let mut violations = Vec::new();
    for integrate in integrates.iter() {
        if !integrate
            .deps
            .iter()
            .any(|d| judge_ids.contains(d.as_str()))
        {
            violations.push(Violation {
                id: "INV-09 integrate_depends_on_judge",
                task_id: Some(integrate.id.clone()),
                agent_id: Some(integrate.agent_id.clone()),
                severity: Severity::MustFix,
                human: format!(
                    "Integrate task `{}` does not depend on any judge task.",
                    integrate.id
                ),
                hint: format!(
                    "Add the judge task id to `{}`'s `deps` so the integrator only runs after the judge verdict.",
                    integrate.id
                ),
            });
        }
    }
    violations
}

fn invariant_no_unknown_deps(ctx: &ValidationContext<'_>) -> Vec<Violation> {
    // Advisory: the downstream `validate_plan_dag` already aborts or repairs
    // (per `.nit/config.toml`) on unknown deps. Catching it here too would
    // either short-circuit that path or burn LLM repair rounds on something
    // a deterministic pass handles cleanly.
    let issues = analyze_swarm_dag(ctx.tasks);
    issues
        .unknown_deps
        .iter()
        .map(|(task_id, dep)| Violation {
            id: "INV-10 no_unknown_deps",
            task_id: Some(task_id.clone()),
            agent_id: None,
            severity: Severity::Advisory,
            human: format!("Task `{task_id}` depends on unknown task `{dep}`."),
            hint: format!(
                "Either remove `{dep}` from `{task_id}`'s deps or add a task with that id."
            ),
        })
        .collect()
}

fn invariant_acyclic(ctx: &ValidationContext<'_>) -> Vec<Violation> {
    // Advisory for the same reason as `no_unknown_deps`: the DAG abort path
    // handles cycles. The validator's repair loop is reserved for issues
    // where re-prompting the planner is the most productive next step.
    let issues = analyze_swarm_dag(ctx.tasks);
    issues
        .cycle
        .map(|cycle| {
            vec![Violation {
                id: "INV-11 acyclic",
                task_id: cycle.first().cloned(),
                agent_id: None,
                severity: Severity::Advisory,
                human: format!("Dependency cycle: {}", cycle.join(" -> ")),
                hint: "Break the cycle by removing one of the deps in the path; tasks must form a DAG.".into(),
            }]
        })
        .unwrap_or_default()
}

fn invariant_integrate_present_for_code_change(ctx: &ValidationContext<'_>) -> Vec<Violation> {
    if !matches!(ctx.mission_kind, SwarmMissionKind::General) {
        return Vec::new();
    }
    if !root_prompt_requests_code_change(ctx.root_prompt) {
        return Vec::new();
    }
    if integrate_tasks(ctx).is_empty() {
        return vec![Violation {
            id: "INV-12 integrate_present_for_code_change",
            task_id: None,
            agent_id: None,
            severity: Severity::MustFix,
            human: "Operator request implies code changes but the plan has no `role=integrate` task with `writes=true`.".into(),
            hint: "Add one integrate task with `writes=true` assigned to the integrator. Without it, the swarm produces no workspace edits.".into(),
        }];
    }
    Vec::new()
}

fn invariant_min_proposers(ctx: &ValidationContext<'_>) -> Vec<Violation> {
    if !matches!(ctx.template, SwarmTemplate::Bulk) {
        return Vec::new();
    }
    let proposers = proposer_tasks(ctx);
    let non_integrator_agents = match ctx.integrator_agent_id {
        Some(integrator) => ctx
            .available_agents
            .iter()
            .filter(|id| id.as_str() != integrator)
            .count(),
        None => ctx.available_agents.len(),
    };
    let need = if non_integrator_agents >= 2 { 2 } else { 1 };
    if proposers.len() >= need {
        return Vec::new();
    }
    vec![Violation {
        id: "INV-13 min_proposers",
        task_id: None,
        agent_id: None,
        severity: Severity::MustFix,
        human: format!(
            "Bulk plan has {} proposer task(s); expected at least {need}.",
            proposers.len()
        ),
        hint: format!(
            "Add at least {need} `role=propose` task(s) so the judge has more than one candidate to weigh."
        ),
    }]
}

fn invariant_role_hint_honored(ctx: &ValidationContext<'_>) -> Vec<Violation> {
    let mut violations = Vec::new();
    for (agent_id, expected) in ctx.role_hints.iter() {
        let expected_norm = normalize_role_label(expected);
        let Some(expected_norm) = expected_norm else {
            continue;
        };
        if expected_norm == "all" {
            continue;
        }
        let matched = ctx.tasks.iter().any(|task| {
            task.agent_id.as_str() == agent_id.as_str()
                && task_role(task).as_deref() == Some(expected_norm.as_str())
        });
        if matched {
            continue;
        }
        // If the agent has any task at all but with a different role, that's
        // the more interesting case to surface.
        let assigned_role = ctx
            .tasks
            .iter()
            .find(|t| t.agent_id.as_str() == agent_id.as_str())
            .and_then(task_role);
        let human = match assigned_role.as_deref() {
            Some(other) => format!(
                "Agent `{agent_id}` has role hint `{expected_norm}` but was assigned a task with role `{other}`."
            ),
            None => format!(
                "Agent `{agent_id}` has role hint `{expected_norm}` but the plan assigns it no task with that role."
            ),
        };
        violations.push(Violation {
            id: "INV-14 role_hint_honored",
            task_id: None,
            agent_id: Some(agent_id.clone()),
            severity: Severity::MustFix,
            human,
            hint: format!(
                "Assign agent `{agent_id}` a task with `role={expected_norm}` (its roster hint)."
            ),
        });
    }
    violations
}

fn invariant_no_proposer_to_proposer_dep(ctx: &ValidationContext<'_>) -> Vec<Violation> {
    let proposers = proposer_tasks(ctx);
    if proposers.len() < 2 {
        return Vec::new();
    }
    let proposer_ids: HashSet<&str> = proposers.iter().map(|t| t.id.as_str()).collect();
    let mut violations = Vec::new();
    for task in proposers.iter() {
        for dep in task.deps.iter() {
            if proposer_ids.contains(dep.as_str()) && dep != &task.id {
                violations.push(Violation {
                    id: "INV-15 no_proposer_to_proposer_dep",
                    task_id: Some(task.id.clone()),
                    agent_id: Some(task.agent_id.clone()),
                    severity: Severity::Advisory,
                    human: format!(
                        "Proposer `{}` depends on proposer `{dep}`; proposers must run in parallel.",
                        task.id
                    ),
                    hint: format!(
                        "Remove `{dep}` from `{}`'s `deps`. Proposers are independent investigators, not a pipeline.",
                        task.id
                    ),
                });
            }
        }
    }
    violations
}

fn invariant_artifacts_field_shape(ctx: &ValidationContext<'_>) -> Vec<Violation> {
    // We only care that the field is a flat list of identifiers/labels; the
    // v2 schema enforces this at deserialize time. This invariant catches
    // the leftover case of artifacts that are obviously placeholder garbage
    // (empty strings) which sometimes slip through when the LLM emits
    // `"artifacts": [""]`.
    let mut violations = Vec::new();
    for task in ctx.tasks.iter() {
        for entry in task.artifacts.iter() {
            if entry.trim().is_empty() {
                violations.push(Violation {
                    id: "INV-16 artifacts_field_shape",
                    task_id: Some(task.id.clone()),
                    agent_id: Some(task.agent_id.clone()),
                    severity: Severity::Advisory,
                    human: format!(
                        "Task `{}` has an empty entry in its `artifacts` array.",
                        task.id
                    ),
                    hint: format!(
                        "Drop the blank entry from `{}`'s `artifacts`, or replace it with a real label.",
                        task.id
                    ),
                });
                break;
            }
        }
    }
    violations
}

fn invariant_bulk_max_proposers(ctx: &ValidationContext<'_>) -> Vec<Violation> {
    if !matches!(ctx.template, SwarmTemplate::Bulk) {
        return Vec::new();
    }
    let proposers = proposer_tasks(ctx);
    if proposers.len() <= BULK_PROPOSER_HARD_CAP {
        return Vec::new();
    }
    vec![Violation {
        id: "INV-17 bulk_max_proposers",
        task_id: None,
        agent_id: None,
        severity: Severity::MustFix,
        human: format!(
            "Bulk plan has {} proposers; per-dep budget collapses past {BULK_PROPOSER_HARD_CAP}.",
            proposers.len()
        ),
        hint: format!(
            "Reduce proposers to at most {BULK_PROPOSER_HARD_CAP}; pick the most differentiated lenses."
        ),
    }]
}

const CODE_CHANGE_HINTS: &[&str] = &[
    "implement",
    "refactor",
    "rewrite",
    "build the",
    "add a",
    "add the",
    "fix the",
    "fix a ",
    "make the ",
    "modify",
    "introduce",
    "create a new",
    "wire ",
    "wire up",
    "hook up",
    "rip out",
    "rename",
    "delete",
    "remove the",
    "extract ",
    "split ",
    "merge ",
    "consolidate",
];

fn root_prompt_requests_code_change(root_prompt: &str) -> bool {
    let lower = root_prompt.to_ascii_lowercase();
    CODE_CHANGE_HINTS
        .iter()
        .any(|needle| lower.contains(needle))
}

/// Renders the validator's MustFix invariant catalog as a bulleted list for
/// inclusion in the planner prompt. Keeps the planner's constraints in lock-
/// step with what the validator actually enforces — both sides read the same
/// list, so drift between the prompt and the check is impossible by
/// construction.
pub(super) fn planner_invariants_for_prompt(template: SwarmTemplate) -> Vec<&'static str> {
    let mut lines: Vec<&'static str> = vec![
        "Every task must have a unique `id`.",
        "Every task's `agent_id` must be in the allowed agent list.",
        "At most one `role=judge` task. The judge MUST depend on every proposer task.",
        "Every task with `writes=true` MUST have `role=integrate`.",
        "Every `integrate` task MUST depend on the judge task when a judge is present.",
        "All `deps` must reference task ids that exist in the same plan.",
        "Tasks must form a DAG (no cycles).",
        "When an agent has a non-`all` role hint, you MUST assign that agent a task with the matching role.",
        "Proposers (`role=propose`) must run in parallel; never make one proposer depend on another.",
    ];
    if !matches!(template, SwarmTemplate::Parallel) {
        lines.push(
            "At most one `role=integrate` task. The integrate task MUST be assigned to the designated integrator agent.",
        );
    }
    if matches!(template, SwarmTemplate::Bulk) {
        lines.push("Emit at least 2 proposer tasks when there are 2+ non-integrator agents.");
        lines.push("Cap proposers at 12 to keep the per-dep budget meaningful.");
    }
    if matches!(template, SwarmTemplate::Parallel) {
        // Parallel-template fanout floor. The actual numeric floor comes
        // from `parallel_integrate_floor` at validation time (operator
        // intent + available-writer count); the prompt-side rule states
        // the contract so the planner emits N integrators on the first
        // attempt rather than waiting for a repair round.
        lines.push(
            "PARALLEL FANOUT — MUST: when the operator's prompt enumerates distinct tickets (bullet list, numbered list, `T<n>.` headers, or multiple `Files:` blocks), produce one `role=integrate` task per ticket. Consolidating multiple tickets into a single integrator is a plan failure that the post-parse validator auto-rejects. The OPERATOR INTENT block earlier in this prompt gives the exact count.",
        );
        lines.push(
            "PARALLEL DISTINCT SCOPE: each integrate task MUST declare a distinct scope (different files / different ticket). Two integrators with the same scope is a plan failure.",
        );
    }
    lines
}
