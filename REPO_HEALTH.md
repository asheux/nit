# Repo Health (Validated)

Date: 2026-03-03
Scope: quick, command-backed repo health check (integrator pass)

## Current State Snapshot

- Repo type: Rust workspace (`Cargo.toml`, `crates/*`).
- Tooling in use: `cargo`, `rustfmt`, `clippy`, `cargo-deny`, `just`.
- Local task aliases: `just fmt-check`, `just clippy`, `just test`, `just deny` (from `justfile`).
- CI workflow: `.github/workflows/ci.yml` runs fmt, clippy, test, deny on `ubuntu-latest`.
- Toolchain config:
  - `rust-toolchain.toml` => `channel = "stable"` (floating).
  - Workspace package `rust-version = "1.74"` (`Cargo.toml`).
- Dependency setup:
  - Workspace patches `time` to local path `vendor/time` (`[patch.crates-io]`).
  - `Cargo.lock` exists in tree but is ignored by `.gitignore`.
  - `vendor/` is ignored by `.gitignore`.
- Working tree status: dirty (multiple modified files + untracked directories/files).

## Verification Commands Run

```bash
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all
cargo deny check
git status --short
cat rust-toolchain.toml
cat Cargo.toml
cat .gitignore
cat .github/workflows/ci.yml
rg -n '^rust-version\\s*=|^version\\s*=' Cargo.toml vendor/time/Cargo.toml rust-toolchain.toml
git check-ignore -v Cargo.lock vendor vendor/time
```

Observed results:
- `fmt`: pass.
- `clippy`: pass with `-D warnings`.
- `test`: pass (all workspace tests green).
- `cargo deny check`: pass (`advisories/bans/licenses/sources ok`).
- `git check-ignore` confirms `Cargo.lock` and `vendor/` are ignored.

## Broken or Risky (Prioritized)

1. Reproducibility risk (high)
- `Cargo.toml` depends on `vendor/time` via patch.
- `.gitignore` ignores both `Cargo.lock` and `vendor/`.
- Risk: clean clones/CI can diverge from local build inputs.

2. MSRV/toolchain mismatch risk (high)
- Workspace says `rust-version = "1.74"`.
- Vendored `time` says `rust-version = "1.88.0"`.
- `rust-toolchain.toml` uses floating `stable`.
- Risk: MSRV claims are not enforceable and may break unexpectedly.

3. CI determinism risk (medium)
- CI installs `cargo-deny` with `cargo install cargo-deny --locked` but without explicit version pin.
- CI only runs on `ubuntu-latest`.
- Risk: tool and runner drift can cause non-reproducible failures.

4. Automation/security coverage gap (medium)
- No dependency update automation config found (Dependabot/Renovate).
- No CI secret-scanning workflow config found.
- Risk: delayed dependency hygiene and missed accidental secret commits.

5. Operational hygiene risk (low/medium)
- Working tree has many unrelated local modifications/untracked paths.
- Risk: noisy diffs can hide regressions and complicate triage.

## Concrete Next Steps

1. Lock reproducibility policy (maintainer decision)
- Choose one:
  - Track `Cargo.lock` and required `vendor/` content.
  - Remove `[patch.crates-io] time = { path = "vendor/time" }` and use crates.io lockfile flow.
- Follow-up commands:
```bash
git add Cargo.toml Cargo.lock vendor/
git status --short
```

2. Align MSRV and toolchain policy
- Pick a supported minimum Rust version and make it consistent across:
  - `Cargo.toml` `rust-version`
  - vendored deps (or remove vendoring)
  - CI/toolchain pin
- Follow-up commands:
```bash
cat rust-toolchain.toml
rg -n '^rust-version\\s*=' Cargo.toml vendor/time/Cargo.toml
```

3. Harden CI determinism
- Pin `cargo-deny` version in `.github/workflows/ci.yml`.
- Add matrix at least for stable + MSRV (once policy is decided).
- Follow-up commands:
```bash
sed -n '1,220p' .github/workflows/ci.yml
```

4. Add lightweight automation and secret scanning
- Add `.github/dependabot.yml` (or Renovate config).
- Add a CI secret scan step/workflow.
- Follow-up task scope:
  - one PR for dependency automation
  - one PR for secret scanning workflow

5. Reduce working tree noise before release branches
- Create a clean-branch validation routine for CI-equivalent checks.
- Follow-up commands:
```bash
git status --short
just ci
```
