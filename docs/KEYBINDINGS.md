# Keybindings

## Global
- Ctrl+Q: Quit (confirm if dirty)
- Ctrl+S: Save
- Ctrl+T: Toggle NITTree (file tree overlay)
- Ctrl+P: Fuzzy file search popup
- Ctrl+F: Content search popup
- Tab / Shift+Tab: Cycle pane focus
- Ctrl+1/2/3: Focus Editor / Job Output / Notes
- Ctrl+H/J/K/L: Focus panes (vim/tmux style: left/down/up/right)
- Ctrl+B: Toggle debug mode
- F1 / ?: Toggle help overlay
- Ctrl+Enter: Run Petri Dish simulation popup (active app)
- Ctrl+^: Show hidden Petri Dish
- : (Normal mode): Command prompt

## NITTree (Editor overlay)
- Esc / q: Close tree
- j/k or Up/Down: Move selection
- PageUp/PageDown: Page
- Home/End: Top/Bottom
- Enter: Toggle directory / open file (closes tree)
- r: Refresh
- .: Toggle hidden files
- i: Toggle ignored files

## Fuzzy Search popup
- Enter: Open selection (closes popup)
- Esc: Close popup
- Tab: Switch mode (FILES ↔ CONTENT)
- Up/Down: Move selection
- PageUp/PageDown: Page
- Home/End: Top/Bottom
- Backspace: Delete character
- Ctrl+Backspace: Delete word
- Mouse wheel: Scroll results / preview
- Mouse click: Select result
- Ctrl+U / Ctrl+D: Scroll preview up/down
- Ctrl+J / Ctrl+K: Scroll results down/up
- Ctrl+Y / Ctrl+E: Scroll preview line down/up
- F2 / Ctrl+.: Toggle hidden files
- F3 / Ctrl+G: Toggle ignored files
- F5 / Ctrl+R: Refresh (re-index / rerun)

## Editor (focused)
- Arrow keys / PageUp / PageDown / Home / End: Move cursor/scroll
- H/J/K/L (Normal mode): Move cursor
- I (Normal mode): Enter Insert mode
- a (Normal mode): Append + Insert mode
- v (Normal mode): Visual mode
- o (Normal mode): Open line below + Insert mode
- Shift+O (Normal mode): Open line above + Insert mode
- JJ (Insert mode): Save + switch to Normal
- Shift+S (Editor focus): Toggle syntax highlighting
- GG (Normal mode): Go to top
- Shift+G (Normal mode): Go to bottom
- e (Normal mode): Move to end of word
- b (Normal mode): Move to beginning of word
- y (Visual mode): Yank selection
- d (Visual mode): Delete selection
- p (Normal mode): Paste
- Shift+P (Normal mode): Paste above
- yy (Normal mode): Yank line
- $ (Normal mode): End of line
- % (Normal mode): Beginning of line
- u (Normal mode): Undo
- Shift+R (Normal mode): Redo
- dd (Normal mode): Delete line
- Enter: Newline
- Tab: Insert tab when in Insert mode (otherwise pane cycle)
- Backspace / Delete: Delete
- Esc: Switch to Normal mode

### Vim-style motions (Normal + Visual mode)
- w: Jump to the start of the next word (`alnum + _`, punctuation is a boundary)
- Shift+W: Jump to the start of the next WORD (whitespace-separated)
- Shift+B: Jump to the start of the previous WORD
- Shift+E: Jump to the end of the current/next WORD
- 0: Jump to the first column of the line
- ^: Jump to the first non-blank character of the line
- {: Jump to the previous blank-line paragraph boundary
- }: Jump to the next blank-line paragraph boundary
- Shift+H: Jump to the top of the visible viewport
- Shift+M: Jump to the middle of the visible viewport
- Shift+L: Jump to the bottom of the visible viewport

### Vim-style operators (Normal mode)
- x: Delete the character under the cursor
- Shift+X: Delete the character before the cursor (backspace)
- Shift+D: Delete from cursor to end of line
- Shift+C: Change from cursor to end of line (delete + Insert mode)
- s: Substitute character (delete char + Insert mode)
- Shift+J: Join the next line onto the current line with a space separator
- ~: Toggle the case of the character under the cursor (and advance)
- Shift+Y: Yank the current line (same as `yy`)

### Vim-style char search on the current line (Normal + Visual mode)
- f<char>: Jump forward to the next occurrence of `<char>` on the line
- Shift+F<char>: Jump backward to the previous occurrence of `<char>` on the line
- t<char>: Jump forward to one before the next `<char>` (till)
- Shift+T<char>: Jump backward to one after the previous `<char>` (till)
- ;: Repeat the last f / F / t / T in the same direction
- ,: Repeat the last f / F / t / T in the opposite direction

### Vim-style replace chord (Normal mode)
- r<char>: Replace the character under the cursor with `<char>` (does not enter Insert mode)

### Vim-style viewport / scroll (Normal + Visual mode)
- Ctrl+D: Scroll down half a page (cursor follows)
- Ctrl+U: Scroll up half a page (cursor follows)
- zz: Center the viewport on the cursor line
- zt: Scroll so the cursor line is at the top of the viewport
- zb: Scroll so the cursor line is at the bottom of the viewport

### Vim-style in-buffer search (Normal + Visual mode)
- *: Search forward for the whole word under the cursor; highlight every match
- \# (Shift+3): Search backward for the whole word under the cursor; highlight every match
- n: Jump to the next occurrence of the active search term (same direction as the last search)
- Shift+N: Jump to the next occurrence in the opposite direction
- /: Open the search prompt. Type a term and press Enter to jump to the next match; Esc cancels without applying.
- Repeated `*` / `#` on an occurrence of the word scans through all matches in that direction (equivalent to pressing `n` / `N`).

## Agent Ops
- Tab / Shift+Tab / Left/Right: Cycle Ops tabs (ROSTER / MISSIONS / DAG / ARTIFACTS / MCP / ALERTS / DIAG / SCRATCHPAD)
- j/k or Up/Down: Move selection
- Enter: Focus Agent Chat with selected context (except ARTIFACTS tab; see below)
- n: New mission (mock runner in MVP)
- Roster:
  - 1/2/3: Select swarm template (lab/parallel/bulk)
  - Space (on an agent row): Toggle priority (used as a planning hint for parallel/bulk)
  - l: Expand + enter the roster tree cursor (Size/Role)
  - h: Exit the roster tree cursor (then collapse on next h)
  - Mouse: Click the model name (left column) to expand; click again to collapse
  - Space/Enter (in the tree): Select the highlighted Size/Role option
- Artifacts (ARTIFACTS tab):
  - Enter or mouse click: Open selected artifact detail popup
  - Esc or q: Close artifact popup
  - j/k or Up/Down: Scroll popup content (when open)
- r / s / x: MCP reconnect / start / stop (MCP tab; default runtime for Codex, override with `--codex-runtime exec`)
  - Note: MCP reconnect preserves thread context; MCP stop clears it. If Codex reports “Session not found for thread_id …”, nit drops that agent’s saved thread id.
- Ctrl+Space / F6: Pause/resume active Petri/tournament runtime (global)

## Agent Chat
- Type message, Enter to send, Esc or Ctrl+C to clear input
  - Note: agent reply bodies are captured in Agent Ops → ARTIFACTS; the thread shows `done (see ARTIFACTS)` placeholders.
  - `@all <msg>`: broadcast (same prompt) to multiple agents (Codex and Claude)
  - `@swarm [all|N] [template=lab|parallel|bulk] [mission=general|research|computational-research] <msg>`: orchestrated multi-agent workflow (`t=` and `m=` are accepted as shorthands)
  - `@shadow <msg>`: single-agent dispatch with hidden propose/judge/review pipeline (also auto-enables for heavy prompts, see `docs/SHADOWS.md`)
  - `@new <msg>`: spawn a fresh-context clone when the agent is busy
  - `@queue` / `@q <msg>`: explicit queue (same as the implicit behaviour below)
  - Prompts sent while an agent is busy are automatically queued and dispatched when it becomes idle
- Left/Right/Home/End: Move input cursor
- Up/Down: Move input cursor between lines
- Ctrl+Up/Ctrl+Down: Scroll chat thread

## Visualizer (GoL)

### Title Bar Buttons (clickable)
- **APPLY**: Apply the best seed search proposal (swaps in the candidate's params)
- **SEED**: Cycle symmetry (none → mirror-x → mirror-y → rotate-180)
- **SNAP**: Snapshot current seed to `gol-snapshots/` (RLE + JSON metadata, deduped by grid hash)
- **SEARCH**: Toggle seed search (background worker mutates params and scores by component count vs density error)

### Keyboard Shortcuts
- Ctrl+E: Cycle seed encoder (ascii_bytes → hilbert_bits → lifehash16)
- Ctrl+S: Cycle symmetry (same as SEED button)
- Ctrl+V: Toggle view (GENOME ↔ PLATE)
- Ctrl+R: Cycle seed view (genome/plate/map/stats)
- Ctrl+M: Cycle plate render (solid/half/braille/tissue/heat)
- Ctrl+Y: Seed source (Editor only)
- Ctrl+A: Apply seed search proposal (same as APPLY button)
- Ctrl+G: Toggle seed search (same as SEARCH button)
- Ctrl+N: Snapshot seed (same as SNAP button)
- Ctrl+Shift+V: Cycle seed overlays
- Arrows / HJKL: Move genome inspector (Visualizer focus)
- Home / End: Inspector jump to edges
- 0 / $: Inspector jump to edges (fallback)
- G + digits + Enter: Jump to genome index
- C: Center inspector
- I: Toggle inspector

## Petri Dish (GoL popup)
- Esc: Close popup
- Space: Pause/resume
- Enter: Step one generation
- + / -: Speed up/down
- S: Snapshot sim state
- Ctrl+R: Reseed from current code
- H: Hide popup (sim keeps running)
- F2 / Ctrl+P: Rule picker
- P: Protocol picker
- T: Toggle wrap mode
- O: Cycle auto-stop policy (Off → Fixed → Repeat)
- G: Toggle rule search
- A: Apply best rule

## Petri Dish (Games popup)
- Esc: Close tournament
- Space: Pause/resume
- Enter: Step one round (when paused)
- + / -: Speed up/down
- H: Hide popup (tournament keeps running)

## Command/Prompts
- Y / N to confirm quit when prompted
- :run: Run the active app
- :q: Quit (confirm if dirty)
- :tree / :nittree / :explore: Toggle NITTree
- :find / :ff: Open fuzzy file search
- :grep / :rg / :search: Open content search
- :close: Close search popup (if open)
- Commands are routed to the active lab; use `--lab gol|games` at startup to switch labs.
- :gol hide / :petri hide: Hide GoL Petri Dish (sim keeps running)
- :gol show / :petri show: Show GoL Petri Dish
- :gol stop / :run stop (in GoL): Stop the GoL Petri Dish
- :gol rule: Show current rule + built-ins
- :gol rule <id|B/S>: Set rule by id or B/S string (e.g. `:gol rule conway`, `:gol rule B3/S23`)
- :gol rules: List available rules
- :gol seed / :seed view: Cycle seed view (GENOME → PLATE → MAP → STATS)
- :gol encoder / :seed encoder: Cycle to next seed encoder
- :gol encoder <name> / :seed encoder <name>: Switch to a named encoder (`ascii_bytes`, `hilbert_bits`, `lifehash16`, `structural`, `token_spectrum`, `ast_structure`, `complexity_field`)
- :games run: Run Games tournament
- :games run force <fsm|ca|tm> {params}: Force a family-scope run (e.g. `:games run force fsm {3,2}`)
- :games hide: Hide Games Petri Dish (tournament keeps running)
- :games show: Show Games Petri Dish
- :games stop: Stop the Games tournament
- :games status: Show tournament status
- :games export: Re-emit last run summary (if present)
- :games runs / :games browse / :games browser: Open run browser
- :games replay: Open match replay selector (uses loaded run summary)
- :games history / :games hist / :games plot: Open match history viewer
- :games strategy [run|all|config]: Open strategy inspector
- :games inspect <strategy_id>: Show introspection for a strategy (pretty text)
- :games inspect <strategy_id> {rule,states,symbols}: Inspect a TM rule tuple override
- :games inspect {rule,states,symbols}: Inspect a TM rule tuple (no config/run)
- :games tm [run|config] <input> [steps] [strategy_id]: TM simulator
- :games tm {rule,states,symbols} <input> [steps]: TM rule simulator
- :games ca [run|config] <input> [steps] [strategy_id]: CA simulator
- :games ca {n,k,r} <input> [steps]: CA rule tuple simulator
- :games analyze [path] [tail=N] [samples=N]: Analyze history log
