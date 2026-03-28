# Security Policy

## Philosophy

- Secure-by-default: no plugins, no network calls from `nit` itself, and no arbitrary command execution.
- `nit` spawns a small set of external tools **directly (no shell)**:
  - `git` (repo introspection, ignore checks, file listing)
  - `codex` (Agent Station — MCP server or exec runtime; may make network requests depending on Codex configuration)
  - `claude` (Agent Station — subprocess per turn via `claude -p`; may make network requests)
  - `open` / `xdg-open` / `cmd` (platform-specific URL launcher, used only when the user activates a link)
- At startup, `nit` probes for `codex`, `claude`, and `gemini` CLI availability on `PATH` and may invoke them briefly to list models. No persistent `gemini` subprocess is spawned at runtime.
- Terminal state is restored on exit and panic.
- Saves are atomic and confined to explicit paths provided by the user.

## Reporting

If you find a vulnerability, please open an issue or contact the maintainers privately. Avoid public disclosure until a fix is available.

## Protections Implemented

- `#![forbid(unsafe_code)]` across all crates except `nit-metal` (Metal GPU interop).
- No network I/O in-process.
- External command execution is limited to invoking `git`/`codex`/`claude` directly (no shell).
- Atomic file writes using temp files in the destination directory.
- Defensive error handling around terminal raw mode; drop to a safe state on panic.

## Future Work

- Configurable sandboxing for extensions.
- Signed theme/config bundles.
- Tainted data tracking for future LSP/plugins.
