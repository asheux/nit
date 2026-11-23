# Check a source build

Use these checks when contributing a change to nit or diagnosing a difference between a release and your local checkout.

## Build the editor you will test

```sh
cargo build --locked -p nit -p nit-mcp
./target/debug/nit --version
./target/debug/nit --agents local .
```

Run the executable from `target/debug` to exercise this checkout. The repository pins its Rust toolchain in `rust-toolchain.toml`. Use a disposable workspace to test edits, agent runs or tournaments. [Hands-on checks](SMOKE_TEST.md)

## Check a change before sharing it

| Check | Command |
| --- | --- |
| Formatting | `cargo fmt --all -- --check` |
| Lints across supported features | `cargo clippy --locked --workspace --all-targets --all-features -- -D warnings` |
| Tests, including documentation examples | `cargo test --locked --workspace --no-fail-fast` |
| Dependency advisories, licenses and sources | `cargo deny check` |

`just ci` runs the project’s formatting, lint, test and dependency checks. For a failing test, include its exact name, the checkout revision, the platform and the command output in your report. If the issue is interactive, include the pane, the keys you pressed, the visible outcome and what you expected. Use synthetic filenames and data when sharing terminal captures.
