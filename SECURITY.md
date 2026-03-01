# Security Policy

## Philosophy

- Secure-by-default: no plugins, no network calls from `nit` itself, and no arbitrary command execution.
- Agent Station can invoke the local `codex` CLI (which may make network requests depending on Codex configuration).
- Terminal state is restored on exit and panic.
- Saves are atomic and confined to explicit paths provided by the user.

## Reporting

If you find a vulnerability, please open an issue or contact the maintainers privately. Avoid public disclosure until a fix is available.

## Protections Implemented

- `#![forbid(unsafe_code)]` across all crates.
- No network I/O in-process.
- External command execution is opt-in and limited to invoking `codex` directly (no shell).
- Atomic file writes using temp files in the destination directory.
- Defensive error handling around terminal raw mode; drop to a safe state on panic.

## Future Work

- Configurable sandboxing for extensions.
- Signed theme/config bundles.
- Tainted data tracking for future LSP/plugins.
