# Repo Health Report

Date: 2026-03-28
Repo: `/Users/nitrika/Projects/Configs/nit`

## Quick Checklist Run

1. `cargo fmt --all -- --check` -> PASS

2. `cargo clippy --locked --all-targets --all-features -- -D warnings` -> PASS

3. `cargo test --all --locked --no-fail-fast` -> PASS
   - 526 tests across 8 crates (nit-tui: 360, nit-games: 68, nit-core: 56, nit-gol: 22, nit-syntax: 8, nit-metal: 7, nit: 5).

4. `cargo deny check` -> PASS
   - `advisories ok, bans ok, licenses ok, sources ok`

5. `git check-ignore -v Cargo.lock vendor vendor/time` -> PASS
   - No output (not ignored).

## Policy Snapshot

- Toolchain: pinned to Rust 1.88.0 via `rust-toolchain.toml` (rustfmt + clippy).
- MSRV: 1.88.0 (enforced across workspace crates via `rust-version`).
- Reproducibility: `Cargo.lock` is committed; `vendor/time` is committed due to `[patch.crates-io]`.
- CI: `.github/workflows/ci.yml` runs tests on `{1.88.0, stable}`; lint/deny on `1.88.0`. Runner: `ubuntu-24.04`.
- Automation: Dependabot, secret scanning (gitleaks), dependency review.
- Safety: `#![forbid(unsafe_code)]` on 7/8 crates (exception: `nit-metal` for GPU interop).

## Notes

- `justfile` provides convenience targets (`fmt`, `clippy`, `test`, `deny`, `ci`, `run`), but CI gates on the `cargo ...` equivalents.
