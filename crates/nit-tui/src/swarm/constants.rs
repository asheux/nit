pub(crate) const DEFAULT_SWARM_SIZE: usize = 4;
/// Hard ceiling on roster size for a single swarm mission. Sized so the worst
/// case (every agent spawning a Codex/Claude subprocess concurrently, each
/// holding ~4 file descriptors) fits under typical OS ulimits — ~250 agents
/// on Linux's 1024-fd default, comfortably within macOS's 10240. Beyond this,
/// the FD ceiling is the real limit, not nit. Soft warning surfaces at
/// `LARGE_SWARM_WARN_THRESHOLD`.
pub(super) const MAX_SWARM_SIZE: usize = 256;
/// At or above this roster size, push a one-line system message warning the
/// operator to bump `ulimit -n` and confirm machine has enough RAM/CPU.
/// Picked so the warning fires well before subprocess spawn starts failing
/// from FD exhaustion on Linux defaults.
pub(crate) const LARGE_SWARM_WARN_THRESHOLD: usize = 64;
pub(super) const SWARM_VERIFY_MAX_CHARS: usize = 12_000;
pub(super) const SWARM_DEP_OUTPUT_MAX_CHARS: usize = 8_000;
/// Per-dep ceiling for roles that need full dependency output (judge,
/// integrate, any write-role task). A single comprehensive multi-file
/// refactoring proposal can reach 20–30K chars; giving the downstream agent
/// the full reasoning chain materially improves decisions. Biased toward
/// preserving information.
pub(crate) const SWARM_DEP_OUTPUT_MAX_CHARS_FULL: usize = 48_000;

// Shared role-contract clauses.
//
// These constants keep three classes of rule single-source across the
// propose/integrate/judge/review/test role contracts AND the retry prompts
// (gate retry, genome retry). Previous drafts drifted — e.g. "NO REVERT"
// wording differed between gate-retry and genome-retry, "don't pad small
// files" was written three different ways. Route every copy through these.

/// Targeted-vs-workspace-wide test/verify command rule. Every copy must be
/// byte-for-byte identical, so the role contracts reference this constant
/// instead of inlining the text.
pub(crate) const TEST_DISCIPLINE_CLAUSE: &str =
    "TEST DISCIPLINE — STRICT: Workspace-wide / repo-wide commands \
     (`cargo test --all` / `--workspace`, `cargo clippy --workspace`, \
     `cargo fmt --all`, `go test ./...`, `pytest` from the repo root, \
     `npm test --workspaces`, `just test`, `just ci`, full lint/type-check \
     sweeps, etc.) are ONLY allowed when the OPERATOR explicitly asked for \
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
     running `cargo test -p nit-gol` (passes) AND THEN running \
     `cargo test --all` to \"verify the full results\" — exactly the \
     duplication the rule forbids. The post-execution gate verifier handles \
     workspace-wide gates as the next swarm stage.";

/// Don't-pad-small-files + inline-test-module + comment-hygiene clause.
/// Referenced by the integrate role contract and the genome-retry prompt
/// so both say the same thing word-for-word.
pub(crate) const NO_PADDING_CLAUSE: &str =
    "CODE SHAPE: Do NOT add inline test modules (`#[cfg(test)] mod tests \
     { ... }`) inside source files — tests live in a dedicated tests \
     directory or test file. If you encounter an existing inline test \
     module during a refactor, move it to the appropriate test file. Do \
     NOT pad small files (lib.rs, mod.rs, re-export files) with \
     unnecessary code to boost genome scores — trivially small files are \
     auto-passed. Do NOT over-engineer trivial logic to hit a metric. \
     COMMENTS: trim doc comments that restate type/function names, echo \
     visible type signatures, or describe obvious behavior. Keep comments \
     that explain WHY, document non-obvious constraints, safety \
     invariants, or algorithmic choices.";

/// Non-revert rule for any retry prompt (gate retry, per-agent genome
/// retry). Reverting your own BROKEN code is fine — that's how you fix
/// things. Rolling back WORKING real work to satisfy a metric is not.
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
/// Total budget across ALL deps for full-output roles. Sized against Claude's
/// 200K-token context (~800K chars) minus ~100K chars of system scaffolding
/// and a safety margin for turn-time tool_use/tool_result accumulation (which
/// was the observed overflow path for clone-04/05). The cap only bites when
/// fan-in is large (7+ deps); for typical 2–6-proposer swarms every dep still
/// gets its full 48K per-dep ceiling.
pub(super) const SWARM_DEP_OUTPUT_TOTAL_MAX_CHARS_FULL: usize = 240_000;
pub(super) const COMPUTATIONAL_RESEARCH_ROLE: &str = "computational-research";
pub(super) const COMPUTATIONAL_RESEARCH_ROLE_LEGACY: &str = "computational research";
