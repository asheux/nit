# Repo Health Report

Date: 2026-03-04
Repo: `/Users/nitrika/Projects/Configs/nit`

## Quick Checklist Run

1. `cargo fmt --all -- --check` -> PASS

2. `cargo clippy --locked --all-targets --all-features -- -D warnings` -> PASS

3. `cargo test --all --locked --no-fail-fast` -> PASS
   - All discovered unit/integration/doc tests completed with no failures.

4. `cargo deny check` -> PASS
   - `advisories ok, bans ok, licenses ok, sources ok`

5. `git check-ignore -v Cargo.lock vendor vendor/time` -> PASS
   - No output (not ignored).

## Policy Snapshot

- Toolchain: pinned to Rust 1.88.0 via `rust-toolchain.toml` (rustfmt + clippy).
- MSRV: 1.88.0 (enforced across workspace crates via `rust-version`).
- Reproducibility: `Cargo.lock` is committed; `vendor/time` is committed due to `[patch.crates-io]`.
- CI: `.github/workflows/ci.yml` runs tests on `{1.88.0, stable}`; lint/deny on `1.88.0`.
- Automation: Dependabot, secret scanning (gitleaks), dependency review.

## Notes

- `justfile` provides convenience targets, but CI gates on the `cargo ...` equivalents.
