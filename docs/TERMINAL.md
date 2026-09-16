# Terminal

nit can run a real OS shell inside the TUI, with text selection, copy, and
scrollback. This doc covers where the shell appears, its keys, and how
selection and scrolling work. All key bindings live in `docs/KEYBINDINGS.md`.

## What it is

nit starts your `$SHELL` (or `/bin/sh` when it is unset) in a PTY. A `vt100`
emulator parses the output and nit paints it into its own frame. The shell sees
`TERM=xterm-256color`, so colours, prompts, and full-screen programs work.

The shell is not your host terminal. nit captures the mouse and keyboard to
drive the rest of the UI, so nit provides selection and scrollback itself. This
is the same model `tmux`, vim's `:terminal`, and editor terminals use.

## Three surfaces

| Surface | Where it appears | Behaviour |
|---|---|---|
| Agent-chat tab | The chat pane toggles between `AGENT CHAT` and `TERMINAL`. | Tabbing away parks the shell. Tabbing back resumes the same session. |
| Modal popup | A centred overlay over whatever you are doing. | Closing hides the shell. Reopening resumes the same session. |
| Multipane pane | Any pane can flip its `NIT` / `TERM` title pill to a terminal. | A grid of independent shells runs side by side. |

The shell ends only when it exits (you run `exit`, or it dies) or when nit
quits.

In multipane mode, `nit multipane --terminal-command <COMMAND>` (one per pane)
starts each pane with its terminal open running that command. See
`docs/MULTIPANE.md`.

## Keys

| Action | Key |
|--------|-----|
| Toggle the agent-chat terminal tab | `Ctrl+\` |
| Open or close the modal popup | `Ctrl+Shift+T` |
| Close the popup | `Esc Esc` (the first Esc reaches the shell, the second closes) |
| Toggle a multipane pane's terminal | click its `TERM` / `NIT` pill, or `Ctrl+\` on the focused pane |

Every other keystroke goes to the shell, so editors, REPLs, and full-screen
TUIs run as usual.

## Selecting and copying text

Drag with the left mouse button to select a rectangle of terminal text. nit
highlights the selection and copies it to the system clipboard when you release
the button. No extra copy keystroke is needed. This matches selection in nit's
editor and chat panes. It works in the agent-chat terminal, the popup, and the
focused multipane pane.

## Scrolling

The terminal keeps 10,000 lines of scrollback. Scroll the mouse wheel over any
terminal to move through history. Typing snaps the view back to the live
bottom, like a real terminal. The scrollback belongs to the `vt100` emulator;
the wheel only moves its offset.

## Notes

- Rendering is separate from output. The PTY reader thread keeps the grid
  current on its own, and nit samples it at its normal frame rate. A chatty
  process cannot flood nit's redraw loop. The popup repaints every frame to
  keep live output smooth. See `docs/PERF.md`.
- A parked or hidden terminal keeps its shell, and any running process, alive
  in the background until you return to it or quit nit.
