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

// Operator-facing rollback for the deterministic plan validator + repair
// loop. Accepted truthy values: `1`, `true`, `yes`, `on` (case-insensitive).
// Resolved once at `SwarmRuntime::new`; when set, the planner stage stays
// byte-identical to the pre-validator behaviour.
pub(super) const NIT_PLANNER_LEGACY_ENV: &str = "NIT_PLANNER_LEGACY";

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
