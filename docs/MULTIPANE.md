# Multipane Mode

> **Status**: shipped. Phase 1–6 are live. Outstanding follow-ups
> (cross-pane @all-panes broadcast, Ctrl+Q confirm dialog) are tracked
> as out-of-scope items at the bottom of this document.

## Vision

A second launch mode for nit that opens a grid of independent **chat
panes**, each operating in its own working directory, all backed by a
single user-chosen agent backend. Editor, agent ops, visualizer, and
the rest of the standard nit UI are unavailable in this mode — only
chat dispatch.

```
MULTIPANE  pane 1/9  cwd=nit
┌── pane 0 · roster · nit ─────┬── pane 1 · roster · nit ─────┬── pane 2 · roster · nit ─────┐
│ ↑/↓ j/k · h/l fold · Tab pane│ ↑/↓ j/k · h/l fold · Tab pane│ ↑/↓ j/k · h/l fold · Tab pane│
│ Template: [lab] parallel bulk│ Template: [lab] parallel bulk│ Template: [lab] parallel bulk│
│ Mission:  [auto] general  …  │ Mission:  [auto] general  …  │ Mission:  [auto] general  …  │
│                              │                              │                              │
│  → ▸ Codex                   │    ▸ Codex                   │    ▸ Codex                   │
│    ▸ Claude                  │    ▸ Claude                  │    ▸ Claude                  │
│    ▸ Gemini                  │    ▸ Gemini                  │    ▸ Gemini                  │
│    ▸ Local                   │    ▸ Local                   │    ▸ Local                   │
├── pane 3 · roster · nit ─────┼── pane 4 · roster · nit ─────┼── pane 5 · roster · nit ─────┤
│ ↑/↓ j/k · h/l fold · Tab pane│ ↑/↓ j/k · h/l fold · Tab pane│ ↑/↓ j/k · h/l fold · Tab pane│
│ Template: [lab] parallel bulk│ Template: [lab] parallel bulk│ Template: [lab] parallel bulk│
│ Mission:  [auto] general  …  │ Mission:  [auto] general  …  │ Mission:  [auto] general  …  │
│                              │                              │                              │
│    ▸ Codex                   │    ▸ Codex                   │    ▸ Codex                   │
│    ▸ Claude                  │    ▸ Claude                  │    ▸ Claude                  │
│    ▸ Gemini                  │    ▸ Gemini                  │    ▸ Gemini                  │
│    ▸ Local                   │    ▸ Local                   │    ▸ Local                   │
├── pane 6 · roster · nit ─────┼── pane 7 · roster · nit ─────┼── pane 8 · roster · nit ─────┤
│ ↑/↓ j/k · h/l fold · Tab pane│ ↑/↓ j/k · h/l fold · Tab pane│ ↑/↓ j/k · h/l fold · Tab pane│
│ Template: [lab] parallel bulk│ Template: [lab] parallel bulk│ Template: [lab] parallel bulk│
│ Mission:  [auto] general  …  │ Mission:  [auto] general  …  │ Mission:  [auto] general  …  │
│                              │                              │                              │
│    ▸ Codex                   │    ▸ Codex                   │    ▸ Codex                   │
│    ▸ Claude                  │    ▸ Claude                  │    ▸ Claude                  │
│    ▸ Gemini                  │    ▸ Gemini                  │    ▸ Gemini                  │
│    ▸ Local                   │    ▸ Local                   │    ▸ Local                   │
└──────────────────────────────┴──────────────────────────────┴──────────────────────────────┘
MULTIPANE · Tab cycle · Ctrl+Q quit · F1 help
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
  - Plain text → fuzzy-match subdirectories of `pane.cwd` recursively.
  - `../<query>` → search children of `pane.cwd`'s parent (one level up).
  - `../../<query>` → search children of grandparent (two levels up).
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
| `(empty)` | Show children of cwd (the immediate subdirectories) |
| `foo` | Recursive fuzzy match of subdirectories under cwd, displayed as `parent/foo/match/` breadcrumbs |
| `../` | Show children of cwd's parent |
| `../foo` | Recursive fuzzy match under cwd's parent containing "foo" |
| `../../` | Show children of cwd's grandparent |
| `../../foo` | Recursive fuzzy match under cwd's grandparent containing "foo" |
| `/abs/path` | Treat as absolute, match descendants |
| `~/foo` | Expand `~` and search |

Gitignored bare-name directories (read from the workspace `.gitignore`
at startup) and the heavyweight build dirs (`node_modules`, `target`,
`.venv`, `dist`, `build`) are filtered at the walker source so they
never reach the dropdown.

Results render below the search bar as a dynamically sized dropdown
(3 to 16 rows, sized to fit the pane). Up/Down or Ctrl+J/Ctrl+K move
the highlight; passing the bottom row scrolls the viewport, and the
inverse at the top. Right or Ctrl+L expands the highlighted directory
in place (its children indent one level beneath it); Left or Ctrl+H
collapses. Enter commits the highlighted entry as the new pane cwd;
Esc cancels and resumes the chat thread underneath. Home / End jump
the input cursor to the start / end of the query. Typing more
characters narrows the list live; the recursive walk runs on a
background thread, and a fresh keystroke supersedes any in-flight walk
so the UI never blocks. Length penalty in the fuzzy ranker means a
short relative path (`nit-tui/`) outranks a deeper match
(`crates/nit-tui/`); refine with a deeper prefix when needed.

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

When a pane has no committed selection, the body of the pane renders the
same tree the Agent OPS Roster tab does:

```
 Template:  lab   parallel   bulk
 Mission:   auto   general   research   computational

 ▾ Codex
   ↳ gpt-5  [Codex]
       [x] medium
       [ ] high
 ▸ Claude
 ▸ Gemini
 ▸ Local
```

- The `Template:` and `Mission:` rows control swarm defaults
  (`state.agents.swarm_default_template` /
  `state.agents.swarm_default_mission`). Click a word to set it. These
  writes are global — clicking from one pane affects all panes and the
  Agent OPS dock.
- Backend headers expand / collapse per pane. Expansion is purely
  cursor-driven: at most one backend group is visible at a time
  (whichever the cursor is on, via `PaneSession.auto_expanded_backend`),
  and the group collapses the moment the cursor walks off it. Two
  panes can be on different backends because the cursor is per-pane.
- Each agent under an expanded backend shows a `↳ Size` branch with one
  `[x]` checkbox per supported reasoning effort. Toggling sets the
  effort in the global `codex_selected_reasoning_effort` /
  `claude_selected_effort` map (matching Agent OPS behaviour).

| Key | Action |
|---|---|
| `↑` / `k`, `↓` / `j` | Move the per-pane cursor through selectable rows (Backend, Agent, SizeBranch, SizeLeaf). Template / Mission rows are skipped — they are click-only. |
| `→` / `l` | Expand the focused row: opens a Backend group, un-collapses an Agent's tree, etc. |
| `←` / `h` | Collapse: closes a Backend group, hides an Agent's Size leaves. |
| `PgUp` / `PgDn` | Jump the cursor by a page (8 rows). |
| `g` / `G` | Jump cursor to the first / last selectable row. |
| `Space` | Toggle the checkbox under the cursor when it sits on a SizeLeaf. |
| `Enter` | Commit: Backend → toggle expand; Agent → materialise `<base>#mp-pane-NN` and switch the pane to chat mode; SizeBranch → toggle agent tree collapse; SizeLeaf → toggle the checkbox. |
| `Tab` / `Shift+Tab` | Cycle focus between panes (never moves the roster cursor). |
| `Mouse left-click` | Focuses the clicked pane and routes to the row under the cursor: Template/Mission word → set the global default; Backend → toggle expand; Agent → materialise + switch to chat; SizeBranch → toggle tree; SizeLeaf → toggle checkbox. |
| `Mouse wheel` | Scrolls the pane's roster viewport (`PaneSession.roster_scroll`). |
| `Ctrl+C`, `Esc Esc` | Emits the no-op "no agent selected — nothing to abort" notice. |

When a pane is already in chat mode:

| Key | Action |
|---|---|
| `Ctrl+R` | Revert the focused pane back to roster mode. Clears `selected_agent_id`, the chat input buffer, and the active mission. The original roster cursor / expansion state are preserved per pane. |
| `PgUp` / `PgDn` | Scroll the chat thread (`PaneSession.chat_thread_scroll`). The auto-stick-to-bottom default returns when the operator scrolls back to 0. |
| `Mouse wheel` | Scrolls the focused pane's chat thread. Wheel events on a different pane don't steal focus — the wheel always targets the pane under the cursor. |

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
    pub roster_scroll: usize,                 // viewport top row inside the roster body
    pub auto_expanded_backend: Option<AgentLaneKind>,     // cursor-driven, single-backend latch
    pub auto_expanded_agent: Option<String>,              // cursor-driven, mirrors the agent row
    pub roster_collapsed_agent_ids: HashSet<String>,      // pane-local Size/Role collapse
    pub roster_tree_selected: Option<RosterTreeSelection>, // leaf cursor inside Size branch
    pub chat_thread_scroll: usize,            // separate from chat_input_scroll
    pub selected_agent_id: Option<String>,    // None ⇒ render roster picker
}

pub struct DirSearchState {
    pub query: String,
    pub query_cursor: usize,
    pub results: Vec<PathBuf>,                // populated by dir_search_runner
    pub selected: usize,
    pub generation: u64,                      // request-id latch, runner drops stale results
    pub show_hidden: bool,                    // Alt+f toggle
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

`crates/nit-tui/src/widgets/agent_console_view::render` (in `agent_console_view/mod.rs`) already
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

## Implementation notes

> The original spec carried a six-phase delivery plan plus copy-paste
> coding-agent prompts. All phases shipped; the historical material has
> been removed. Key landed surfaces, with current source paths:
>
> - **CLI + state types** — `crates/nit/src/cli/mod.rs::Command::Multipane`,
>   `MultipaneArgs`; `crates/nit-core/src/state/multipane.rs` for
>   `PaneSession`, `DirSearchState`, `MultipaneState`; re-exports in
>   `crates/nit-core/src/lib.rs`.
> - **Launch wiring** — `crates/nit/src/multipane_setup.rs` materialises
>   the pane roster, validates `--backend`, and forwards into the TUI.
> - **Render + event loop** — `crates/nit-tui/src/multipane/mod.rs`,
>   `runtime/`, `grid.rs`, `focus.rs`, `roster_view.rs`. Standard mode is
>   untouched (no impact when `state.multipane.is_none()`).
> - **Per-pane chat dispatch** — `crates/nit-tui/src/multipane/dispatch.rs`
>   wraps `app::chat_input::submit_chat_input_and_dispatch` with a
>   `with_pane_aliased` shim that injects the pane's `cwd` + agent id.
> - **Dir search** — `crates/nit-tui/src/multipane/dir_search.rs` (pure
>   parser + ranker) and `dir_search_runner.rs` (async walker, mirrors
>   `fuzzy_search_runner` and reuses `fuzzy_score_bytes`).
> - **Locked-down key map** — `multipane::runtime::handle_key` allow-lists
>   Tab / Shift+Tab / Enter / Ctrl+/ / F2 / Ctrl+R / Ctrl+C / Esc / Ctrl+Q
>   / F1 / `?` / character / mouse and silently swallows everything else.
> - **Persistence** — `crates/nit-tui/src/multipane/persistence.rs` writes
>   `<state_dir>/multipane/session-<workspace-hash>.json` on Ctrl+Q and on
>   focus change (debounced ≤ 1 write/sec). `chat_input` is capped at 4 KB.
>   A "fresh" Ctrl+Q (no prior file, no mission run) drops the file rather
>   than persisting an empty layout.
> - **Tests** — `crates/nit-tui/src/tests/multipane_integration.rs` (5
>   tests: per-pane cwd dispatch, focused-pane abort isolation,
>   no-agent-selected notice, dir-search cwd commit, persistence
>   roundtrip).

### Notable deviations from the original phase plan

- The Phase-5 standalone `key_dispatch.rs` was collapsed into
  `multipane/runtime/` once the allow-list shrank below the threshold
  where a separate module was paying for itself.
- Ctrl+Q ships without a confirm dialog. Multipane has no popup
  state machine, and persistence already snapshots typed prompts on
  disk, so an accidental Ctrl+Q is recoverable.
- Persistence dropped the original "across nit restarts" caveat and
  instead lands `<state_dir>/multipane/session-<workspace-hash>.json`
  on Ctrl+Q and on focus change (debounced ≤ 1 write/sec). `chat_input`
  is capped at 4 KB on save; a "fresh" Ctrl+Q (no pane has run a
  mission and no prior file existed) drops the file rather than
  persisting an empty layout.

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

## Resolved decisions

1. **Agent-id namespace**: pane lanes use `<base>#mp-pane-NN`
   (zero-padded, two digits). Distinct from `#chat-clone-` and
   `#swarm-` separators so the runner's id-keyed maps never collide.
2. **Persistence**: shipped. `<state_dir>/multipane/session-<hash>.json`
   stores per-pane `cwd`, `chat_input`, history, swarm_template /
   swarm_mission, `selected_agent_id`, and the focused index. UI-only
   fields (`help_open`, dir-search overlay, roster auto-expansion
   latches) are `#[serde(skip)]` and start fresh on each launch.
3. **Backend specificity**: `--backend <specific-id>` pre-picks every
   pane; `--backend <family>` filters the per-pane roster to that
   family; omitting the flag shows the full roster. All three modes
   ship.
4. **Mission scope**: per-pane mission, no cross-pane swarms. Each
   pane carries its own `mission_id` field on `PaneSession`.
5. **Resize handling**: when per-pane width drops below 20 cells or
   height below 10 rows, the runtime renders a single centered
   "Terminal too small for N panes — resize or relaunch with
   --panes <smaller>" paragraph instead of the grid.
6. **Ctrl+Q without confirm dialog (deviation)**: the original spec
   asked for a confirmation prompt before exiting. v1 ships without
   one — multipane has no popup state machine yet, and persistence
   already preserves typed prompts on disk so an accidental Ctrl+Q is
   recoverable. A confirm dialog can land as a follow-up.

## Out of scope (v1)

- Splitting / closing panes at runtime. Layout is fixed at launch.
- Different backends per pane (each pane picks once from its per-pane
  roster; flip back via `Ctrl+R` to re-select).
- Cross-pane operations (broadcast one prompt to every pane). `/abort all`
  already cancels across panes, but there is no `@all-panes <prompt>`
  dispatch helper yet.
- Mouse drag to resize pane boundaries.

