use std::collections::HashSet;

use super::validator::Violation;

/// Result of comparing this round's MustFix violations against the prior
/// round. The repair loop only continues when the planner is making concrete
/// progress — strict improvement OR a clean subset of the prior set.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) struct RepairOutcome {
    pub(super) strictly_improved: bool,
    pub(super) same_violations_persist: bool,
}

/// Evaluates whether one repair round actually moved the needle. Two rules:
///
/// 1. `strictly_improved` — the new violation set is smaller in cardinality
///    OR is a proper subset of the prior set (same size, but the planner
///    fixed some and introduced different ones is NOT good enough — that's
///    ping-pong).
/// 2. `same_violations_persist` — any `(id, task_id)` pair from the prior
///    round still appears. If a specific defect survived the repair prompt,
///    further rounds won't move it either.
pub(super) fn evaluate_repair_round(prior: &[Violation], current: &[Violation]) -> RepairOutcome {
    let prior_sigs: HashSet<_> = prior.iter().map(Violation::signature).collect();
    let current_sigs: HashSet<_> = current.iter().map(Violation::signature).collect();

    let strictly_improved = current_sigs.len() < prior_sigs.len()
        || (current_sigs.len() == prior_sigs.len()
            && current_sigs != prior_sigs
            && current_sigs.is_subset(&prior_sigs));

    let same_violations_persist = prior_sigs.iter().any(|sig| current_sigs.contains(sig));

    RepairOutcome {
        strictly_improved,
        same_violations_persist,
    }
}

/// Builds the repair prompt fed back to the planner. The shape is:
///   1. A short framing line explaining the situation and round counter.
///   2. The verbatim original planner prompt (so the planner sees the same
///      operator request, constraints, and output format).
///   3. The previous plan JSON (so the planner can edit, not rewrite).
///   4. A numbered list of MustFix violations with their `hint` strings.
///   5. A reminder that the response must be the same v2 JSON schema.
///
/// All ids and prompts are template-substituted from typed fields — never
/// free-form text — so repair messages can't be injected via task ids that
/// look like prompt fragments. The validator's own `Violation.hint` is
/// itself constructed from typed fields and sanitized ids.
pub(super) fn build_repair_prompt(
    original_planner_prompt: &str,
    prior_plan_json: &str,
    violations: &[Violation],
    round: u8,
    max_rounds: u8,
) -> String {
    let mut out =
        String::with_capacity(original_planner_prompt.len() + prior_plan_json.len() + 512);
    out.push_str(&format!(
        "Your previous plan did not pass the deterministic validator. Repair round {round}/{max_rounds}. \
         Re-emit a corrected plan IN THE SAME v2 JSON SCHEMA. Keep what was correct; fix only the violations listed below.\n\n"
    ));
    out.push_str("VIOLATIONS TO FIX:\n");
    for (idx, v) in violations.iter().enumerate() {
        let scope = match (&v.task_id, &v.agent_id) {
            (Some(task), Some(agent)) => format!(" (task={task}, agent={agent})"),
            (Some(task), None) => format!(" (task={task})"),
            (None, Some(agent)) => format!(" (agent={agent})"),
            (None, None) => String::new(),
        };
        out.push_str(&format!(
            "{n}. [{id}]{scope} {human}\n   → {hint}\n",
            n = idx + 1,
            id = v.id,
            human = v.human,
            hint = v.hint,
        ));
    }
    out.push_str("\nPREVIOUS PLAN (correct in place):\n```json\n");
    out.push_str(prior_plan_json);
    out.push_str("\n```\n\n");
    out.push_str("ORIGINAL PLANNER PROMPT (re-stated; constraints unchanged):\n");
    out.push_str(original_planner_prompt);
    out.push_str(
        "\n\nReturn the corrected plan as: 3-6 summary bullets followed by a single ```json code block matching the v2 schema. Do not include any other code blocks.\n",
    );
    out
}
