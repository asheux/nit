# Keybindings

## Global
- Ctrl+Q: Quit (confirm if dirty)
- Ctrl+S: Save
- Ctrl+T: Toggle NITTree (file tree overlay)
- Ctrl+P: Fuzzy file search popup
- Ctrl+F: Content search popup
- Tab / Shift+Tab: Cycle pane focus
- Ctrl+1/2/3: Focus Editor / Agent Ops / Agent Chat
- Ctrl+H/J/K/L: Focus panes (vim/tmux style: left/down/up/right)
- F1 / ?: Toggle help overlay
- Ctrl+Enter: Run Petri Dish simulation popup (active app)
- Ctrl+^: Show hidden Petri Dish
- : (Normal mode): Command prompt

## NITTree (Editor overlay)
- Esc / q: Close tree
- j/k or Up/Down: Move selection
- PageUp/PageDown: Page
- Home/End: Top/Bottom
- Enter: Open file (closes tree)
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

## Agent Ops
- Tab / Shift+Tab / Left/Right: Cycle Ops tabs (Roster/Missions/MCP/Alerts/Diagnostics/Scratchpad)
- j/k or Up/Down: Move selection
- Enter: Focus Agent Chat with selected context
- n: New mission (mock runner in MVP)
- r / s / x: MCP reconnect / start / stop (MCP tab)
- Ctrl+Space / F6: Pause/resume active Petri/tournament runtime (global)

## Agent Chat
- Type message, Enter to send, Esc or Ctrl+C to clear input (`@all <msg>` broadcasts)
- Left/Right/Home/End: Move input cursor
- Up/Down: Move input cursor between lines
- Ctrl+Up/Ctrl+Down: Scroll chat thread

## Visualizer (GoL)
- Ctrl+E: Cycle seed encoder
- Ctrl+V: Toggle view (GENOME ↔ PLATE)
- Ctrl+R: Cycle seed view (genome/plate/map/stats)
- Ctrl+M: Cycle plate render (solid/half/braille/tissue/heat)
- Ctrl+Y: Toggle seed source (Editor/Notes)
- Ctrl+A: Apply seed search proposal
- Ctrl+G: Toggle seed search
- Ctrl+N: Snapshot seed (SNAP)
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
- :gol rule: Show current rule + built-ins
- :gol rule <id|B/S>: Set rule by id or B/S string
- :gol rules: List available rules
- :games run: Run Games tournament
- :games hide: Hide Games Petri Dish (tournament keeps running)
- :games show: Show Games Petri Dish
- :games status: Show tournament status
- :games export: Re-emit last run summary (if present)
- :games runs: Open run browser
- :games replay: Open match replay selector (uses loaded run summary)
- :games inspect <strategy_id>: Show introspection for a strategy (pretty text)
