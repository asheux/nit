# Keybindings

## Global
- Ctrl+Q: Quit (confirm if dirty)
- Ctrl+S: Save
- Tab / Shift+Tab: Cycle pane focus
- Ctrl+H/J/K/L: Focus panes (vim/tmux style: left/down/up/right)
- F1 / ?: Toggle help overlay
- Ctrl+Enter: Run Petri Dish simulation popup
- Ctrl+^: Show hidden Petri Dish
- : (Normal mode): Command prompt

## Editor & Notes (focused)
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

## Job Output
- Ctrl+L: Clear logs
- Ctrl+Space: Pause/resume job updates

## Visualizer
- Ctrl+E: Cycle seed encoder
- Ctrl+V: Toggle view (GENOME ↔ PLATE)
- Ctrl+R: Cycle seed view (genome/plate/map/stats)
- Ctrl+M: Cycle plate render (solid/half/braille/tissue/heat)
- Ctrl+Y: Toggle seed source (Editor/Notes)
- Ctrl+A: Apply seed search proposal
- Ctrl+G: Toggle seed search
- Ctrl+N: Snapshot seed (SNAP)
- Ctrl+Shift+V: Cycle seed overlays

## Petri Dish (Popup)
- Esc: Close popup
- Space: Pause/resume
- Enter: Step one generation
- + / -: Speed up/down
- S: Snapshot sim state
- Ctrl+R: Reseed from current code
- H: Hide popup (sim keeps running)
- T: Toggle wrap mode
- O: Cycle auto-stop policy (Off → Fixed → Repeat)
- G: Toggle rule search
- A: Apply best rule

## Command/Prompts
- Y / N to confirm quit when prompted
- :gol hide / :petri hide: Hide Petri Dish (sim keeps running)
- :gol show / :petri show: Show Petri Dish
