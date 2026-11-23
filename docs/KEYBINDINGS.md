# Editor and station controls

Open a file with `nit path/to/file`, or run `nit .` to browse a directory. In the editor, press **i** to type, **Esc** to return to Normal mode, and **Ctrl+S** to save. Press **F1** for help. Letter case matters in the tables below. A sequence such as `gg` means two lowercase keypresses.

## Find and open a file

| Task | Control |
|---|---|
| Browse the file tree (NITTree) | `Ctrl+T` or `:tree`. Select a row with a click, the wheel, Up/Down, or `k`/`j`. Press `Enter` to expand a directory or open a file. `q` or `Esc` closes the tree. |
| Create, rename, or refresh in the tree | `n` creates a file, `N` creates a directory, `r` renames, and `R` refreshes. Use `.` for hidden files and `i` for ignored files. |
| Find by name | `Ctrl+P` or `:find`. Type part of the filename. |
| Find text across files | `Ctrl+F` or `:grep`. Type the text to find. Open a result with `Enter` to visit its location. |
| Open a known path | In Normal mode, type `:e path/to/file` and press `Enter`. Relative paths start at the workspace directory. |
| Go back to a location | `Ctrl+O` jumps back and `Ctrl+I` jumps forward in Normal mode when the terminal distinguishes it from Tab. |

In the search popup, `Tab` switches between filenames and contents. Up/Down or `Ctrl+K`/`Ctrl+J` selects a result. `PageUp`/`PageDown` moves by a page and `Home`/`End` jumps to the ends. Click to select, press `Enter` to open, or `Esc` to close.

| Search options | Control |
|---|---|
| Scroll the preview | `Ctrl+U`/`Ctrl+D` by a page, `Ctrl+Y`/`Ctrl+E` up/down by one line, or wheel over the preview. |
| Change the query | `Backspace` deletes a character, `Ctrl+Backspace` a word. |
| Include hidden or ignored files | `F2` or `Ctrl+.` toggles hidden files. `F3` or `Ctrl+G` toggles ignored files. |
| Refresh results | `F5` or `Ctrl+R`. |

## Edit and save

The center pane shows the open file's path. A `●` beside it means the buffer has unsaved edits. The gutter marks additions with `+`, modifications with `~`, and a deletion above a line with `▔`. Saving clears the dot and resets the gutter. Use `:changes` to review the saved patch against Git.

When reading unfamiliar code in the default theme, look for gold function and method names, blue types, lavender keywords, green strings and peach numbers. The current line has a shaded background. A Markdown code fence with a recognized language tag uses that language’s colors.

| Task | Keys |
|---|---|
| Start typing | From Normal mode, `i` inserts at the cursor, `a` appends, and `o`/`O` opens a line below/above. `Enter` keeps indentation and `Tab` indents while typing. |
| Save | `Ctrl+S` or `:w`. Typing `jj` quickly in Insert mode saves and returns to Normal mode. |
| Select, copy, cut, paste | `Ctrl+A/C/X/V` or `Cmd+A/C/X/V` in the editor or Scratchpad. Pasting replaces a selection. |
| Edit by word | Hold Ctrl or Alt with Left/Right to move, or with Backspace/Delete to erase a word. |
| Select a range | `v` enters Visual mode. Move to extend the selection, then `y` to copy, `d` to delete, `U`/`u` to change case, or `>`/`<` to indent/dedent. |
| Copy or paste whole lines | In Normal mode, `yy` or `Y` copies the line. `p` pastes, `P` pastes a line above. |
| Delete or change | `x`/`X` deletes at/before the cursor, `dd` deletes a line, `D` deletes to line end. `cw` changes a word, `C` changes to line end, and `s` replaces the current character in Insert mode. |
| Make a small correction | `r` followed by a character replaces one character. `~` toggles case, and `J` joins the following line. |
| Undo and redo | `u` / `R` in Normal mode. `Ctrl+Z` or `Cmd+Z` also undoes. Add Shift to redo, or use `Ctrl+Y`. |
| Toggle syntax colours | `S` in the focused editor outside Insert mode. |

### Navigate within a file

Use the arrows or `h j k l` to move left, down, up, and right in Normal or Visual mode. Prefix a motion with a count, for example `5j`.

| Destination | Keys |
|---|---|
| Line start, first nonblank, line end | `0`, `^`, `$`. `Home` and `End` also move within the line. |
| Word start, word end, previous word | `w`, `e`, `b`. Uppercase `W E B` uses whitespace boundaries. |
| Previous or next paragraph boundary | `{` / `}`. |
| File start, file end, numbered line | `gg`, `G`, or `:56` to visit line 56. |
| Matching bracket or symbol definition | `%` for the bracket, `gd` to look up a definition. |
| Top, middle, or bottom visible line | `H`, `M`, `L`. `Ctrl+U`/`Ctrl+D` scrolls half a page and moves the cursor. |
| Place the cursor line at top, centre, bottom | `zt`, `zz`, `zb`. |
| A character on this line | `f`/`F` then the character searches forward/back. `t`/`T` stops just before/after it. `;` repeats and `,` reverses. |
| Text in this buffer | Type `/` and the search term, then `Enter`. `Esc` cancels. `*`/`#` searches forward/back for the whole word at the cursor. `n` repeats the search and `N` reverses it. |

## Review changes, switch panes, or quit

Run `:changes` to focus Git review. With a clean working tree, the pane shows up to 24 recent commits. The ahead and behind counts compare your branch with its locally recorded upstream. Use ↑/↓, `j`/`k` or a click to select a recent commit and preview its patch. Press Enter or → to expand the patch, PageUp/PageDown to scroll and Esc or ← to return to the list.

When the working tree has changes, each file row shows two Git status characters. The first describes the staged copy, the second the working copy. `MM` means a file has both staged and unstaged edits, `??` marks an untracked file, and `UU` marks a conflict where both sides modified the file. Select with `j`/`k`, Up/Down, or a click. Use `PageUp`/`PageDown` to read the patch and `Enter` to open the selected path. The pane refreshes automatically. Press `r` to refresh immediately.

| Task | Control |
|---|---|
| Move focus | Click a pane or cycle with `Tab` / `Shift+Tab` outside a local input or tab selector. Directional shortcuts are `Ctrl+H/J/K/L`. `Ctrl+1/2/3` selects Editor, Agent Ops, or Agent Chat on terminals that deliver those chords. |
| Open a shell | `F4` opens the terminal popup. See [Terminal](TERMINAL.md) for toggling a pane, copying, and scrollback. |
| View help or diagnostics | `F1`, `?` in Normal mode, or `:help` for shortcuts. `Ctrl+B` toggles debug information. |
| Close a file | After a directory launch, `:q` closes the current file, then returns to another open file or the tree. `:wq` saves first. |
| Quit the station | `Ctrl+Q`, or `:q` after a file launch. Confirm with `Y` to discard unsaved changes or `N` to stay. `:wq` saves the active file and checks for other unsaved buffers. |

## Read a built-in reference

Opening a file or directory with `nit` shows **NITSKILLS**. Focus starts in the editor or file tree. Type `:skills` to focus the catalog. Choose a reference with Up/Down, `k`/`j`, a click, or the wheel over the list. Press `/` to filter by title, skill ID, group, or description. `Enter` keeps the results, and `Esc` clears the filter. The selected preview appears beside or below the list when space permits. Press `Enter` or click **Open** to read it in the full pane. Scroll the document with the wheel or, in the full reader, the arrows and `PageUp`/`PageDown`. Press `Esc`, Left, or Backspace, or click **Back**, to return to the catalog. Press `a` to attach the selected reference to an unsent Agent Chat draft. The command at the top of each document prints the reference in your shell.

## Work with agents

In **Agent Ops**, use Left/Right to change tabs and Up/Down or `k`/`j` to select a row. The Codex group starts expanded on a standard launch so you can select a model immediately. In **Roster**, expand a model with `l`, collapse with `h`, and choose a reasoning effort or role with `Space` or `Enter`. On a model row, `Space` toggles priority. Keys `1–3` select the lab, parallel, and bulk swarm templates. Keys `4–7` select auto, general, research, and computational research. Press `Enter` on a model to focus its chat.

- **Send and revisit prompts.** `Enter` sends and `Shift+Enter` starts another line. Up/Down moves through multiline input, then through prompt history at its ends. `Ctrl+Up`/`Ctrl+Down` scrolls the conversation. A new prompt queues while the agent is busy.
- **Choose how a prompt runs.** `@new <prompt>` starts a fresh conversation, `@queue <prompt>` queues explicitly, and `@all <prompt>` sends to several agents. For delegated tasks and review, see [Swarm](SWARM.md) and [Shadow](SHADOWS.md).
- **Read a result.** In **Artifacts**, click or press `Enter` to open a result. Scroll with Up/Down or `j`/`k`. Close with `Esc` or `q`.
- **Cancel work.** In Agent Chat, `/abort`, two quick presses of `Esc`, or `Ctrl+C` with empty input cancels current work. `/abort AGENT-ID` targets one agent. In the standard station, `/abort all` cancels all running swarms and queued work. The **Missions** tab also accepts `x` on an active mission.
- **Reconnect Codex.** In the **MCP** tab, `r` reconnects while retaining saved thread context, `s` starts the server, and `x` stops it and clears that context.

Press `F3` to inspect signals, file claims, and assumptions. Use `Tab` to switch tables, the wheel to scroll, and `Esc` to close. The commands `:signals`, `:claims`, and `:assumptions` open specific tables. See [Coordination records](SUBSTRATE.md).

For separate conversations in a grid, use [Multipane](MULTIPANE.md). For an enabled multiway search, `Ctrl+Shift+M` opens its graph. See [Multiway search](MULTIWAY.md).

## Run a simulation

Use `:gol` to switch the Changes pane to the Game of Life visualizer, or launch with `nit gol`. In either lab, `Ctrl+Enter` runs the simulation or tournament, and `F6` pauses or resumes it. The seed used in the GoL lab is a grid derived from source text. The chosen rule determines how its cells evolve. Use `:changes` to return to Git.

| In the GoL visualizer | Control |
|---|---|
| Change the encoding | `Ctrl+E` cycles encoders. `:gol encoder NAME` selects one directly. See [Seed encodings](SEEDS.md). |
| Inspect the seed | `Ctrl+V` toggles the grid view, `Ctrl+R` cycles views, `Ctrl+M` changes rendering, and `Ctrl+Shift+V` cycles overlays. Use `i` to toggle the inspector, arrows to move, Home/End for the edges, and `c` to centre. |
| Adjust or record a seed | **SEED** or `Ctrl+S` cycles symmetry. **SEARCH** or `Ctrl+G` toggles the search for seed settings. **APPLY** or `Ctrl+A` applies a proposal. **SNAP** or `Ctrl+N` saves the seed and its metadata. |
| Run or stop | `:gol run` starts the Petri Dish. `:gol hide` and `:gol show` change visibility while it runs. `:gol stop` stops it. |
| Choose a rule | `:gol rules` lists rules. `:gol rule conway` selects Conway's rule. `:gol rule B3/S23` specifies the same birth and survival counts directly. |

In the GoL **Petri Dish**, `Space` pauses or resumes, `Enter` steps one generation while paused, `+`/`-` changes speed, and `s` saves a snapshot. `Ctrl+R` reseeds from the editor. `F2` or `Ctrl+P` opens the rule picker, `p` opens protocols, `t` toggles wrapping, and `o` cycles automatic stopping. `g` searches for rules and `a` applies the best proposal. `h` hides the simulation and `Esc` closes it.

Launch `nit games` for strategy tournaments. Use `:games run`, `:games status`, and `:games stop`. In the tournament, `Space` pauses, `Enter` steps one round while paused, `+`/`-` adjusts speed, and `Tab` switches to the inspector. Use `:games runs` to browse results, `:games replay` to replay a match, and `:games history` to inspect its history. For strategy inspection, simulation, export, and analysis, see [Games](GAMES.md).

| Explore a Games result | Command |
|---|---|
| Hide, resume viewing, or export a tournament | `:games hide`, `:games show`, `:games export`. |
| Browse strategies from a run or configuration | `:games strategy run`, `:games strategy config`, or `:games strategy all`. |
| Read a strategy's definition | `:games inspect <strategy_id>`. For a Turing machine, supply `{rule,states,symbols}` to inspect those parameters directly. |
| Try an input | `:games tm [run\|config] <input> [steps] [strategy_id]` or `:games ca [run\|config] <input> [steps] [strategy_id]`. |
| Analyze a history log | `:games analyze [path] [tail=N] [samples=N]`. |
