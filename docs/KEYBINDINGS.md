# Keybindings

The full list of nit's key bindings and `:` commands, grouped by the pane or
popup that owns them. It is a reference, not a tutorial. For the features
themselves see `docs/MULTIPANE.md`, `docs/TERMINAL.md`, `docs/SUBSTRATE.md`,
`docs/MULTIWAY.md`, `docs/SWARM.md`, and `docs/SHADOWS.md`.

## Global

- Ctrl+Q: Quit. Asks first if a buffer is dirty.
- Ctrl+S: Save.
- Ctrl+T: Toggle NITTree, the file tree overlay.
- Ctrl+P: Open the fuzzy file search popup.
- Ctrl+F: Open the content search popup.
- Tab / Shift+Tab: Cycle pane focus.
- Ctrl+1 / Ctrl+2 / Ctrl+3: Focus Editor / Job Output / Notes.
- Ctrl+H / Ctrl+J / Ctrl+K / Ctrl+L: Focus the pane to the left / below / above / right.
- Ctrl+B: Toggle debug mode.
- F1 / ?: Toggle the help overlay.
- Ctrl+Enter: Run the Petri Dish popup for the active app.
- Ctrl+^ / Ctrl+6: Show a hidden Petri Dish.
- : (Normal mode): Open the command prompt.

## NITTree (Editor overlay)

- Esc / q: Close the tree.
- j / k or Up / Down: Move the selection.
- PageUp / PageDown: Move by a page.
- Home / End: Jump to the top / bottom.
- Enter: Toggle a directory, or open a file and close the tree.
- r: Refresh.
- .: Toggle hidden files.
- i: Toggle ignored files.

## Fuzzy Search popup

- Enter: Open the selection and close the popup.
- Esc: Close the popup.
- Tab: Switch between FILES and CONTENT mode.
- Up / Down: Move the selection.
- PageUp / PageDown: Move by a page.
- Home / End: Jump to the top / bottom.
- Backspace: Delete a character.
- Ctrl+Backspace: Delete a word.
- Mouse wheel: Scroll the results or the preview.
- Mouse click: Select a result.
- Ctrl+U / Ctrl+D: Scroll the preview up / down.
- Ctrl+J / Ctrl+K: Scroll the results down / up.
- Ctrl+Y / Ctrl+E: Scroll the preview one line down / up.
- F2 / Ctrl+.: Toggle hidden files.
- F3 / Ctrl+G: Toggle ignored files.
- F5 / Ctrl+R: Refresh (re-index or rerun the search).

## Editor (focused)

The editor is modal like vim: Normal, Insert, and Visual. Each binding says
which mode it needs.

- Arrow keys / PageUp / PageDown / Home / End: Move the cursor or scroll.
- H/J/K/L (Normal mode): Move the cursor.
- I (Normal mode): Enter Insert mode.
- a (Normal mode): Append, then Insert mode.
- v (Normal mode): Enter Visual mode.
- o (Normal mode): Open a line below, then Insert mode.
- Shift+O (Normal mode): Open a line above, then Insert mode.
- JJ (Insert mode): Save and switch to Normal mode.
- Shift+S (Editor focus): Toggle syntax highlighting.
- GG (Normal mode): Go to the top.
- Shift+G (Normal mode): Go to the bottom.
- e (Normal mode): Move to the end of the word.
- b (Normal mode): Move to the start of the word.
- y (Visual mode): Yank the selection.
- d (Visual mode): Delete the selection.
- p (Normal mode): Paste.
- Shift+P (Normal mode): Paste above.
- yy (Normal mode): Yank the line.
- $ (Normal mode): End of line.
- % (Normal mode): Start of line.
- u / Ctrl+Z / Cmd+Z: Undo.
- Shift+R / Ctrl+Y / Ctrl+Shift+Z / Cmd+Shift+Z: Redo.
- dd (Normal mode): Delete the line.
- Enter: Newline. Keeps the indentation.
- Tab: Insert a tab in Insert mode. Otherwise cycle panes.
- Backspace / Delete: Delete.
- Esc: Switch to Normal mode.
- Ctrl+A / Cmd+A: Select all (also in the Scratchpad).
- Ctrl+C / Cmd+C: Copy the selection (also in the Scratchpad).
- Ctrl+X / Cmd+X: Cut the selection (also in the Scratchpad).
- Ctrl+V / Cmd+V: Paste, replacing the selection (also in the Scratchpad).
- Ctrl+Left / Ctrl+Right (or Alt): Move by word (also in the Scratchpad).
- Ctrl+Backspace / Ctrl+Delete (or Alt): Delete a word (also in the Scratchpad).

### Vim-style motions (Normal and Visual mode)

- w: Jump to the start of the next word. A word is letters, digits, and `_`; punctuation is a boundary.
- Shift+W: Jump to the start of the next WORD (whitespace-separated).
- Shift+B: Jump to the start of the previous WORD.
- Shift+E: Jump to the end of the current or next WORD.
- 0: Jump to the first column of the line.
- ^: Jump to the first non-blank character of the line.
- {: Jump to the previous blank-line paragraph boundary.
- }: Jump to the next blank-line paragraph boundary.
- Shift+H: Jump to the top of the visible viewport.
- Shift+M: Jump to the middle of the visible viewport.
- Shift+L: Jump to the bottom of the visible viewport.
- `<N> + motion`: Repeat a motion N times (`5j`, `3w`, `56G`).

### Vim-style operators (Normal mode)

- x: Delete the character under the cursor.
- Shift+X: Delete the character before the cursor.
- Shift+D: Delete from the cursor to the end of the line.
- Shift+C: Change from the cursor to the end of the line (delete, then Insert mode).
- s: Substitute the character (delete it, then Insert mode).
- Shift+J: Join the next line onto the current line with a space.
- ~: Toggle the case of the character under the cursor and advance.
- Shift+Y: Yank the current line (same as `yy`).

### Vim-style char search on the current line (Normal and Visual mode)

- `f<char>`: Jump forward to the next `<char>`.
- `Shift+F<char>`: Jump backward to the previous `<char>`.
- `t<char>`: Jump forward to one before the next `<char>`.
- `Shift+T<char>`: Jump backward to one after the previous `<char>`.
- ;: Repeat the last f / F / t / T in the same direction.
- ,: Repeat the last f / F / t / T in the opposite direction.

### Vim-style replace (Normal mode)

- `r<char>`: Replace the character under the cursor with `<char>`. Stays in Normal mode.

### Vim-style viewport and scroll (Normal and Visual mode)

- Ctrl+D: Scroll down half a page. The cursor follows.
- Ctrl+U: Scroll up half a page. The cursor follows.
- zz: Center the viewport on the cursor line.
- zt: Put the cursor line at the top of the viewport.
- zb: Put the cursor line at the bottom of the viewport.

### Vim-style in-buffer search (Normal and Visual mode)

- *: Search forward for the whole word under the cursor and highlight every match.
- \# (Shift+3): Search backward for the whole word under the cursor and highlight every match.
- n: Jump to the next match in the direction of the last search.
- Shift+N: Jump to the next match in the opposite direction.
- /: Open the search prompt. Type a term and press Enter to jump to the next match. Esc cancels.
- Pressing `*` or `#` again on a match scans through all matches in that direction, like `n` / `N`.

## Agent Ops

- Tab / Shift+Tab / Left / Right: Cycle the Ops tabs (ROSTER, MISSIONS, DAG, ARTIFACTS, MCP, ALERTS, DIAG, SCRATCHPAD).
- j / k or Up / Down: Move the selection.
- Enter: Focus Agent Chat with the selected context. On the ARTIFACTS tab it opens the artifact instead.
- n: New mission (mock runner).
- Ctrl+Space / F6: Pause or resume the active Petri or tournament runtime. Works from any pane.

ROSTER tab:

- 1 / 2 / 3: Select the swarm template (lab / parallel / bulk).
- Space (on an agent row): Toggle priority. Parallel and bulk planning use it as a hint.
- l: Expand the row and enter the roster tree cursor (Size / Role).
- h: Leave the roster tree cursor. Press again to collapse the row.
- Mouse: Click the model name in the left column to expand. Click again to collapse.
- Space / Enter (in the tree): Select the highlighted Size or Role option.

ARTIFACTS tab:

- Enter or mouse click: Open the selected artifact in a detail popup.
- Esc / q: Close the artifact popup.
- j / k or Up / Down: Scroll the popup content.

MCP tab (the default runtime for Codex; override with `--codex-runtime exec`):

- r / s / x: Reconnect / start / stop the MCP server.
  - Reconnect keeps the saved thread context. Stop clears it. If Codex reports "Session not found for thread_id ...", nit drops that agent's saved thread id.

MISSIONS tab:

- x: Abort the highlighted mission. Kills its in-flight and queued turns and leaves other missions running. Does nothing if the mission is already finished.

## Agent Chat

- Enter: Send the message.
- Esc (with text in the input): Clear the input.
- Ctrl+C (with text in the input): Clear the input, or copy the selection.
- Ctrl+C (with empty input): Abort the active swarm mission.
- Esc Esc (two presses within about 500 ms): Abort the active swarm mission. A single Esc only clears the selection.
- Left / Right / Home / End: Move the input cursor.
- Up / Down: Move the input cursor between lines.
- Ctrl+Up / Ctrl+Down: Scroll the chat thread.

Agent replies land in Agent Ops, ARTIFACTS tab. The thread shows a
`done (see ARTIFACTS)` placeholder. Prompts sent while an agent is busy queue
up and run when it is idle.

Chat commands:

- `@all <msg>`: Broadcast the same prompt to several agents (Codex and Claude).
- `@swarm [all|N] [template=lab|parallel|bulk] [mission=general|research|computational-research] <msg>`: Start an orchestrated multi-agent workflow. `t=` and `m=` are accepted shorthands. See `docs/SWARM.md`.
- `@shadow <msg>`: Single-agent dispatch with a hidden propose / judge / review pipeline. Heavy prompts turn it on by themselves. See `docs/SHADOWS.md`.
- `@new <msg>`: Spawn a fresh-context clone when the agent is busy.
- `@queue <msg>` / `@q <msg>`: Queue behind the agent's current turn. Same as the implicit queueing.
- `/abort` or `@abort`: Cancel the active swarm mission. `/abort all` cancels every running swarm. `/abort <agent-id>` cancels one agent. See `docs/SWARM.md` "Aborting a swarm".

## Substrate overlay

A popup for the living-system substrate: signals, claims, and assumptions.
`docs/SUBSTRATE.md` explains each table.

- F3: Open or close the overlay.
- `:substrate` / `:sub` / `:sig` / `:signals`: Open on the Signals tab.
- `:claims`: Open on the Claims tab.
- `:assumptions` / `:asm`: Open on the Assumptions tab.
- Tab: Cycle the sub-tabs (Signals, Claims, Assumptions). Clicking a tab label also cycles. Clicking the active tab closes the overlay.
- Mouse wheel: Scroll the active table. The scroll position is shared across sub-tabs.
- Esc or F3: Close.

## Multiway graph view (`NIT_MULTIWAY`)

A live popup of the whole multiway search DAG. The key and the popup only work
when `NIT_MULTIWAY=1`. See `docs/MULTIWAY.md`.

- Ctrl+Shift+M: Toggle the live multiway popup. It also opens by itself when a search starts. The `@multiway-popup` chat command toggles it too.
- j / k or Up / Down: Scroll the search tree.
- PageUp / PageDown: Page the tree.
- Home: Scroll to the top.
- Esc or q: Close the popup.
- `@multiway-graph` (chat command, not a key): Render the current DAG to a Graphviz image and open it in the OS viewer. Without Graphviz, nit saves the `.dot` file and shows its path.

## Terminal (embedded shell)

An OS shell inside nit: a tab in Agent Chat, a modal popup, or one per
multipane pane. See `docs/TERMINAL.md`.

- `Ctrl+\`: Toggle the Agent Chat terminal tab. In multipane mode it toggles the focused pane's terminal.
- `Ctrl+Shift+T`: Open or close the modal terminal popup.
- `Esc Esc`: Close the popup. The first Esc goes to the shell; the second closes it.
- Mouse drag (left button): Select a rectangle of terminal text. Releasing copies it to the system clipboard.
- Mouse wheel: Scroll the 10,000-line scrollback. Typing snaps back to the live bottom.

## Visualizer (GoL)

### Title bar buttons (clickable)

- APPLY: Apply the best seed-search proposal. Swaps in the candidate's params.
- SEED: Cycle symmetry (none, mirror-x, mirror-y, rotate-180).
- SNAP: Snapshot the current seed to `gol-snapshots/` as RLE plus JSON metadata, deduped by grid hash.
- SEARCH: Toggle seed search. A background worker mutates params and scores by component count against density error.

### Keyboard shortcuts

- Ctrl+E: Cycle the seed encoder, in this order: ascii_bytes, hilbert_bits, lifehash16, structural, token_spectrum, ast_structure, complexity_field. See `docs/SEEDS.md`.
- Ctrl+S: Cycle symmetry (same as SEED).
- Ctrl+V: Toggle the view (GENOME / PLATE).
- Ctrl+R: Cycle the seed view (genome / plate / map / stats).
- Ctrl+M: Cycle the plate render (solid / half / braille / tissue / heat).
- Ctrl+Y: Seed source (Editor only).
- Ctrl+A: Apply the seed-search proposal (same as APPLY).
- Ctrl+G: Toggle seed search (same as SEARCH).
- Ctrl+N: Snapshot the seed (same as SNAP).
- Ctrl+Shift+V: Cycle seed overlays.
- Arrows / HJKL: Move the genome inspector (Visualizer focus).
- Home / End: Jump the inspector to the edges.
- 0 / $: Jump the inspector to the edges (fallback).
- G, digits, Enter: Jump to a genome index.
- C: Center the inspector.
- I: Toggle the inspector.

## Petri Dish (GoL popup)

- Esc: Close the popup.
- Space: Pause or resume.
- Enter: Step one generation.
- + / -: Speed up or slow down.
- S: Snapshot the sim state.
- Ctrl+R: Reseed from the current code.
- H: Hide the popup. The sim keeps running.
- F2 / Ctrl+P: Open the rule picker.
- P: Open the protocol picker.
- T: Toggle wrap mode.
- O: Cycle the auto-stop policy (Off, Fixed, Repeat).
- G: Toggle rule search.
- A: Apply the best rule.

## Petri Dish (Games popup)

- Esc: Close the tournament.
- Space: Pause or resume.
- Enter: Step one round (when paused).
- + / -: Speed up or slow down.
- Tab: Toggle between the tournament and the inspector.
- Left / Right: Adjust the inspector window.
- H: Hide the popup. The tournament keeps running.

## Command prompt (`:` commands)

Press `:` in Normal mode. Commands go to the active lab; start nit with
`--lab gol|games` to switch labs. Answer `Y` or `N` when asked to confirm a
quit.

General:

- `:q` / `:quit` / `:exit`: Quit. Asks first if a buffer is dirty.
- `:w` / `:write`: Save the current file.
- `:wq` / `:x`: Save and quit (file launch), or save and switch to the last buffer or NITTree (directory launch).
- `:e <path>` / `:edit <path>`: Open a file. The path is workspace-relative.
- `:<N>`: Jump to line N. For example `:56`.
- `:help` / `:commands`: Open the help overlay.
- `:run`: Run the active app.
- `:tree` / `:nittree` / `:explore`: Toggle NITTree.
- `:find` / `:ff`: Open the fuzzy file search.
- `:grep` / `:rg` / `:search`: Open the content search.
- `:close`: Close the search popup if one is open.

GoL lab:

- `:gol run` / `:gol start` / `:life run`: Run the GoL Petri Dish.
- `:gol hide` / `:petri hide`: Hide the GoL Petri Dish. The sim keeps running.
- `:gol show` / `:petri show`: Show the GoL Petri Dish.
- `:gol stop` / `:run stop` (in GoL): Stop the GoL Petri Dish.
- `:gol rule`: Show the current rule and the built-ins.
- `:gol rule <id|B/S>`: Set the rule by id or B/S string. For example `:gol rule conway` or `:gol rule B3/S23`.
- `:gol rules`: List the available rules.
- `:gol seed` / `:seed view`: Cycle the seed view (GENOME, PLATE, MAP, STATS).
- `:gol encoder` / `:seed encoder`: Cycle to the next seed encoder.
- `:gol encoder <name>` / `:seed encoder <name>`: Switch to a named encoder: `ascii_bytes`, `hilbert_bits`, `lifehash16`, `structural`, `token_spectrum`, `ast_structure`, `complexity_field`.

Games lab:

- `:games run`: Run a Games tournament.
- `:games run force <fsm|ca|tm> {params}`: Force a family-scope run. For example `:games run force fsm {3,2}`.
- `:games hide`: Hide the Games Petri Dish. The tournament keeps running.
- `:games show`: Show the Games Petri Dish.
- `:games stop`: Stop the Games tournament.
- `:games status`: Show the tournament status.
- `:games export`: Re-emit the last run summary, if there is one.
- `:games runs` / `:games browse` / `:games browser`: Open the run browser.
- `:games replay`: Open the match replay selector. Uses the loaded run summary.
- `:games history` / `:games hist` / `:games plot` / `:games plots`: Open the match history viewer. In the Games lab `:history`, `:hist`, `:plot`, and `:plots` work too.
- `:games strategy [run|all|config]` (alias `:games strategies`): Open the strategy inspector.
- `:games inspect <strategy_id>`: Show introspection for a strategy as pretty text.
- `:games inspect <strategy_id> {rule,states,symbols}`: Inspect a TM rule tuple override.
- `:games inspect {rule,states,symbols}`: Inspect a TM rule tuple with no config or run.
- `:games tm [run|config] <input> [steps] [strategy_id]`: TM simulator.
- `:games tm {rule,states,symbols} <input> [steps]`: TM rule simulator.
- `:games ca [run|config] <input> [steps] [strategy_id]`: CA simulator.
- `:games ca {n,k,r} <input> [steps]`: CA rule tuple simulator.
- `:games analyze [path] [tail=N] [samples=N]`: Analyze a history log. See `docs/GAMES.md`.

## Multipane mode

Start it with `nit multipane [--backend <model>] [--panes N] [--cwd PATH] [--terminal-command CMD ...]`.
See `docs/MULTIPANE.md`. Editor, file tree, visualizer, and Agent Ops keys do
nothing here; use plain `nit` for those.

- Tab / Shift+Tab: Focus the next / previous pane. Wraps around.
- Mouse click in a pane: Focus that pane.
- Ctrl+Q: Quit without a confirm. Per-pane state persists to `<state_dir>/multipane/session-<hash>.json`.
- F1 or ? (chat empty): Toggle the multipane help overlay.
- Enter (in the focused pane's chat input): Send the prompt. The agent runs in the pane's cwd.
- Backspace / Left / Right / Home / End / printable keys: Edit the focused pane's chat input.
- Ctrl+A, Ctrl+C / Ctrl+V / Ctrl+X, Shift+Enter, Ctrl+Backspace, Ctrl+Left / Ctrl+Right: Select all, clipboard, newline with indent, delete word, move by word. Same as the standard chat input.
- Up / Down: Walk the focused pane's prompt history. Up keeps the current draft; Down past the newest entry restores it.
- PgUp / PgDn: Scroll the focused pane's chat thread.
- Ctrl+C (with empty chat input): Cancel the focused pane.
- Esc Esc (within about 500 ms): Cancel the focused pane.
- Ctrl+R: Return the focused pane to the roster picker.
- Ctrl+/ or F2: Toggle the per-pane dir-search overlay. Some terminals, including the default macOS Terminal.app, drop Ctrl+/; use F2 there.

Chat commands in a pane:

- `@swarm [all|N] [template=lab|parallel|bulk] [mission=...] <prompt>`: Start a per-pane swarm mission. The pane's agent is the planner.
- `@shadow <prompt>`: Single-agent dispatch with a hidden propose / judge / review pipeline.
- `@new <prompt>`: Spawn a fresh-context chat clone of the pane's base agent.
- `@queue <prompt>` / `@q <prompt>`: Queue behind the pane's current turn. Same as the default queueing.
- `@all <prompt>`: Broadcast to every dispatchable agent in the focused pane's mission.
- `/abort` or `@abort`: Cancel the focused pane's mission. The run moves to `completed_runs` and nit posts a SYSTEM_ALERT.
- `/abort all`: Cancel every active swarm and clear the runner queues.
- `/abort <agent-id>`: Cancel one agent's in-flight and queued turns.

Roster picker (after Ctrl+R):

- Up / Down / k / j, PgUp / PgDn, g / G: Move the cursor.
- h / l (or Left / Right): Fold or unfold a subtree.
- Space: Toggle a Size leaf.
- Enter: Commit the selection.

Dir search (Ctrl+/ or F2):

- Up / Down: Move the selection in the results dropdown. It shows 3 to 16 rows, sized to the pane.
- Enter: Set the focused pane's cwd to the chosen directory and post a `cwd → /new/path` system alert.
- Esc: Close the overlay and keep the chat input. A second Esc within about 500 ms still cancels the focused pane.
- Alt+f: Toggle hidden directories such as `.git` and `.cache` for this overlay.
- Tab / Shift+Tab: Switch pane focus and close the overlay. Reopen it in the new pane with Ctrl+/ or F2.
