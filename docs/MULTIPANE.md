# Multipane Mode

> **Status**: design proposal. Not yet implemented. This doc is the
> authoritative plan + the prompt to feed coding agents.

## Vision

A second launch mode for nit that opens a grid of independent **chat
panes**, each operating in its own working directory, all backed by a
single user-chosen agent backend. Editor, agent ops, visualizer, and
the rest of the standard nit UI are unavailable in this mode — only
chat dispatch.

```
$ nit multipane --backend claude-haiku-4-5 --panes 8

┌── pane 0 ─────┬── pane 1 ─────┬── pane 2 ─────┬── pane 3 ─────┐
│ cwd: nit/     │ cwd: ../web/  │ cwd: ../infra │ cwd: ~/scripts│
│               │               │               │               │
│  agent chat   │  agent chat   │  agent chat   │  agent chat   │
│  (focused)    │               │               │               │
│               │               │               │               │
│ > ___         │ > ___         │ > ___         │ > ___         │
├── pane 4 ─────┼── pane 5 ─────┼── pane 6 ─────┼── pane 7 ─────┤
│ ...           │ ...           │ ...           │ ...           │
└───────────────┴───────────────┴───────────────┴───────────────┘
```

Use case: drive N concurrent agent sessions across different projects
from one terminal. Like `tmux` for AI agents.

## Hard requirements (from operator brief)

- **`--backend` is optional**. When supplied (specific lane id or family
  alias), the chosen backend applies to **every pane** — no per-pane
  backend override is allowed. When omitted, every pane opens in
  **roster mode** showing every available backend, and the operator
  picks per pane independently.
- Each pane is **its own session** (chat history, mission state, queued
  turns, in-flight work) anchored at its own **cwd**.
- Agent dispatches from a pane operate inside that pane's cwd —
  subprocess `Command::current_dir(pane.cwd)`.
- A **dir search** at the top of each pane:
  - Plain text → fuzzy-match subdirectories of `pane.cwd`.
  - `../<query>` → search siblings of `pane.cwd` (one level up).
  - `../../<query>` → search siblings of parent (two levels up).
  - … and so on for `../../../`.
  - Enter / select → switch the pane's cwd to the matched directory.
- **Search must be fast** — feels instant on directory trees of 10k+
  entries.
- Default panes per launch: **8** (4 wide × 2 tall) — fit into a normal
  terminal. Operator override: `--panes N`.
- Navigation: **Tab** / **Shift+Tab** to cycle, **mouse click** to
  focus directly.
- In multipane mode, **only the chat pane works**. No editor, no agent
  ops dock, no visualizer. To use those, exit multipane and launch
  nit normally.

## CLI surface

New subcommand on the existing `clap` enum:

```bash
nit multipane                                          # 8 panes, full roster per pane
nit multipane --panes 4                                # 4 panes, full roster per pane
nit multipane --backend claude                         # 8 panes, all locked to Claude family
nit multipane --backend gpt-5 --panes 6                # 3×2 grid, all pre-picked to gpt-5
nit multipane --backend claude-haiku-4-5 --panes 4     # 4 panes, all pre-picked
nit multipane --backend claude --panes 8 --cwd /work   # full composition
```

Note: the operator's shorthand `nit multipane --backend claude 5 --panes 8`
includes a stray positional `5` that clap rejects. The CLI surface is
strictly `--backend <id-or-family>` + `--panes <N>` + `--cwd <path>`.

`--panes` clamps to `[1, 32]`. The grid layout chooses dimensions to
keep panes roughly square (computed as `ceil(sqrt(N))` columns).

`--backend` accepts either a specific lane id (`claude-haiku-4-5`,
`gpt-5`, …) or one of four reserved family aliases — `codex`, `claude`,
`gemini`, `local` — case-insensitive. A specific lane id pre-picks every
pane (the lane is cloned into `state.agents.agents` as
`<id>#mp-pane-NN` and the pane lands directly in chat). A family alias
filters the per-pane roster but leaves panes unselected; the operator
picks within that family. Omitting `--backend` shows the full roster
with per-pane independence. Unknown specific ids exit with the available
ones listed.

## UX spec

### Per-pane layout

```
┌─[pane 0]─ cwd: /Users/me/code/nit ──────────────┐
│ search:  __                                     │  ← 1-row dir search
│ ─────────────────────────────────────────────── │
│ ↳ [haiku] done (see ARTIFACTS)                  │
│   You: refactor crates/nit-utils/src/lib.rs     │  ← chat thread
│ ↳ [haiku] Working ...                           │     (existing
│                                                 │      agent_console_view)
│                                                 │
│ ─────────────────────────────────────────────── │
│ ↳ /abort · Ctrl+C · Esc Esc                     │  ← hint strip
│ ┌── CHAT BOX ────────────────────────────────── ┐
│ │                                               │  ← input
│ └────────────────────────────────────────────── ┘
└─────────────────────────────────────────────────┘
```

The middle and bottom (chat thread + hint + input box) are the
**existing** `agent_console_view::render` output, parameterised on a
per-pane state. Top row is new.

### Dir search modes

The dir search bar at the top accepts free text. The parser looks at
the prefix:

| Input | Meaning |
|---|---|
| `(empty)` | Show siblings of cwd (same as `../`) |
| `foo` | Fuzzy match subdirectories of cwd whose name contains "foo" |
| `../` | Show siblings of cwd |
| `../foo` | Fuzzy match siblings of cwd containing "foo" |
| `../../` | Show siblings of cwd's parent |
| `../../foo` | Fuzzy match siblings of cwd's parent containing "foo" |
| `/abs/path` | Treat as absolute, match descendants |
| `~/foo` | Expand `~` and search |

Results render below the search bar as a small dropdown (max 10 rows).
Up/Down to move, Enter to commit, Esc to cancel and resume the chat
thread underneath. Typing more characters narrows the list live.

When the user commits a directory, the pane:
1. Updates `pane.cwd`.
2. Pushes a system message to the pane: `cwd → /new/path`.
3. The next `@swarm`/free-text dispatch uses the new cwd.

### Focus & input routing

- One pane has focus at any time, drawn with a brighter border.
- Tab cycles forward, Shift+Tab cycles backward. Tab/Shift+Tab never
  move the per-pane roster cursor — they only switch which pane has
  focus.
- Mouse click anywhere inside a pane focuses it.
- Only the focused pane receives keyboard input. Background panes
  still update (turn output streams in, "Working..." breather animates,
  etc.).
- `/abort`, `/abort all`, `/abort <agent-id>`, Ctrl+C with empty input,
  and Esc-Esc within ~500 ms target the focused pane (or every pane,
  for `all`). Issuing them in a roster-mode pane (no committed
  selection) emits a one-line "no agent selected — nothing to abort"
  notice in the pane's chat history rather than a silent drop.

### Roster mode keymap

When a pane has no committed selection, the body of the pane renders a
filtered roster of available agent lanes (grouped by backend family).

| Key | Action |
|---|---|
| `↑` / `↓` | Move the per-pane cursor through visible lanes (skips backend headers). |
| `Enter` | Commit the highlighted lane: lazily clones it as `<base>#mp-pane-NN`, copies runtime metadata, switches the pane to chat mode. |
| `Tab` / `Shift+Tab` | Cycle focus between panes (never moves the roster cursor). |
| `Ctrl+C`, `Esc Esc` | Emits the no-op "no agent selected — nothing to abort" notice. |
| `Mouse click` | Focuses the clicked pane (no per-row selection in v1). |

When a pane is already in chat mode:

| Key | Action |
|---|---|
| `Ctrl+R` | Revert the focused pane back to roster mode. Clears `selected_agent_id`, the chat input buffer, and the active mission. The original roster cursor position is preserved per pane. |

The four reserved family aliases (`codex`, `claude`, `gemini`, `local`)
filter the roster to a single backend family. If the filter matches no
installed lanes (e.g. `--backend gemini` on a host without the Gemini
CLI), the pane shows a single "No <family> agents detected — install
the CLI" line instead of crashing or exiting.

### Disabled features

In multipane mode:
- No editor pane, no Notes pane, no file tree, no Agent Ops dock, no
  visualizer pane.
- Global keybindings that would have switched focus to those panes
  are no-ops (or absent).
- `@swarm` still works inside a pane — it routes to the per-pane agent
  the same way.
- `/abort`, Ctrl+C (empty), Esc-Esc still work in the focused pane.

## State model

### New core types (in `nit-core/src/state/multipane.rs`)

```rust
pub struct PaneSession {
    pub pane_id: usize,                       // 0..N-1; stable for the run
    pub agent_id: String,                     // empty until selection commits
    pub cwd: PathBuf,                         // working directory for this session
    pub chat_input: String,
    pub chat_input_cursor: usize,
    pub chat_input_selection_anchor: Option<usize>,
    pub chat_input_scroll: usize,
    pub chat_prompt_history: Vec<String>,
    pub chat_prompt_history_pos: Option<usize>,
    pub dir_search: Option<DirSearchState>,   // Some when search is active
    pub mission_id: Option<String>,           // current mission anchored to this pane
    pub roster_cursor: usize,                 // position inside the per-pane roster
    pub selected_agent_id: Option<String>,    // None ⇒ render roster picker
}

pub struct DirSearchState {
    pub query: String,
    pub query_cursor: usize,
    pub results: Vec<PathBuf>,                // populated by fuzzy_dir_search runner
    pub selected: usize,
    /// Index into the directory tree we're searching in. Computed from
    /// query prefix (`../` count) at parse time.
    pub base: PathBuf,
}

pub struct MultipaneState {
    pub backend_agent_id: String,             // operator's --backend verbatim (or "")
    pub panes: Vec<PaneSession>,
    pub focused: usize,                       // 0..panes.len()-1
    pub grid_cols: usize,                     // computed at launch
    pub grid_rows: usize,
    pub backend_filter: Option<String>,       // family alias / specific id / None
}
```

`AppState` gets one new optional field:

```rust
pub multipane: Option<MultipaneState>,
```

When `Some`, render the multipane UI; ignore standard panes/widgets.
When `None` (the default), nit behaves exactly as it does today —
zero impact on existing flows.

### Why this shape

- **Messages stay in `state.agents.messages`** keyed by `agent_id`, as
  today. Each pane has a unique agent_id (`<base>#pane-K`), so the
  message renderer naturally filters per-pane without a duplicate
  Vec.
- **Active turns + queues** stay in `state.agents.active_turns` /
  `queued_*_turns` keyed by agent_id. The runner already supports
  arbitrary agent_id values; the per-pane suffix is enough.
- **Mission + swarm state** stays in `SwarmRuntime`. A pane's swarm
  mission is just a regular swarm mission with the pane's agent_id as
  planner.
- **Per-pane state is small** (input buffer, history, search) — fits
  cleanly in `PaneSession` without restructuring the rest of state.

### Reusing the existing chat renderer

`crates/nit-tui/src/widgets/agent_console_view.rs::render` already
consumes `&AppState` and assumes it's drawing the *one* chat pane.
We extract a thinner core that takes a `&PaneSession` plus the
shared `state` for messages — call it `render_pane(...)`. The
existing `render` becomes a thin wrapper for backward compat
(non-multipane mode).

## CLI parsing

```rust
// crates/nit/src/cli/mod.rs
#[derive(Subcommand, Debug)]
pub enum Command {
    Gol { ... },
    Games { ... },
    Multipane(MultipaneArgs),
}

#[derive(Args, Debug)]
pub struct MultipaneArgs {
    /// Backend model id (specific lane like `claude-haiku-4-5`) or
    /// family alias (`codex` / `claude` / `gemini` / `local`). Optional —
    /// when omitted, every pane opens in roster mode.
    #[arg(long)]
    pub backend: Option<String>,

    /// Number of panes to open. Clamped to [1, 32]. Grid is roughly
    /// square: ceil(sqrt(N)) columns × ceil(N / cols) rows.
    #[arg(long, default_value_t = 8u8, value_parser = clap::value_parser!(u8).range(1..=32))]
    pub panes: u8,

    /// Starting directory for every pane. Defaults to the current
    /// working directory.
    #[arg(long)]
    pub cwd: Option<PathBuf>,
}
```

## Phased plan

### Phase 1 — CLI + skeleton state (no UI yet)

Files:
- `crates/nit/src/cli/mod.rs` — add `Command::Multipane`
- `crates/nit-core/src/state.rs` — `PaneSession`, `DirSearchState`,
  `MultipaneState`; add `pub multipane: Option<MultipaneState>` to
  `AppState`
- `crates/nit-core/src/lib.rs` — re-export new types
- `crates/nit/src/main.rs` — branch on `Command::Multipane`, validate
  backend, populate `state.multipane = Some(...)`

Acceptance:
- `nit multipane --backend claude-haiku-4-5` builds N pane sessions
  in state and exits without rendering (or renders a stub).
- Missing `--backend` produces a clean error.
- Unknown backend produces an error listing available choices.
- Tests: backend validation table, grid-shape computation table.

### Phase 2 — Grid layout + per-pane render

Files:
- `crates/nit-tui/src/app/runner.rs` — when
  `state.multipane.is_some()`, dispatch to a new
  `multipane::run_loop` instead of the standard one.
- `crates/nit-tui/src/multipane/mod.rs` (new) — main render +
  event loop for multipane mode.
- `crates/nit-tui/src/widgets/agent_console_view.rs` — split
  `render` so a thinner core can be called per-pane.

Acceptance:
- Empty 8-pane grid renders without errors.
- Each pane has its own border and cwd shown in the header.
- Tab cycles focus, focused pane has a brighter border.
- Mouse click focuses a pane.
- Tests: grid-shape for N=1, 2, 4, 6, 8, 16, 32.

### Phase 3 — Per-pane chat dispatch

Files:
- `crates/nit-tui/src/multipane/dispatch.rs` — wraps existing
  `app::dispatch::dispatch_agent_prompt` but injects the pane's cwd
  and agent_id.
- Possibly extend `CodexCommand::RunTurn` / `ClaudeCommand::RunTurn`
  with `cwd` (already there) — verify it's plumbed.

Acceptance:
- Type a prompt in pane 0, hit Enter, agent runs with the pane's
  cwd. Type a different prompt in pane 3, runs with pane 3's cwd.
- Both panes show their own thread independently.
- Existing `/abort`, Ctrl+C, Esc-Esc still work in the focused pane.
- Tests: dispatch test with mock runner, asserts cwd argument.

### Phase 4 — Dir search

Files:
- `crates/nit-tui/src/multipane/dir_search.rs` (new) — parser for
  the `../`-prefix grammar, fuzzy match algorithm, results.
- `crates/nit-tui/src/multipane/dir_search_runner.rs` (new) — async
  worker thread that walks directories and fuzzy-matches without
  blocking the UI.

Performance:
- Index strategy: lazy walk on first activation per `(base, depth)`,
  cache results in `state.multipane.dir_index_cache`.
- For each pane, cap walk depth at 4 unless the user explicitly
  navigates deeper.
- Use `std::fs::read_dir` + a small worker pool (rayon) for
  parallel directory enumeration.
- Fuzzy match algorithm: subsequence match with bonus for
  consecutive characters and word-boundary hits. Reuse logic from
  `fuzzy_search_runner.rs` if practical.

UX:
- A dedicated key (e.g. `Ctrl+/` or just typing in the top row)
  activates dir search mode in the focused pane.
- Esc cancels and returns to chat input.
- Enter on a result switches `pane.cwd` and pushes a system message.
- Up/Down navigate results.

Acceptance:
- Typing `foo` in dir search shows subdirectories of cwd containing
  "foo", ranked by fuzzy score.
- `../foo` ranks siblings of cwd, `../../foo` siblings of parent.
- Enter switches cwd; system message confirms.
- Search results update inside 16ms for 10k-entry trees.
- Tests: parser for prefix counting, fuzzy-rank ordering, cwd-switch
  state mutation.

### Phase 5 — Disable non-pane keys + polish

Files:
- `crates/nit-tui/src/multipane/key_dispatch.rs` (new) — gate the
  global key handler so unrelated keys (Editor focus, Visualizer,
  etc.) become no-ops.
- `crates/nit-tui/src/widgets/top_bar.rs` (or its multipane variant)
  — hide LAB / GOL / MOOD chrome in this mode.

Acceptance:
- Pressing keys that would normally focus the editor or
  visualizer in regular nit do nothing in multipane mode.
- Status bar reflects multipane mode.

### Phase 6 — Tests + docs

- Unit tests for: backend validation, grid math, dir-search parser,
  fuzzy ranking, per-pane dispatch, focus cycling, message
  filtering.
- Integration test: launch headless multipane, dispatch a prompt to
  each of 4 panes with mock runners, assert each saw its own cwd.
- Update `docs/KEYBINDINGS.md` with multipane-specific bindings.
- Update `CLAUDE.md` with the new subcommand and env vars (none new
  expected, but flag presence).

## Performance budget

| Op | Target | Notes |
|---|---|---|
| Initial render of 16-pane grid | < 50 ms | Same render code per pane; one ratatui frame |
| Dir search keystroke → result update | < 16 ms | One frame budget; runs on async worker |
| Walk a 10k-entry directory tree | < 100 ms | Parallelised via rayon, cached after first walk |
| Focus switch (Tab) | < 1 ms | Pure state mutation, no IO |
| Pane redraw on background turn output | < 16 ms | Reuses existing render path |

The chat-thread render path is already ratatui — fast. The dominant
cost is the dir walk, which is why we cache and amortise.

## Open questions (to resolve during implementation)

1. **Agent identity collisions**: if the user has `claude-haiku-4-5`
   in their roster already, our pane agent_id `claude-haiku-4-5#pane-0`
   collides with naming conventions for swarm/chat clones. Pick a
   different separator (e.g. `:pane:`) or namespace (`mp-pane-0`)?
2. **Persistence**: should `pane.cwd` and chat history persist across
   nit restarts? Probably yes — write to
   `<state_dir>/multipane/session-<hash>.json`. Nice-to-have, not
   blocking.
3. **Backend specificity**: `--backend claude-haiku-4-5` (specific
   model) vs `--backend claude` (family, pick default)? Start with
   specific; add family as a convenience later.
4. **Single mission across panes vs per-pane mission**: each pane is
   independent — one mission per pane, max. Cross-pane swarms are out
   of scope.
5. **Resize handling**: when the terminal resizes below the minimum
   per-pane size, show a single "terminal too small" message rather
   than rendering broken panes.

## Out of scope (v1)

- Splitting / closing panes at runtime. Layout is fixed at launch.
- Different backends per pane.
- Cross-pane operations (broadcast a prompt to all panes).
  *(But — this could be `@all-panes <prompt>`. Easy follow-up.)*
- Persistence of pane sessions across nit restarts.
- Mouse drag to resize pane boundaries.

## Coding-agent prompt

Copy-paste-ready brief for `@swarm` or a follow-up Claude/Codex run.
Reference this doc as the source of truth.

---

> **Goal**: Implement multipane mode for nit per `docs/MULTIPANE.md`.
>
> **Scope**: Phases 1-3 of the plan in one mission (CLI + skeleton
> state, grid layout + render, per-pane dispatch). Phases 4-6 are
> follow-up missions.
>
> **Constraints**:
> - Follow `docs/MULTIPANE.md` exactly. Deviations need a comment in
>   the doc explaining why.
> - **Do not modify** any code path that runs when `state.multipane`
>   is `None` — multipane mode must be additive.
> - **Single backend per launch**: validate `--backend` at startup;
>   reject unknown models with a list of valid ones.
> - All new code must pass `just clippy` (no warnings) and have unit
>   tests in the relevant `tests/` module.
> - When the integrator finishes, total `cargo test --all` must
>   match the previous count + new tests.
>
> **Deliverables**:
> 1. `Command::Multipane` in `crates/nit/src/cli/mod.rs` (Phase 1)
> 2. `PaneSession` / `DirSearchState` / `MultipaneState` types in
>    `crates/nit-core/src/state.rs` + re-exports (Phase 1)
> 3. `crates/nit-tui/src/multipane/mod.rs` with the multipane event
>    loop (Phase 2)
> 4. Per-pane render reusing `agent_console_view` core (Phase 2)
> 5. Per-pane dispatch with cwd injection (Phase 3)
> 6. `/abort`, Ctrl+C, Esc-Esc still work in the focused pane
>    (regression check, no new behaviour)
> 7. Updated `CLAUDE.md` and `docs/KEYBINDINGS.md` with the new
>    subcommand
> 8. Unit tests for: backend validation, grid math (N → cols × rows),
>    cwd injection, focus cycling
>
> **Out of scope for this mission**: dir search (Phase 4), feature
> gating polish (Phase 5), persistence. Leave TODOs and create
> follow-up work items.
>
> **Verification**:
> - `nit multipane --backend claude-haiku-4-5 --panes 4` launches 4 panes
>   already pre-picked to that lane and ready for chat.
> - `nit multipane --backend claude --panes 4` launches 4 panes whose
>   roster is filtered to the Claude family; operator picks per pane.
> - `nit multipane --panes 4` launches 4 panes showing the full roster;
>   each pane picks independently with `↑/↓` + `Enter`.
> - Tab cycles pane focus, mouse click focuses, focused pane has a
>   brighter border. Up/Down stays scoped to the focused pane's roster
>   cursor.
> - Typing a prompt in pane 0, then pane 3, runs them in their own
>   cwd (default to `--cwd` arg or process cwd for v1; dir search
>   wires up real switching in Phase 4).
> - `nit` (without multipane) opens the standard editor exactly as
>   before — multipane is purely additive.
>
> **Reading list**:
> - `docs/MULTIPANE.md` (this doc — full plan)
> - `docs/SWARM.md` (how `@swarm` is wired — multipane reuses the
>   per-pane agent dispatch model)
> - `crates/nit-tui/src/widgets/agent_console_view.rs::render`
>   (chat-pane render — to be split into a per-pane core)
> - `crates/nit-tui/src/app/runner.rs::run_loop` (current event loop
>   — multipane will branch off it)
> - `crates/nit-tui/src/codex_runner.rs` /
>   `crates/nit-tui/src/claude_runner.rs` (already accept per-turn
>   `cwd: PathBuf` — no changes needed there)
>
> Use `template=lab` for this mission (single-writer integrator,
> read-only proposers/reviewer/test) to keep the change set
> coherent and the workspace consistent.
