# Security Policy

## Philosophy

- Secure by default: no plugins, no network calls from nit itself, and no arbitrary command execution.
- nit spawns a small set of external tools directly, with no shell:
  - `git`: repo introspection, ignore checks, file listing
  - `codex`: Agent Station, as an MCP server or exec runtime; may use the network depending on your Codex configuration
  - `claude`: Agent Station, one `claude -p` subprocess per turn; may use the network
  - `open` / `xdg-open` / `cmd`: the platform URL launcher, only when you activate a link
- At startup nit probes `PATH` for `codex`, `claude`, and `gemini` and may run them briefly to list models. It never keeps a `gemini` subprocess running.
- Terminal state is restored on exit and on panic.
- Saves are atomic and go only to paths you name.

## Reporting

If you find a vulnerability, open an issue or contact the maintainers
privately. Please hold public disclosure until a fix is available.

## Protections implemented

- `#![forbid(unsafe_code)]` in every crate except `nit-metal` (Metal GPU interop) and `nit-mcp`.
- No network I/O in-process.
- External commands are limited to `git`, `codex`, and `claude`, run directly with no shell.
- Atomic file writes through temp files in the destination directory.
- Defensive error handling around terminal raw mode. nit drops to a safe state on panic.

## Hardening backlog

Guiding principles: prefer secure-by-default behaviour with opt-outs you choose.
Treat file contents, repo contents, and agent output as untrusted unless you
say otherwise. When in doubt: do not execute, do not write outside the
workspace, and do not render raw control sequences.

### High priority

- [ ] Strip or neutralize ANSI escape sequences (ESC, CSI, OSC) in editor rendering, agent output, status lines, logs, and diagnostics.
- [ ] Decide a policy for control characters (`0x00..0x1f`, `0x7f`): drop them or show visible glyphs.
- [ ] Add tests with payloads such as OSC 52, window title changes, and cursor movement.
- [ ] Add a debug-mode escape hatch to view raw bytes.
- [ ] Refuse to save through symlinks (file or parent dirs) without confirmation.
- [ ] Add an optional "confine saves to workspace root" mode that warns on writes outside it.
- [ ] Harden atomic saves: unique temp names with `create_new(true)` or the `tempfile` crate, and fsync the parent directory after rename on Unix.
- [ ] Show a clear UI warning when editing a symlinked path.
- [x] Treat `git`, `codex`, and `claude` as untrusted boundaries and document that nit spawns them, plus `open` / `xdg-open` for links and `gemini` for model probing.
- [ ] Reduce PATH hijack risk: show the resolved path to `git` / `codex` / `claude` at startup and allow pinning absolute paths in config.
- [ ] Add a "safe mode" flag that disables all external processes.
- [ ] Add `.nit/` to `.gitignore` by default, or store it under an OS-specific app dir.
- [ ] Make agent run provenance optional, off by default for privacy-sensitive workflows.
- [ ] Write provenance files with restrictive permissions (best-effort `0700` / `0600` on Unix).
- [ ] Add optional redaction of obvious secret patterns before logs hit disk.
- [x] Fix the RustSec advisory flagged by `cargo deny` (patched `time`).
- [ ] Decide a policy for BSL-1.0 dependencies (allow or replace).
- [x] Commit `Cargo.lock` for reproducible builds.
- [x] Add CI gates for `cargo deny` (advisories and licenses) and `cargo clippy`.

### Medium priority

- [ ] Safer Codex defaults (sandbox and approval), with a prompt before relaxing them.
- [ ] Show a prominent indicator in "danger-full-access" or low-approval modes.
- [ ] Add a per-workspace allowlist or denylist of agent backends (Codex, Claude, Gemini).
- [ ] Add a "network use" indicator based on the selected backend and runtime.
- [ ] Show the Claude permission mode in Agent Ops.
- [ ] Allow disabling clipboard integration entirely.
- [ ] Optionally auto-clear the clipboard after N seconds for copied secrets.
- [ ] Block implicit copying of content that contains control sequences.
- [ ] Add file size limits or progressive loading for very large files.
- [ ] Add directory walk limits and cancellation for huge repos.
- [ ] Harden JSON parsing of external event streams (Codex, MCP) with strict line and field limits.
- [ ] Rate-limit very verbose agent logs to avoid UI lockups.
- [ ] Separate trusted and untrusted workspace profiles, like an editor's restricted mode.
- [ ] Taint agent output and external events, and keep them out of file writes by default.

### Longer term

- [ ] Optional OS-level sandbox for nit itself, for example a macOS `sandbox-exec` profile.
- [ ] Stronger sandboxing for external tools, beyond Codex's own knobs.
- [ ] Fuzz targets for rule parsers, snapshot formats, event JSON, and custom protocol parsing.
- [ ] Regression corpus for terminal escape payloads and odd Unicode.
- [x] Keep `SECURITY.md` matching what nit spawns (`git`, `codex`, `claude`, `open` / `xdg-open`).
- [ ] Maintain a release security checklist: deny and audit, escape sanitization, safe defaults.
- [ ] Document recommended settings for working on untrusted repos.
