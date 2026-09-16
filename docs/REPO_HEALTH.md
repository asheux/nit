# Repo Health

How to check that the `nit` repo is healthy, and what the project enforces.
Test counts change constantly, so this doc does not state them; run
`cargo test --all` for the live count.

## How to check

Quick check:

```bash
scripts/healthcheck.sh
```

It prints repo probes (working tree, toolchain pins, CI config, security
tooling), then gates on `cargo check`, `cargo fmt --all -- --check`, and
`cargo deny check`. Add `--deep` to also run clippy and the full test suite.
Every gate should print `[green] pass` and the script should exit 0.

Full local CI:

```bash
just ci
```

The `justfile` wraps the cargo commands (`fmt`, `fmt-check`, `clippy`,
`test`, `deny`, `ci`, `run`). CI gates on the cargo commands themselves:

```bash
cargo fmt --all -- --check
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo test --all --locked --no-fail-fast
cargo deny check
git check-ignore -v Cargo.lock vendor vendor/time
```

The last command must print nothing: `Cargo.lock`, `vendor`, and
`vendor/time` are committed, not ignored.

## What the project enforces

- Toolchain: pinned to Rust 1.88.0 via `rust-toolchain.toml`, with rustfmt and clippy.
- MSRV: 1.88.0, set once as `rust-version` in the workspace `Cargo.toml` and inherited by every crate.
- Reproducibility: `Cargo.lock` is committed and CI runs with `--locked`. `vendor/time` is committed because `Cargo.toml` patches `time` through `[patch.crates-io]`.
- CI (`.github/workflows/ci.yml`): lint (fmt, clippy, doc, deny) on `ubuntu-24.04` with 1.88.0; tests on `ubuntu-24.04`, `macos-14`, and `windows-2022` with 1.88.0; an MSRV release build with 1.88.0.
- Automation: Dependabot and gitleaks secret scanning run from `.github/`. Dependency review is a GitHub repository setting, not a workflow file.
- Safety: `#![forbid(unsafe_code)]` in every crate except `nit-metal` (Metal GPU interop) and `nit-mcp`.
