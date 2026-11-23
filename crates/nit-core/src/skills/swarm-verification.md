# Verify a swarm change

Choose checks that exercise the changed behavior and report exactly what they establish.

## TEST DISCIPLINE

1. Identify the affected modules, packages or files.
2. Run targeted checks for those paths. Examples include `cargo test -p <affected-crate>`, `pytest path/to/affected/dir`, `go test ./path/to/affected/...` and `npm test --workspace=<pkg>`.
3. For multiple affected packages, combine package flags or run each package separately.
4. Run workspace-wide tests, builds, lint or formatting sweeps only when the operator explicitly asks for that scope. Report targeted failures and their cause. The post-execution verifier owns the configured full gates.

## Code shape

Keep tests in dedicated test files or directories. When refactoring a source file that contains an inline test module, move the tests into a separate test file. Keep barrel files and re-exports small. Retain comments that explain decisions, constraints and assumptions. Remove comments that simply repeat the code.

## Revert policy

Repair or revert your own broken edits when required to restore working behavior. Preserve working changes required by the task. If an edit lowers a structural score, inspect the finding, address justified issues and report remaining tradeoffs. Removing required behavior solely to make a metric pass leaves the task incomplete.
