# nit — Neural Interface Terminal

A terminal-first, multi-pane TUI editor and **agent station** built in Rust. Secure-by-default, event-driven rendering, multi-backend agent orchestration (Codex + Claude), persistent stigmergic substrate, two built-in research labs (Conway's Game of Life, game-theory tournaments), and a multipane grid mode for driving N concurrent agent sessions from a single terminal.

## Install

**macOS, Linux, WSL:**

```bash
curl -fsSL https://download.nit.tools/install.sh | bash
```

**Windows (PowerShell):**

```powershell
irm https://download.nit.tools/install.ps1 | iex
```

**Homebrew (macOS / Linux):**

```bash
brew install asheux/tap/nit
```

**From source:**

```bash
git clone https://github.com/asheux/nit.git && cd nit
cargo build --release
# Binaries land at target/release/{nit, nit-mcp-server}
```

Binaries are distributed via `https://download.nit.tools/<tag>/`, backed by Amazon CloudFront. The bucket also hosts a `SHA256SUMS` file per release and `latest.json` for tag resolution.

### Supported platforms

| OS         | Architecture            | Distribution                                |
|------------|-------------------------|---------------------------------------------|
| macOS      | arm64 + x86_64 (universal) | `install.sh`, Homebrew, direct tarball   |
| Linux      | x86_64 (glibc)          | `install.sh`, Homebrew, direct tarball      |
| Windows    | x86_64 (MSVC)           | `install.ps1`, direct zip                   |

The macOS asset is a single universal Mach-O binary — Apple Silicon and
Intel Macs both run native code from the same file (no Rosetta needed).

`nit` requires external CLIs (`codex`, `claude`, `git`) on `PATH` to drive its agent runners.

## Quick start

```bash
nit path/to/file
nit games
nit multipane
```

- `nit <file>` opens the file in the editor.
- `nit <dir>` sets the workspace root (opens an untitled buffer).
- `nit` defaults to the current directory and an untitled buffer.
- `nit gol [path]` explicitly launches GoL mode.
- `nit games [path]` launches Games mode (tournaments between programs).
- `nit multipane [--backend <model>] [--panes N] [--cwd PATH]` opens a grid of independent chat panes.

## Development

```bash
just fmt
just clippy
just test
just run -- path/to/file

# Full CI gates (fmt-check + clippy + test + cargo deny):
just ci

# Quick repo-health preflight (add --deep to include clippy + tests):
scripts/healthcheck.sh
scripts/healthcheck.sh --deep
```

### Toolchain

- Rust 1.88.0 (pinned via `rust-toolchain.toml`)
- ratatui + crossterm for UI/input
- ropey, unicode-segmentation, unicode-width for text correctness
- tree-sitter 0.25 for syntax highlighting (28 active grammars; the workspace's language registry — extensions, filenames, shebangs, injection aliases, `is_code` flag — lives in `crates/nit-core/src/languages.rs`) and AST-based seed encoders

### Releasing

Pushing a `v*` tag to GitHub kicks off `.github/workflows/release.yml`, which:

1. Verifies the tag version matches the workspace `Cargo.toml` version (fast-fail if mismatched).
2. Creates a draft GitHub Release for human-readable notes.
3. Builds `nit` + `nit-mcp-server` for macOS (universal), Linux x86_64 (glibc), and Windows x86_64 in parallel.
4. Uploads each archive plus `.sha256` and an aggregated `SHA256SUMS` to `s3://download.nit.tools/<tag>/`.
5. On non-prerelease tags only: writes `s3://download.nit.tools/latest.json` so `install.sh` can resolve `latest`.
6. Re-uploads `install.sh` and `install.ps1` to the bucket root and invalidates the relevant CloudFront paths.
7. Updates the Homebrew formula at `asheux/homebrew-tap` (pre-release tags skip this step).
8. Promotes the draft Release to published; pre-release tags are marked as such.

Cut a release (e.g. `v0.1.2`):

```bash
# 1. Bump the workspace version. CI guards against tag/Cargo.toml mismatch.
$EDITOR Cargo.toml                       # version = "0.1.2" under [workspace.package]
git commit -am "Bump version to 0.1.2"
git push origin main

# 2. Tag and push. End-to-end workflow runs in ~6-8 min.
git tag v0.1.2
git push origin v0.1.2

# 3. Watch progress.
gh run watch
```

Pre-release tags (anything with a hyphen like `v0.2.0-rc1`) are marked as pre-release on GitHub, skip the Homebrew formula update, and don't bump `latest.json`.

#### Re-releasing a botched tag

If a release fails partway through and you need to retry under the same version:

```bash
git tag -d v0.1.2                        # delete locally
git push origin :refs/tags/v0.1.2        # delete remote
gh release delete v0.1.2 --yes           # if a GitHub Release was created
git tag v0.1.2
git push origin v0.1.2
```

If artifacts at the same key were already cached at the CDN, invalidate manually:

```bash
aws cloudfront create-invalidation \
  --distribution-id E31XHIF603G4P3 \
  --paths "/v0.1.2/*" "/latest.json" "/SHA256SUMS"
```

#### Required GitHub repo secrets

`AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`, `AWS_S3_BUCKET`, `AWS_CF_DISTRIBUTION_ID`, `HOMEBREW_TAP_TOKEN`.

### Download stats aggregator

`.github/workflows/stats-aggregator.yml` runs daily at 06:00 UTC. It walks CloudFront access logs under `s3://${AWS_S3_BUCKET}/AWSLogs/<account>/CloudFront/`, counts successful release-asset downloads by version and platform, and writes `s3://${AWS_S3_BUCKET}/stats.json`. The nit-website's prebuild script reads that file at deploy time and renders the Downloads section on the landing page.

Idempotent — each run only processes logs strictly newer than `last_processed_log_key` in the existing `stats.json`.

```bash
# Run on demand (e.g. after enabling CloudFront logging for the first time,
# or after fixing a schema bug).
gh workflow run stats-aggregator.yml
gh run watch

# Wipe and rebuild stats from scratch.
aws s3 rm s3://${AWS_S3_BUCKET}/stats.json
gh workflow run stats-aggregator.yml
```

### Reproducibility

- `Cargo.lock` is checked in; CI uses `--locked`.
- `time` is patched to a vendored copy at `vendor/time` (see `Cargo.toml`).

## Security Notes

- No plugins.
- No network calls from `nit` itself.
- No arbitrary command execution; `nit` may invoke `git`, `codex`, `claude`, and the platform URL launcher (`open`/`xdg-open`/`cmd`) directly (no shell). At startup, `codex`, `claude`, and `gemini` are probed for model detection.
- `#![forbid(unsafe_code)]` across all crates except `nit-metal` (Metal GPU interop).
- Atomic file writes.
- Terminal restored on exit and panic.

For details see `docs/SECURITY.md`.

## Agent Station

nit includes an Agent Station UI (Agent Ops + Agent Chat) with multiple backends: Codex (MCP or exec runtime), Claude (subprocess per turn, optional warm worker pool), and a local mock lane. Gemini models are detected at startup but display-only (no runtime runner yet).

- Default: seeds all available lanes (Codex, Claude, and Gemini models when detected on `PATH`).
- `nit --agents local` (alias: `mock`) — force local lane only.
- `nit --agents codex` — force Codex only (loads a model roster from `~/.codex/models_cache.json`).
- `nit --agents claude` — force Claude only (probes `claude models --json` for available models).
- `nit --agents all` — include all available lanes.
- Codex runtime knobs:
  - `--codex-runtime <mcp|exec>` (default: `mcp` — runs a persistent `codex mcp-server`; `exec` spawns `codex exec` per turn).
  - `--codex-sandbox <read-only|workspace-write|danger-full-access>` (default: Codex config).
  - `--codex-approval-policy <untrusted|on-failure|on-request|never>` (default: `never`).
  - `--codex-max-parallel-turns <N>` (alias `--codex-parallel`; default `8`, range `1..=16`). Shared cap across Codex and Claude.

### Agent Chat commands

- `@all <prompt>` — fan-out to multiple agents (Codex and Claude).
- `@swarm [all|N] [template=lab|parallel|bulk] [mission=general|research|computational-research] <prompt>` — orchestrated multi-agent workflow (plan → DAG tasks → verify → synthesis). `lab` is the default template. See `docs/SWARM.md`.
- `@shadow <prompt>` — single-agent dispatch with hidden propose-a / propose-b → judge → review pipeline; auto-enables for heavy prompts (>500 chars or keywords like `refactor`, `rewrite`, `implement`). See `docs/SHADOWS.md`.
- `@new <prompt>` — spawn a fresh-context clone when the agent is busy.
- `@queue` / `@q <prompt>` — explicit queue (same as the implicit queueing below).
- `/abort` (or `@abort`) — cancel the active swarm mission. `/abort all` cancels every running swarm; `/abort <agent-id>` is a surgical strike on one agent.
- Prompts sent while an agent is busy are automatically queued and dispatched when the agent becomes idle.

In front of every Claude-class dispatch, a hidden **intake agent** classifies the operator's intent and appends a file checklist for write/mixed prompts. Disable with `intake_enabled = false` in `config.toml` or `NIT_INTAKE_DISABLED=1` for a runtime kill switch. See `docs/INTAKE.md`.

Examples:

```bash
# Load all available lanes (default)
nit

# Force Codex agent station
nit --agents codex

# Force Claude-only agent station with the warm worker pool
NIT_CLAUDE_POOL=1 nit --agents claude

# Force Codex agent station, per-turn `codex exec`
nit --agents codex --codex-runtime exec

# Force local-only agent station
nit --agents local

# Multipane: 8 panes, full roster picker per pane
nit multipane

# Multipane: 4 panes pre-picked to a specific Claude lane
nit multipane --backend claude-haiku-4-5 --panes 4

# From source
cargo run -p nit -- --agents codex
```

## Project layout

```
nit/
├─ crates/
│  ├─ nit/                CLI binary entry point (args, agent discovery, lab dispatch)
│  │  └─ src/
│  │     ├─ agents/       Backend discovery (Claude, Codex, Gemini, discover)
│  │     ├─ cli/          clap subcommands + arg enums (lab, agents, codex, games)
│  │     ├─ games/        Headless games CLI (run, sweep, enumerate, inspect, graph)
│  │     ├─ graph/        Strategy graph export (DOT / JSON)
│  │     ├─ logging/      Tracing init + panic hook + log-path resolution
│  │     ├─ workspace/    Workspace target resolution + notes loading
│  │     ├─ bootstrap.rs  Runner config assembly, lab dispatch
│  │     ├─ multipane_setup.rs  Multipane launch wiring
│  │     └─ main.rs       Entry point + dispatch
│  ├─ nit-core/           Pure state + protocol layer (no terminal deps)
│  │  └─ src/
│  │     ├─ agent_bus/    `AgentBusEvent` enum + state-mutation helpers
│  │     ├─ arbiters/     Substrate arbiters (escalate, intervene)
│  │     ├─ buffer/       Rope-backed text buffer + diff/edit
│  │     ├─ config/       Settings + TOML loaders (editor, highlight, gol, swarm, genome)
│  │     ├─ genome_report/  Code-as-genome tier scoring, parsimony, recommendations
│  │     ├─ genome_storage/ Disk-backed report cache (sharded, atomic writes)
│  │     ├─ mission_memory/ Cross-mission retrieval index
│  │     ├─ observers/    Substrate observers (pattern detectors)
│  │     ├─ rule_protocol/  Rule protocol types (GoL B/S, presets)
│  │     ├─ seed/         GoL seed encoders (token_spectrum, ast_structure,
│  │     │                complexity, structural, ascii, hilbert, lifehash)
│  │     ├─ state/        AppState, AgentsState, MultipaneState, GamesState,
│  │     │                VisualizerState, etc.
│  │     ├─ substrate/    Signals, claims, assumptions, mood
│  │     └─ tests/        Core unit tests
│  ├─ nit-tui/            TUI app loop, widgets, agent runners, swarm + multipane
│  │  └─ src/
│  │     ├─ app/          Main event loop, key/mouse dispatch, chat input,
│  │     │                runner, draw, terminal, scroll, popups
│  │     ├─ codex_runner/ Codex backend (MCP + exec runtime, JSON-RPC)
│  │     ├─ multipane/    Multipane grid (dispatch, dir search, persistence)
│  │     ├─ swarm/        Swarm orchestrator (DAG planning/execution, gates,
│  │     │                plan parser, dashboard, prompts, workers, scope)
│  │     ├─ widgets/      All TUI widgets (agent ops, gate monitor, artifacts,
│  │     │                file tree, top/bottom bar, popups, ...)
│  │     ├─ gol_render/   Game of Life rendering
│  │     ├─ seed_render/  Genome seed visualization
│  │     ├─ workspace_scan/  Background workspace scanner
│  │     ├─ claude_runner.rs   Claude CLI subprocess runtime (`claude -p`)
│  │     ├─ claude_pool.rs     Warm worker pool (`NIT_CLAUDE_POOL=1`)
│  │     ├─ intake.rs          Hidden intent classifier (Claude-class only)
│  │     ├─ shadow.rs          Shadow agents (propose-a/-b → judge → review)
│  │     ├─ seed_runtime.rs    Seed compute worker + change detection
│  │     ├─ genome_worker.rs   Off-thread genome evaluation
│  │     ├─ mcp_backchannel.rs Unix-domain socket for spawned `codex mcp-server`
│  │     ├─ vitals.rs / system_stats.rs / power.rs   Process vitals + ECG
│  │     └─ ...                (file_watcher, fuzzy_*_runner, syntax, layout, ...)
│  ├─ nit-mcp/            MCP stdio JSON-RPC server (`nit-mcp-server` binary)
│  │                      — bridges spawned `codex` back into substrate tools
│  │                      (`emit_signal`, `assert_claim`, `assert_assumption`)
│  ├─ nit-games/          Game theory tournament engine
│  │  └─ src/
│  │     ├─ analysis/     History-log analysis (per-match, per-strategy, trajectories)
│  │     ├─ config/       Config parsing, normalization, payoff matrices
│  │     ├─ fsm_enum/     FSM enumeration + canonicalization
│  │     ├─ strategy/     Strategy codecs (FSM, CA, one-sided TM)
│  │     ├─ tournament/   Match execution, accumulation, Metal batching, halting filter
│  │     ├─ fast_eval.rs  Analytical evaluator (cycle detection on deterministic FSM)
│  │     ├─ introspection.rs   Strategy introspection / export
│  │     └─ history.rs / history_log.rs / events.rs / output.rs / ndjson.rs
│  ├─ nit-gol/            Conway's Game of Life engine
│  │  └─ src/             Grid, step, rules, hashing, attractor detection,
│  │                      snapshot manager, catalog
│  ├─ nit-metal/          Metal GPU acceleration (macOS)
│  │  └─ src/
│  │     ├─ macos/        Device, dispatch, shader, policy, cache
│  │     └─ stubs.rs      No-op stubs for non-macOS platforms
│  ├─ nit-syntax/         Tree-sitter syntax highlighting (registry derives
│  │  │                   from `nit-core::languages::LANGUAGES`; only the
│  │  │                   per-grammar `tree_sitter_<lang>` arms in
│  │  │                   `language/grammars.rs` live here)
│  │  ├─ src/             Engine, registry, captures, debounce
│  │  └─ queries/         Tree-sitter highlight queries per language
│  └─ nit-utils/          Shared filesystem, hashing, path utilities
├─ docs/                  Architecture, swarm, substrate, multipane, intake,
│                         shadows, seeds, games, keybindings, security, ...
├─ vendor/                Vendored dependencies (`time` crate)
├─ scripts/               Build and CI helpers (`healthcheck.sh`)
└─ assets/                Static assets
```

## Documentation

- `docs/ARCHITECTURE.md` — module layout, state model, agent system, swarm orchestration, runtime modes.
- `docs/KEYBINDINGS.md` — full keymap and `:` command reference (editor, agent ops, multipane).
- `docs/SWARM.md` — swarm orchestration operator guide (templates, roles, DAG, gates, custom gates, abort).
- `docs/SHADOWS.md` — shadow agents (propose-a/-b → judge → review behind a single agent).
- `docs/INTAKE.md` — intake preprocessor (hidden Claude-class intent classifier).
- `docs/MULTIPANE.md` — multipane grid mode (per-pane cwd, dir search, persistence).
- `docs/SUBSTRATE.md` — stigmergic substrate (signals, claims, assumptions, metabolism, mood).
- `docs/SUBSTRATE_TESTING.md` — substrate testing recipes + concrete verification steps.
- `docs/LIVING_SYSTEM.md` — coordination role roster (worker / observer / arbiter / resolver).
- `docs/GAMES.md` — games engine (strategies, config, headless CLI, analysis, Metal accelerator).
- `docs/SEEDS.md` — code-as-genome seed encoders, parsimony rule, retry guardrails.
- `docs/RULES.md` — Game of Life rule catalog and contribution guide.
- `docs/SMOKE_TEST.md` — feature tour + manual smoke checklist.
- `docs/PERF.md` — benchmarks and flamegraphs.
- `docs/SECURITY.md` — security policy, protections, and hardening backlog.
- `docs/REPO_HEALTH.md` — snapshot of the last repo-health audit (fmt/clippy/tests/deny).

## Command prompt (`:`)

Open the command prompt with `:` in Normal mode (or press `F1` / `?` for the full help overlay). Commands are routed to the active lab; start nit with `--lab gol|games` to switch.

- `:q` — quit (confirm if dirty)
- `:help` / `:commands` — open the help overlay
- `:run` — run the active app (GoL Petri Dish or Games tournament)
- `:gol run|hide|show|stop|rule|rules|encoder|seed` — GoL controls (aliases: `:petri`, `:life`)
- `:games run|hide|show|stop|status|runs|replay|inspect|tm|ca|analyze|strategy` — Games controls

Full command and keybinding reference: `docs/KEYBINDINGS.md`.

## GoL (Game of Life)

- Run Petri Dish: `Ctrl+Enter`; show hidden: `Ctrl+^`
- Petri Dish popup: `Space` pause, `Enter` step, `+/-` speed, `H` hide, `S` snapshot, `F2` rule picker, `P` protocol picker, `G` rule search, `A` apply best rule
- Visualizer seed controls: `Ctrl+E` encoder, `Ctrl+S` symmetry, `Ctrl+V` view, `Ctrl+R` cycle seed view, `Ctrl+M` plate render, `Ctrl+Y` seed source, `Ctrl+G` search, `Ctrl+A` apply, `Ctrl+N` snapshot
- Seed encoders: 7 encoders (byte-level, hybrid, AST-driven) that turn the open buffer into a Game of Life genome. See `docs/SEEDS.md`.
- Rule selection: 28-rule built-in catalog (`crates/nit-gol/assets/rules.toml`), custom B/S input, user overlay (`~/.config/nit/rules.toml`). See `docs/RULES.md`.
- Snapshots land in `gol-snapshots/` (async, bounded, deduped).

## Games

- Launch: `nit games [path]` (opens `games.toml` by default).
- Run tournament: `Ctrl+Enter` or `:games run`; hide/show: `H` / `Ctrl+^`.
- Outputs land in `runs/games/` under the workspace root.
- Optional Metal GPU acceleration on macOS (`engine.accelerator = "auto" | "cpu" | "metal"` in `games.toml`).
- Headless CLI: `nit games {run | sweep | enumerate fsm | inspect | graph}` — see `docs/GAMES.md`.

For strategy types (FSM, CA, one-sided TM), config format (payoff, history, scoring, engine), headless CLI, and analysis: see `docs/GAMES.md`.

## Multipane

`nit multipane [--backend <model>] [--panes N] [--cwd PATH]` opens a grid of N independent chat panes (default 8, range `1..=32`), each anchored at its own working directory. `--backend` is optional: omit for a per-pane roster picker, name a family (`claude` / `codex` / `gemini` / `local`) to filter the per-pane roster, or name a specific lane id to pre-pick every pane. Per-pane sessions persist to `<state_dir>/multipane/session-<workspace-hash>.json`.

Per-pane keymap: Tab / Shift+Tab cycle focus, mouse click focuses a pane directly, `Ctrl+Q` quits cleanly, `F1` / `?` toggles the help overlay, `Ctrl+/` (or `F2`) opens the dir-search overlay, `Ctrl+R` reverts a pane to its roster picker. `/abort`, Ctrl+C (empty), Esc-Esc target the focused pane only.

See `docs/MULTIPANE.md` for the full spec.

## Known limitations (MVP)

- Horizontal scrolling uses character columns; tabs before the viewport can shift alignment.
- Syntax highlighting covers 28 languages (the canonical list lives in `crates/nit-core/src/languages.rs`) and falls back to plain text for the rest or for very large files.
- Dockerfile detection is wired (it's an entry in the central `LANGUAGES` table) but the grammar crate is pinned to an older tree-sitter ABI; renders as plain text until upstream catches up.
- Gemini models appear in the roster but are display-only (no runtime runner).

## License

MIT © 2026 nit contributors
