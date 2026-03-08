# Repo Health (Validated)

Date: 2026-03-04
Scope: quick, command-backed repo health check (integrator pass)

## Current State Snapshot

- Repo type: Rust workspace (`Cargo.toml`, `crates/*`).
- Tooling in use: `cargo`, `rustfmt`, `clippy`, `cargo-deny`, `just`.
- Local task aliases: `just fmt-check`, `just clippy`, `just test`, `just deny` (from `justfile`).
- CI workflow: `.github/workflows/ci.yml` runs fmt, clippy, test, deny on `ubuntu-24.04`.
- Toolchain config:
  - `rust-toolchain.toml` => `channel = "1.88.0"` (pinned).
  - Workspace MSRV: `rust-version = "1.88.0"` (enforced across crates).
- Dependency setup:
  - Workspace patches `time` to local path `vendor/time` (`[patch.crates-io]`).
  - `Cargo.lock` is tracked (CI uses `--locked`).
  - `vendor/time` is tracked (required by the `time` patch).
- Automation:
  - Dependabot config for Cargo + GitHub Actions.
  - Secret scanning workflow (gitleaks) + dependency review workflow.

## Verification Commands Run

```bash
cargo fmt --all -- --check
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo test --all --locked --no-fail-fast
cargo deny check
cat rust-toolchain.toml
cat Cargo.toml
cat .gitignore
cat .github/workflows/ci.yml
rg -n '^rust-version\\s*=|^version\\s*=' Cargo.toml vendor/time/Cargo.toml rust-toolchain.toml
git check-ignore -v Cargo.lock vendor vendor/time
cargo metadata --no-deps
```

Observed results:
- `fmt`: pass.
- `clippy`: pass with `-D warnings`.
- `test`: pass (all workspace tests green) with `--locked`.
- `cargo deny check`: pass (`advisories/bans/licenses/sources ok`).
- `git check-ignore` shows no ignore rules for `Cargo.lock` or `vendor/`.
- `cargo metadata --no-deps` reports `rust_version = "1.88.0"` across workspace crates.

## Remaining Risks / Tradeoffs

1. Vendored patch maintenance (medium)
- `vendor/time` is a checked-in, patched dependency.
- Tradeoff: updates require manual vendoring steps; Dependabot ignores `time` to avoid inconsistent bumps.

2. GitHub Actions supply-chain pinning (optional)
- Workflows currently pin actions to release tags (e.g. `@v4`) rather than full commit SHAs.
- Tradeoff: easier maintenance vs stronger determinism/supply-chain guarantees.

---

## 2026-03-08 Quick Health Check

Scope: operator-facing, command-backed repo health check on the current local worktree.

### Current Snapshot

- Branch: `main...origin/main`.
- Worktree state: 10 tracked modified files, 1 untracked file (`docs/ANTIGRAVITY.md`), and large ignored runtime/build state under `.nit/`, `runs/`, `target/`, and `gol-snapshots/`.
- Diff footprint: `git diff --stat` reports 10 files changed, 1139 insertions, 235 deletions.
- Diff hygiene: `git diff --check` returned clean.
- Rust gates: `cargo fmt --all -- --check`, `cargo clippy --locked --all-targets --all-features -- -D warnings`, `cargo test --all --locked --no-fail-fast`, and `cargo deny check` all passed on 2026-03-08.
- CLI probe: `cargo run -- games run --help` still reports `--out` as "Output directory (defaults to ./output)".

### Commands Run

```bash
git status --short --branch --ignored
git diff --stat
git diff --check
cargo fmt --all -- --check
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo test --all --locked --no-fail-fast
cargo deny check
cargo run -- games run --help
rg -n "cargo fmt|cargo clippy|cargo test|cargo deny|just fmt|just clippy|just test|just deny|Build \+ CI Checks|verify|verification" justfile .github/workflows/ci.yml README.md docs/SMOKE_TEST.md docs/ARCHITECTURE.md docs/SWARM.md
rg -n "games run|runs/games|games-runs|\.\/output|output directory|--out" README.md docs/SMOKE_TEST.md docs/GAMES.md crates/nit/src/main.rs crates/nit-games -g '!target'
rg --files -g 'SECURITY.md' -g 'CONTRIBUTING.md' -g '.github/**'
nl -ba justfile | sed -n '1,40p'
nl -ba .github/workflows/ci.yml | sed -n '24,44p'
nl -ba docs/SMOKE_TEST.md | sed -n '1,20p;196,244p'
nl -ba docs/ARCHITECTURE.md | sed -n '256,268p'
nl -ba README.md | sed -n '1,40p;160,176p'
nl -ba docs/GAMES.md | sed -n '220,250p'
nl -ba crates/nit/src/main.rs | sed -n '96,108p'
nl -ba SECURITY.md | sed -n '1,80p'
nl -ba docs/REPO_HEALTH.md | sed -n '1,40p'
rg -n "ARCHITECTURE|KEYBINDINGS|SMOKE_TEST|GAMES|SWARM|SECURITY|CONTRIBUTING" README.md
```

### Prioritized Findings

1. High: verification parity is drifting across local aliases, docs, swarm docs, and CI.
   - `justfile` runs `cargo clippy --all-targets --all-features -- -D warnings` and `cargo test --all`, while CI uses the stricter locked variants in `.github/workflows/ci.yml`.
   - `docs/SMOKE_TEST.md` labels `just fmt`, `just clippy`, and `just test` as "Build + CI Checks", but `just fmt` mutates the tree and the list omits `cargo deny check`.
   - `docs/ARCHITECTURE.md` still documents `cargo clippy --all-targets --all-features -- -D warnings` and `cargo test --workspace --all-features`, which no longer matches the CI-grade command set that passed today.

2. High: Games output-path guidance is inconsistent across code, help text, and docs.
   - Live CLI help and `crates/nit/src/main.rs` still advertise `./output`.
   - `README.md` says Games outputs land in `games-runs/`.
   - `docs/SMOKE_TEST.md` and `docs/GAMES.md` describe `runs/games/...`, while `docs/GAMES.md` also treats `games-runs/` and `output/` as legacy compatibility paths.

3. High: the repo is currently dirty on `main`, so cleanup guidance needs to stay non-destructive.
   - The active changes are concentrated in `crates/nit-tui/src/*` plus `docs/ARCHITECTURE.md`, `docs/SMOKE_TEST.md`, and `docs/SWARM.md`.
   - Ignored directories include `.nit/`, which holds resumable swarm state, so blanket cleanup commands such as `git clean -fdx` would be unsafe.
   - `git diff --check` is clean, so the current risk is workflow/merge friction rather than whitespace or conflict markers.

4. Medium: contributor and security docs still create avoidable friction.
   - `SECURITY.md` asks reporters to "contact the maintainers privately" but does not provide a private channel.
   - No `CONTRIBUTING.md` was found.
   - Health reporting is duplicated across `REPO_HEALTH.md` and `docs/REPO_HEALTH.md`, and both were already dated 2026-03-04 before this refresh.

### Concrete Next Steps

1. Pick one canonical verification baseline: use the CI-grade commands that passed today, then mirror them in `justfile`, `README.md`, `docs/SMOKE_TEST.md`, `docs/ARCHITECTURE.md`, and swarm verification guidance.
2. Standardize Games outputs on `runs/games/` and update the clap help text, `README.md`, and Games docs together; keep `games-runs/` and `output/` documented only as legacy compatibility locations if they still need to be read.
3. Move the in-flight work off `main` before additional changes, and avoid blanket cleanup commands because `.nit/` and `docs/ANTIGRAVITY.md` are easy to lose accidentally.
4. Add a real private security reporting path, add a short `CONTRIBUTING.md`, and choose one canonical repo-health report location to reduce doc drift.

### Structured Artifacts

```json
{"type":"swarm_artifacts","version":1,"task_id":"integrate","summary":"Quick repo health check completed on 2026-03-08: CI-grade Rust gates are green, but verification/docs drift, Games output-path drift, dirty-worktree safety on main, and contributor/security doc gaps should be addressed next.","artifacts":{"files":["/Users/nitrika/Projects/Configs/nit/REPO_HEALTH.md","/Users/nitrika/Projects/Configs/nit/justfile","/Users/nitrika/Projects/Configs/nit/.github/workflows/ci.yml","/Users/nitrika/Projects/Configs/nit/README.md","/Users/nitrika/Projects/Configs/nit/docs/SMOKE_TEST.md","/Users/nitrika/Projects/Configs/nit/docs/ARCHITECTURE.md","/Users/nitrika/Projects/Configs/nit/docs/GAMES.md","/Users/nitrika/Projects/Configs/nit/SECURITY.md","/Users/nitrika/Projects/Configs/nit/crates/nit/src/main.rs","/Users/nitrika/Projects/Configs/nit/docs/REPO_HEALTH.md"],"diffs":["/Users/nitrika/Projects/Configs/nit/REPO_HEALTH.md: appended 2026-03-08 quick health check section"],"commands":["git status --short --branch --ignored","git diff --stat","git diff --check","cargo fmt --all -- --check","cargo clippy --locked --all-targets --all-features -- -D warnings","cargo test --all --locked --no-fail-fast","cargo deny check","cargo run -- games run --help","rg -n \"cargo fmt|cargo clippy|cargo test|cargo deny|just fmt|just clippy|just test|just deny|Build \\+ CI Checks|verify|verification\" justfile .github/workflows/ci.yml README.md docs/SMOKE_TEST.md docs/ARCHITECTURE.md docs/SWARM.md","rg -n \"games run|runs/games|games-runs|\\.\\/output|output directory|--out\" README.md docs/SMOKE_TEST.md docs/GAMES.md crates/nit/src/main.rs crates/nit-games -g '!target'","rg --files -g 'SECURITY.md' -g 'CONTRIBUTING.md' -g '.github/**'","nl -ba justfile | sed -n '1,40p'","nl -ba .github/workflows/ci.yml | sed -n '24,44p'","nl -ba docs/SMOKE_TEST.md | sed -n '1,20p;196,244p'","nl -ba docs/ARCHITECTURE.md | sed -n '256,268p'","nl -ba README.md | sed -n '1,40p;160,176p'","nl -ba docs/GAMES.md | sed -n '220,250p'","nl -ba crates/nit/src/main.rs | sed -n '96,108p'","nl -ba SECURITY.md | sed -n '1,80p'","nl -ba docs/REPO_HEALTH.md | sed -n '1,40p'","rg -n \"ARCHITECTURE|KEYBINDINGS|SMOKE_TEST|GAMES|SWARM|SECURITY|CONTRIBUTING\" README.md"],"risks":["High: verification parity is drifting across local aliases, docs, swarm docs, and CI.","High: Games output-path guidance is inconsistent across code, help text, and docs.","High: the repo is currently dirty on main, so cleanup guidance needs to stay non-destructive.","Medium: contributor and security docs still create avoidable friction."],"notes":["`git diff --check` returned no whitespace or conflict-marker issues.","`cargo deny check` returned `advisories ok, bans ok, licenses ok, sources ok`.","`cargo run -- games run --help` still advertises `./output` as the default output directory.","Only `REPO_HEALTH.md` was edited; no other dirty files were modified."]}}
```
