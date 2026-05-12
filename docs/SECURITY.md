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

## Security Hardening Backlog

### Guiding principles

- Prefer "secure-by-default" behavior with explicit opt-outs.
- Assume **file contents, repo contents, and agent output are untrusted** unless user explicitly trusts them.
- When in doubt: *don't execute*, *don't write outside workspace*, and *don't render raw control sequences*.

### High priority (practical risk reducers)

- [ ] **Terminal escape sanitization (untrusted text rendering)**
  - [ ] Strip/neutralize ANSI escape sequences (ESC `\x1b` + CSI/OSC/etc) from:
    - editor buffer rendering (`crates/nit-tui/src/widgets/editor_view.rs`)
    - agent output rendering (`crates/nit-tui/src/widgets/agent_console_view/`)
    - status lines / logs / diagnostics
  - [ ] Decide policy for control characters (`0x00..0x1f`, `0x7f`): drop vs render as visible glyphs.
  - [ ] Add tests with payloads like OSC 52, window title changes, cursor movement, etc.
  - [ ] "Debug mode" escape hatch to view raw bytes when explicitly enabled.

- [ ] **Path + symlink safety for saves**
  - [ ] On save, refuse to write through symlinks (file or parent dirs) unless explicitly confirmed.
  - [ ] Optional "confine saves to workspace root" mode; warn/confirm on writes outside workspace.
  - [ ] Improve atomic saves to avoid predictable temp names and symlink races:
    - prefer unique temp file names + `create_new(true)` (or the `tempfile` crate)
    - consider fsyncing parent directory after rename on Unix for durability
  - [ ] Display clear UI warning when editing a symlinked path.

- [ ] **External process boundary hardening**
  - [x] Treat `git`, `codex`, and `claude` as untrusted boundaries; document that `nit` spawns all three (plus `open`/`xdg-open` for links and `gemini` for model probing).
  - [ ] Reduce PATH hijack risk:
    - resolve and display the full resolved path to `git`/`codex`/`claude` at startup
    - optionally allow pinning absolute paths in config
  - [ ] Add "safe mode" flag that disables all external processes (`git`, `codex`, `claude`, etc.).

- [ ] **Provenance/logging privacy**
  - [ ] `.nit/` data: add `.nit/` to `.gitignore` by default or store under an OS-specific app dir.
  - [ ] Make agent run provenance optional/configurable (off by default for privacy-sensitive workflows).
  - [ ] Write provenance files with restrictive permissions (best-effort `0700`/`0600` on Unix).
  - [ ] Add optional redaction for obvious secret patterns before writing logs to disk.

- [ ] **Dependency hygiene**
  - [x] Fix RustSec advisory currently flagged by `cargo deny` (e.g. update `time` to a patched version).
  - [ ] Decide policy for BSL-1.0 dependencies (allow vs replace).
  - [x] Stop ignoring `Cargo.lock` for the app (commit lockfile for reproducible builds) or document why not.
  - [x] Add CI gates for `cargo deny` (advisories + licenses) and for `cargo clippy` correctness.

### Medium priority (defense in depth)

- [ ] **Agent safety UX (Codex + Claude)**
  - [ ] Safer defaults for Codex integration (sandbox + approval), with explicit prompts to relax.
  - [ ] Show a prominent indicator when running in "danger-full-access" / low-approval modes.
  - [ ] Add a per-workspace allowlist/denylist for which agent backends (Codex, Claude, Gemini) can execute.
  - [ ] Add a "network use" indicator based on the selected backend/runtime configuration.
  - [ ] Surface Claude permission mode prominently in Agent Ops.

- [ ] **Clipboard controls**
  - [ ] Allow disabling clipboard integration entirely.
  - [ ] Optional auto-clear clipboard after N seconds for copied secrets.
  - [ ] Prevent implicit copying of content that contains control sequences.

- [ ] **Robustness against hostile inputs (DoS)**
  - [ ] File size limits / progressive loading for very large files.
  - [ ] Directory walk limits + cancellation for huge repos.
  - [ ] Harden JSON parsing of external event streams (Codex/MCP): strict limits on line length/fields.
  - [ ] Rate-limit extremely verbose agent logs to avoid UI lockups.

- [ ] **Config hardening**
  - [ ] Separate "trusted" vs "untrusted" workspace profiles (like an editor's restricted mode).
  - [ ] Taint certain sources (agent output, external events) and keep them out of file writes by default.

### Longer-term / advanced hardening

- [ ] **Sandboxing**
  - [ ] Optional OS-level sandbox for `nit` itself (where feasible): e.g. macOS sandbox-exec profile.
  - [ ] Stronger sandbox story for external tools (beyond Codex's own sandboxing knobs).

- [ ] **Fuzzing + property tests**
  - [ ] Add fuzz targets for: rule parsers, snapshot formats, event JSON, and any custom protocol parsing.
  - [ ] Regression corpus for terminal escape payloads and weird Unicode edge cases.

- [ ] **Security documentation & process**
  - [x] Update `SECURITY.md` to match reality (spawns `git`, `codex`, `claude`, and `open`/`xdg-open`).
  - [ ] Maintain a "security checklist" for releases (deny/audit, escape sanitization, safe defaults).
  - [ ] Document recommended settings for working on untrusted repos.
