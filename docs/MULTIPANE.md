# Multipane Mode

Multipane is a second launch mode. It opens a grid of independent chat panes,
each with its own working directory and its own agent session, in one
terminal. Think `tmux` for AI agents. This doc covers the CLI, the pane
layout, dir search, keys, persistence, and limits, with a short contributor
section at the end.

![nit multipane mode: sixteen independent agent chat panes in a 4×4 grid](https://nit.tools/multipane.png)

Only chat dispatch works in this mode. There is no editor, file tree, Notes
pane, Agent Ops dock, or visualizer, and the global keys for those do nothing.
Exit multipane and run plain `nit` for them. Each pane can also show an
embedded terminal; see `docs/TERMINAL.md`.

## CLI

```bash
nit multipane                                          # 8 panes, full roster per pane
nit multipane --panes 4                                # 4 panes, full roster per pane
nit multipane --backend claude                         # 8 panes, all locked to the Claude family
nit multipane --backend gpt-5 --panes 6                # 3×2 grid, all pre-picked to gpt-5
nit multipane --backend claude-haiku-4-5 --panes 4     # 4 panes, all pre-picked
nit multipane --backend claude --panes 8 --cwd /work   # full composition
nit multipane --panes 2 --terminal-command "htop" --terminal-command "tail -f app.log"
```

| Flag | Meaning |
|---|---|
| `--panes N` | Number of panes, clamped to 1 to 32. Default 8. |
| `--backend <id-or-family>` | Backend for every pane. Optional. |
| `--cwd <path>` | Starting directory for every pane. Default: the current directory. |
| `--terminal-command <COMMAND>` | Start a pane with its terminal open running this command. Repeatable. |

There are no positional arguments, so `nit multipane --backend claude 5` is
rejected.

The grid stays roughly square: `ceil(sqrt(N))` columns and `ceil(N / cols)`
rows. The default 8 panes give a grid 4 wide and 2 tall.

`--backend` takes a specific lane id such as `claude-haiku-4-5` or `gpt-5`, or
one of four family aliases: `codex`, `claude`, `gemini`, `local`. Aliases are
case-insensitive.

| Form | Effect |
|---|---|
| Specific lane id | Every pane is pre-picked. The lane is cloned as `<id>#mp-pane-NN` and the pane opens in chat mode. |
| Family alias | Each pane's roster shows only that family. You pick per pane. |
| Omitted | Each pane shows the full roster and picks on its own. |

An unknown specific id exits with the available backends listed. If a family
alias matches no installed lane, the pane shows "No <family> agents detected —
install the CLI" instead of exiting.

`--terminal-command` must be given exactly once per pane, in pane order, and
no command may be empty. Otherwise nit exits with an error. Each pane then
starts with its terminal open running that command instead of the chat or
roster view.

## Per-pane layout

```text
┌─[pane 0]─ cwd: /Users/me/code/nit ──────────────┐
│ search:  __                                     │  ← 1-row dir search
│ ─────────────────────────────────────────────── │
│ ↳ [haiku] done (see ARTIFACTS)                  │
│   You: refactor crates/nit-utils/src/lib.rs     │  ← chat thread
│ ↳ [haiku] Working ...                           │
│                                                 │
│                                                 │
│ ─────────────────────────────────────────────── │
│ ↳ /abort · Ctrl+C · Esc Esc                     │  ← hint strip
│ ┌── CHAT BOX ────────────────────────────────── ┐
│ │                                               │  ← input
│ └────────────────────────────────────────────── ┘
└─────────────────────────────────────────────────┘
```

The chat thread, hint strip, and input box are the standard agent chat view,
drawn once per pane. The dir search row is specific to multipane.

Each pane is its own session: chat history, mission state, queued turns, and
in-flight work, all anchored at the pane's cwd. Agents dispatched from a pane
run inside that cwd. Pane agents get the id `<base>#mp-pane-NN` (zero-padded),
which keeps them apart from `#chat-clone-` and `#swarm-` ids.

## Dir search

Press `Ctrl+/` to open the dir search in the focused pane. Some terminals,
including the default macOS Terminal.app, drop `Ctrl+/`; use `F2` there. The
prefix of the query picks where to search:

| Input | Meaning |
|---|---|
| (empty) | Show the immediate subdirectories of cwd |
| `foo` | Recursive fuzzy match under cwd, shown as `parent/foo/match/` breadcrumbs |
| `../` | Show children of cwd's parent |
| `../foo` | Recursive fuzzy match under cwd's parent |
| `../../` | Show children of cwd's grandparent |
| `../../foo` | Recursive fuzzy match under cwd's grandparent |
| `/abs/path` | Treat as absolute and match descendants |
| `~/foo` | Expand `~` and search |

Directories named in the workspace `.gitignore` (read once at startup) and the
build dirs `node_modules`, `target`, `.venv`, `dist`, and `build` never reach
the list. Dotfiles stay hidden until you press `Alt+f`.

Results appear in a dropdown of 3 to 16 rows, sized to the pane.

| Key | Action |
|---|---|
| `Up` / `Down`, `Ctrl+K` / `Ctrl+J` | Move the highlight. The list scrolls past either end. |
| `Right` / `Ctrl+L` | Expand the highlighted directory in place, indented one level. |
| `Left` / `Ctrl+H` | Collapse it. |
| `Home` / `End` | Jump the input cursor to the start or end of the query. |
| `Alt+f` | Toggle hidden directories such as `.git` and `.cache`. |
| `Enter` | Set the pane's cwd to the highlighted entry. |
| `Esc` | Close the search and keep the chat input. A second `Esc` within about 500 ms aborts the pane. |
| `Tab` / `Shift+Tab` | Switch pane focus and close the search. |

Typing narrows the list live. The recursive walk runs on a background thread,
and each new keystroke replaces the walk in flight, so the UI never blocks.
The ranker penalises length, so `nit-tui/` outranks `crates/nit-tui/`; type a
deeper prefix when you need the deeper match.

When you commit a directory, the pane updates its cwd, posts the system
message `cwd → /new/path`, and the next dispatch uses the new directory.

## Focus and input routing

- One pane has focus at a time, drawn with a brighter border.
- `Tab` cycles forward and `Shift+Tab` backward. Neither moves a roster
  cursor.
- A mouse click anywhere inside a pane focuses it.
- Only the focused pane takes keyboard input. Background panes keep updating:
  turn output streams in and the "Working..." animation runs.
- `/abort`, `/abort <agent-id>`, `Ctrl+C` with empty input, and `Esc Esc`
  within about 500 ms target the focused pane. `/abort all` targets every
  pane. In a pane with no committed agent, these post "no agent selected —
  nothing to abort" to the pane's chat instead of dropping silently.
- `Ctrl+Q` quits multipane. There is no confirm dialog; typed prompts are
  saved to disk (see Persistence).
- `F1`, or `?` with empty chat input, toggles the help overlay.

## Roster mode

A pane with no committed agent shows the same tree as the Agent Ops Roster
tab:

```text
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

- The `Template:` and `Mission:` rows set the pane's swarm defaults. Click a
  word to set it. Each pane keeps its own choice, seeded from the global
  defaults at launch.
- Expansion follows the cursor. Only the backend group under the cursor is
  open, and it closes when the cursor leaves it. Cursors are per pane, so two
  panes can show different backends.
- Each agent under an open backend shows a `↳ Size` branch with one `[x]`
  checkbox per supported reasoning effort. The choice is per pane and is
  applied when the pane dispatches.

| Key | Action |
|---|---|
| `↑` / `k`, `↓` / `j` | Move the cursor through Backend, Agent, SizeBranch, and SizeLeaf rows. Template and Mission rows are click-only. |
| `→` / `l` | Expand: open a Backend group or an Agent's tree. |
| `←` / `h` | Collapse: close a Backend group or hide an Agent's Size leaves. |
| `PgUp` / `PgDn` | Move the cursor by a page (8 rows). |
| `g` / `G` | Jump to the first or last selectable row. |
| `Space` | Toggle the checkbox under the cursor on a SizeLeaf. |
| `Enter` | Backend: toggle expand. Agent: create `<base>#mp-pane-NN` and switch to chat mode. SizeBranch: toggle the tree. SizeLeaf: toggle the checkbox. |
| `Tab` / `Shift+Tab` | Cycle pane focus. Never moves the cursor. |
| Mouse left-click | Focus the pane and act on the row under the pointer, as `Enter` does. A Template or Mission word sets the pane default. |
| Mouse wheel | Scroll the roster. |
| `Ctrl+C`, `Esc Esc` | Post "no agent selected — nothing to abort". |

## Chat mode

Once a pane has an agent, its input works like the standard chat input. It has
the same editing keys and prompt history on `Up` / `Down`. The same commands
work too: `@swarm`, `@shadow`, `@new`, `@queue`, `@all`, and `/abort`. See the
"Multipane mode" section of `docs/KEYBINDINGS.md` for the full list.

| Key | Action |
|---|---|
| `Ctrl+R` | Return the pane to roster mode. Clears the selected agent, the chat input, and the active mission. The roster cursor and expansion state are kept. |
| `PgUp` / `PgDn` | Scroll the chat thread. Scrolling back to the bottom restores auto-stick. |
| Mouse wheel | Scroll the chat thread of the pane under the pointer. It does not move focus. |
| `Ctrl+\`, or the `NIT` / `TERM` title pill | Toggle the pane's embedded terminal. See `docs/TERMINAL.md`. |

## Persistence

nit writes `<state_dir>/multipane/session-<workspace-hash>.json` on `Ctrl+Q`
and on focus change, at most once per second. The next launch with the same
pane count reads it back. It restores the focused pane and, per pane, the cwd,
the chat input (capped at 4 KB), the prompt history, the Template and Mission
choice, and the selected agent. A selected agent that no longer exists drops
the pane back to roster mode. UI-only state (help overlay, dir search, roster
expansion) starts fresh.

A fresh `Ctrl+Q`, where no pane has run a mission and no prior file existed,
removes the file instead of saving an empty layout.

## Too-small terminals

When a pane would be narrower than 20 cells or shorter than 10 rows, nit does
not draw the grid. It shows one line instead:
"Terminal too small for N panes — resize or relaunch with --panes <smaller>".

## Performance budget

| Op | Target | Notes |
|---|---|---|
| Initial render of 16-pane grid | < 50 ms | Same render code per pane; one ratatui frame |
| Dir search keystroke → result update | < 16 ms | One frame budget; runs on async worker |
| Walk a 10k-entry directory tree | < 100 ms | Parallelised via rayon, cached after first walk |
| Focus switch (Tab) | < 1 ms | Pure state mutation, no IO |
| Pane redraw on background turn output | < 16 ms | Reuses existing render path |

The chat thread render is plain ratatui and fast. The dir walk dominates, so
nit caches it.

## State model

Multipane adds one optional field to `AppState`:
`multipane: Option<MultipaneState>`. When it is `Some`, nit renders the grid.
When it is `None`, nit runs as usual with no other change.

`crates/nit-core/src/state/multipane.rs` defines the types:

- `MultipaneState`: the `--backend` value, the backend filter, the pane list,
  the focused index, and the grid columns and rows.
- `PaneSession`: one pane. Its id, cwd, agent id, chat input and prompt
  history, mission ids, roster cursor and scroll, chat thread scroll,
  `selected_agent_id` (`None` means show the roster), `swarm_template`,
  `swarm_mission`, per-agent `selected_effort`, and the terminal flag and
  command.
- `DirSearchState`: the query, results, highlight, the base directory computed
  from the `../` prefix, a generation counter that drops stale walks,
  `show_hidden`, and the set of expanded directories.

Messages stay in `state.agents.messages` keyed by agent id. Active turns and
queues are keyed by agent id too. So the per-pane id suffix is all the
renderer and runner need. A pane's swarm mission is a normal swarm mission
with the pane's agent as planner. At dispatch time the pane's Template,
Mission, and effort choices are applied to the global settings the runner
reads.

## Key files

| Path | Contents |
|---|---|
| `crates/nit/src/cli/mod.rs` | `Command::Multipane`, `MultipaneArgs` |
| `crates/nit/src/multipane_setup.rs` | Validates `--backend`, builds the panes, installs terminal commands |
| `crates/nit-core/src/state/multipane.rs` | `PaneSession`, `DirSearchState`, `MultipaneState` |
| `crates/nit-tui/src/multipane/mod.rs`, `runtime/`, `grid.rs`, `focus.rs`, `roster_view.rs` | Render and event loop. `runtime/keys.rs` allow-lists the multipane keys and swallows the rest. |
| `crates/nit-tui/src/multipane/dispatch.rs` | Per-pane dispatch wrapper that injects the pane's cwd and agent id |
| `crates/nit-tui/src/multipane/dir_search.rs`, `dir_search_runner.rs` | Query parser and ranker; background walker |
| `crates/nit-tui/src/multipane/persistence.rs` | Session file read and write |
| `crates/nit-tui/src/tests/multipane_integration.rs` | Integration tests |

## Limitations

- The layout is fixed at launch. No splitting, closing, or dragging pane
  borders.
- One backend per pane per pick. Use `Ctrl+R` to pick again.
- No broadcast of one prompt to every pane. `/abort all` does cross panes.
- No confirm dialog on `Ctrl+Q`.
