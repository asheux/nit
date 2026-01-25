# Security Policy

## Philosophy

- Secure-by-default: no plugins, no shell execution, and no network calls in the MVP.
- Terminal state is restored on exit and panic.
- Saves are atomic and confined to explicit paths provided by the user.

## Reporting

If you find a vulnerability, please open an issue or contact the maintainers privately. Avoid public disclosure until a fix is available.

## Protections Implemented

- `#![forbid(unsafe_code)]` across all crates.
- No external command execution.
- No automatic file watchers or network I/O.
- Atomic file writes using temp files in the destination directory.
- Defensive error handling around terminal raw mode; drop to a safe state on panic.

## Future Work

- Configurable sandboxing for extensions.
- Signed theme/config bundles.
- Tainted data tracking for future LSP/plugins.

