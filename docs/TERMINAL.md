# Terminal

> **Status**: shipped. A real OS shell, embedded in nit and rendered as a
> grid, with text selection/copy and scrollback. Available in the agent-chat
> pane, as a modal popup, and per-pane in multipane mode.

## What it is

nit can host your `$SHELL` (falling back to `/bin/sh`) directly inside the
TUI. The shell runs in a PTY; its output is parsed by a `vt100` terminal
emulator and painted into nit's frame, so it behaves like a normal xterm
(`TERM=xterm-256color`) — colours, prompts, and full-screen programs work.
The shell is **not** your host terminal: nit captures the mouse and keyboard
to drive the rest of the UI, so terminal features like selection and
scrollback are provided by nit itself (the same model `tmux`, `vim`'s
`:terminal`, and editor integrated-terminals use).

## Three surfaces

- **Agent-chat tab** — the chat pane toggles between `AGENT CHAT` and
  `TERMINAL`. The shell is parked (not killed) when you tab away, so flipping
  back resumes the same session with its history and running processes.
- **Modal popup** — a centered overlay shell over whatever you're doing.
  Closing hides (not kills) it, so re-opening resumes the same session.
- **Multipane** — any pane can flip its `NIT` / `TERM` title pill to show a
  terminal, so a grid of independent shells runs side by side.

The shell is torn down only when it exits (you run `exit`, or it dies) or when
nit quits.

## Keys

| Action | Key |
|--------|-----|
| Toggle the agent-chat terminal tab | `Ctrl+\` |
| Open / close the modal terminal popup | `Ctrl+Shift+T` |
| Close the popup (reaches the shell first, closes on the double-tap) | `Esc Esc` |
| Toggle a multipane pane's terminal | click its `TERM` / `NIT` pill, or `Ctrl+\` on the focused pane |

All other keystrokes are forwarded to the shell, so editors, REPLs, and
full-screen TUIs run inside the terminal as usual.

## Selecting and copying text

Drag with the left mouse button to select a rectangle of terminal text; the
selection is highlighted and copied to the system clipboard on release (no
extra copy keystroke needed), matching how selection works in nit's editor and
chat panes. This works in the agent-chat terminal, the popup, and — for the
focused pane — in multipane.

## Scrolling

The terminal keeps **10,000 lines** of scrollback. Scroll the mouse wheel over
any terminal to move through history; typing snaps the view back to the live
bottom, just like a real terminal. (Scrollback is the `vt100` emulator's own —
the wheel just drives its offset.)

## Notes

- Rendering is decoupled from output: a chatty process can't storm nit's
  redraw loop. The PTY reader thread keeps the grid current on its own thread,
  and nit samples it on its normal frame cadence (the popup repaints every
  frame so live output stays smooth; see `docs/PERF.md`).
- A parked or hidden terminal keeps its shell — and any running process —
  alive in the background until you return to it or quit nit.
