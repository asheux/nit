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
