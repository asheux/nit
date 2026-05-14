pub(crate) const DEFAULT_SWARM_SIZE: usize = 4;

// Sized so the worst case (every agent spawning a Codex/Claude subprocess
// concurrently, each holding ~4 fds) fits under typical OS ulimits — ~250
// agents on Linux's 1024-fd default. Beyond this, the FD ceiling is the
// real limit; nit warns at `LARGE_SWARM_WARN_THRESHOLD`.
pub(super) const MAX_SWARM_SIZE: usize = 256;

// Bounded LLM repair attempts when the validator finds structural defects
// in a parsed plan. Two retries = three planner calls worst case (1 initial
// + 2 repairs). Past this we fall back to the deterministic template or
// abort, rather than burn unbounded planner tokens on a stuck plan.
pub(super) const REPAIR_RETRY_LIMIT: u8 = 2;

// Initial budget for the verifier-findings auto-retry loop. When a
// test or review task finishes with `parsed_artifacts.findings`
// non-empty, the runtime synthesises one integrator turn scoped to
// those findings and decrements the budget. At 0, findings stop
// triggering retries and flow into synthesis as advisory only — the
// operator dispatches any further fix themselves. Set to 1 deliberately
// for cost safety: a runaway retry loop on flaky tests or pedantic
// review remarks would burn agent tokens with no human in the loop.
// Bump cautiously.
pub(super) const VERIFIER_RETRY_BUDGET_DEFAULT: u8 = 1;

// Operator-facing rollback for the deterministic plan validator + repair
// loop. Accepted truthy values: `1`, `true`, `yes`, `on` (case-insensitive).
// Resolved once at `SwarmRuntime::new`; when set, the planner stage stays
// byte-identical to the pre-validator behaviour.
pub(super) const NIT_PLANNER_LEGACY_ENV: &str = "NIT_PLANNER_LEGACY";

// Per-role total-prompt ceilings (bytes) enforced after `wrap_task_prompt`
// assembles a dispatch. Sized against Claude's 200K-token window (~800K
// chars) with ~120K reserved for system framing and turn-time
// tool_use/tool_result accumulation. Integrate/judge get the larger tail
// because they receive full-output dep payloads (`SWARM_DEP_OUTPUT_*_FULL`).
// Propose/review/test sit under the warm-pool worker's safe envelope so
// pool slots can't be poisoned by oversized vanilla prompts.
pub(crate) const PROMPT_BUDGET_INTEGRATE: usize = 480_000;
pub(crate) const PROMPT_BUDGET_JUDGE: usize = 320_000;
pub(crate) const PROMPT_BUDGET_RESEARCH: usize = 240_000;
pub(crate) const PROMPT_BUDGET_PROPOSE: usize = 160_000;
pub(crate) const PROMPT_BUDGET_REVIEW: usize = 120_000;
pub(crate) const PROMPT_BUDGET_TEST: usize = 96_000;
pub(crate) const PROMPT_BUDGET_DEFAULT: usize = 96_000;

// Setting any of `0`/`false`/`no`/`off` (case-insensitive) returns
// `usize::MAX` from `PromptBudgets::for_role`, making `apply_prompt_budget`
// a no-op short-circuit. The off-path stays in the code as the rollback
// pattern documented for `NIT_CLAUDE_POOL=0` and `NIT_PLANNER_LEGACY=1`.
pub(crate) const NIT_PROMPT_TIERS_ENV: &str = "NIT_PROMPT_TIERS";

// Warn well before subprocess spawn starts hitting `EMFILE` on Linux's
// 1024-fd default.
pub(crate) const LARGE_SWARM_WARN_THRESHOLD: usize = 64;

pub(super) const SWARM_VERIFY_MAX_CHARS: usize = 12_000;
pub(super) const SWARM_DEP_OUTPUT_MAX_CHARS: usize = 8_000;

// Per-dep ceiling for full-output roles (judge, integrate, any write-role).
// A multi-file refactor proposal can reach 20–30K chars; preserving the
// reasoning chain materially improves downstream decisions.
pub(crate) const SWARM_DEP_OUTPUT_MAX_CHARS_FULL: usize = 48_000;

// Total budget across ALL deps for full-output roles. Sized against
// Claude's 200K-token context (~800K chars) minus ~100K of system
// scaffolding plus turn-time tool_use/tool_result accumulation (the
// observed overflow path for clone-04/05). Only bites at fan-in 7+; for
// 2–6-proposer swarms every dep gets its full 48K per-dep ceiling.
pub(super) const SWARM_DEP_OUTPUT_TOTAL_MAX_CHARS_FULL: usize = 240_000;

pub(super) const COMPUTATIONAL_RESEARCH_ROLE: &str = "computational-research";
pub(super) const COMPUTATIONAL_RESEARCH_ROLE_LEGACY: &str = "computational research";

// Role-contract clauses below keep three classes of rule single-source
// across propose/integrate/judge/review/test contracts AND retry prompts.
// Earlier drafts drifted (NO REVERT wording differed between gate-retry
// and genome-retry; "don't pad small files" had three variants).

// Verifier roles (test, review) inject this into their role contract so
// they know that emitting structured `findings` in their swarm_artifacts
// JSON drives an automatic re-dispatch — the orchestrator will create an
// integrator turn scoped to those findings instead of waiting for the
// operator to launch a fix swarm. Bounded by `VERIFIER_RETRY_BUDGET_DEFAULT`
// (= 1) so a single retry pass happens automatically; further fixes are
// the operator's call.
pub(crate) const FINDINGS_RETRY_CLAUSE: &str =
    "FINDINGS = AUTO-RETRY TRIGGER: When you detect a concrete, fixable issue \
     (failed test, fmt drift, clippy warning, broken assertion, missing import \
     after a rename, etc.), emit it as a STRUCTURED entry in the \
     `swarm_artifacts.findings` array — NOT as prose in `notes`. Each finding \
     must include at least `file` (repo-relative path) and `issue` (one-line \
     description); add `line`, `severity` (\"error\" | \"warning\"), `category` \
     (\"fmt\" | \"clippy\" | \"test\" | \"other\"), and `suggestion` (concrete \
     replacement text) whenever you have them. The orchestrator parses this \
     array and — if non-empty AND the run still has retry budget left — \
     dispatches ONE follow-up integrator turn scoped to the cited files with \
     your findings injected as the task description. Budget is small (1 retry \
     by default), so high-value findings only: include things a writer can \
     plausibly fix; leave subjective style critiques in `notes` where they \
     inform synthesis without burning a retry turn. You remain READ-ONLY — \
     report findings, do NOT attempt to fix them yourself.";

pub(crate) const TEST_DISCIPLINE_CLAUSE: &str =
    "TEST DISCIPLINE — STRICT: Workspace-wide / repo-wide commands of any \
     toolchain — `cargo test --all` / `--workspace`, `cargo clippy --workspace`, \
     `cargo fmt --all`, `go test ./...`, `pytest` from the repo root, \
     `npm test --workspaces`, `just test`, `just ci`, full lint/type-check \
     sweeps, etc. — are ONLY allowed when the OPERATOR explicitly asked for \
     them in the request above (phrases like \"run full CI\", \"verify the \
     whole workspace\", \"run all tests\", \"make sure nothing else broke\"). \
     If the operator did not, you MUST NOT run a workspace-wide command — \
     not as a confirmation pass after a targeted run, not to be thorough, \
     not \"just to be safe\", not even when a targeted run fails (in that \
     case, report the failure and let the operator decide whether to widen). \
     DEFAULT: run only targeted commands scoped to the modules/packages/files \
     the swarm actually touched (e.g. `cargo test -p <affected-crate>`, \
     `pytest path/to/affected/dir`, `go test ./path/to/affected/...`, \
     `npm test --workspace=<pkg>`). MULTI-MODULE CHANGES: combine targeted \
     flags (`cargo test -p crate1 -p crate2`) or run one targeted command \
     per module. Do NOT widen to workspace-wide. EXAMPLE OF WRONG BEHAVIOUR: \
     running `cargo test -p <affected-crate>` (passes) AND THEN running \
     `cargo test --all` to \"verify the full results\" — exactly the \
     duplication the rule forbids. The post-execution gate verifier handles \
     workspace-wide gates as the next swarm stage.";

pub(crate) const NO_PADDING_CLAUSE: &str =
    "CODE SHAPE: Do NOT add inline test modules (e.g. Rust `#[cfg(test)] \
     mod tests { ... }`, Python `if __name__ == '__main__'` test blocks, \
     Go `_test.go` stubs co-located in source files) inside production \
     source files — tests live in a dedicated tests directory or test \
     file. If you encounter an existing inline test module during a \
     refactor, move it to the appropriate test file. Do NOT pad small \
     files (re-export / barrel files such as `lib.rs`, `mod.rs`, \
     `index.ts`, `__init__.py`) with unnecessary code to boost genome \
     scores — trivially small files are auto-passed. Do NOT over-engineer \
     trivial logic to hit a metric. COMMENTS: trim doc comments that \
     restate type/function names, echo visible type signatures, or \
     describe obvious behavior. Keep comments that explain WHY, document \
     non-obvious constraints, safety invariants, or algorithmic choices.";

pub(crate) const NO_REVERT_CLAUSE: &str =
    "REVERT POLICY: reverting your own BROKEN code (compile errors, failing \
     tests, broken logic) is fine — that's how you fix things. Rolling back \
     WORKING edits just to make a gate or metric pass vacuously (e.g. `git \
     restore` on a real refactor because genome-quality dropped a tier, \
     deleting a new file the proposer asked for, restoring the pre-refactor \
     body of a function) is a task failure, not a fix. Passing a gate by \
     throwing away real work is strictly worse than failing the gate with \
     the work intact. If you genuinely can't improve the metric further \
     through real edits, say so in your reply and STOP — leave the mission's \
     actual changes on disk.";

pub(crate) const CODE_HYGIENE_OPEN_MARKER: &str = "[code hygiene]";

// Same intent as `NO_PADDING_CLAUSE` but always prepended at dispatch time.
// Returns `None` when the prompt already carries the marker OR inlines
// `NO_PADDING_CLAUSE` verbatim via a role contract — so the same rules
// never get duplicated in one prompt.
pub(crate) fn code_hygiene_preamble(prompt: &str) -> Option<String> {
    if prompt.contains(CODE_HYGIENE_OPEN_MARKER)
        || prompt.contains("CODE SHAPE: Do NOT add inline test modules")
    {
        return None;
    }
    Some(format!(
        "{CODE_HYGIENE_OPEN_MARKER}\n{NO_PADDING_CLAUSE}\n[/code hygiene]\n\n"
    ))
}
