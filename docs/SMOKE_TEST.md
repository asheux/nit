# Smoke Test / Feature Tour

This doc is a practical checklist: run these steps and compare the expected behavior.
It's meant for quick confidence after changes to UI, commands, or engine wiring.

## Build + CI Checks

- Format: `just fmt`
  - Expect: clean run; no formatting diffs.
- Lint: `just clippy`
  - Expect: success with `-D warnings` (no warnings).
- Tests: `just test`
  - Expect: all tests pass.
- Build: `cargo build`
  - Expect: clean compile.

## Core TUI (Applies To All Labs)

- Launch:
  - `cargo run --`
  - `cargo run -- path/to/file`
  - `cargo run -- .`
  - Expect: a multi-pane editor TUI (Editor/Notes/Logs + right-side panes).
  - Expect: with no args or a directory target, NITTree opens in the Editor pane (rooted at the cwd/target dir).
- NITTree:
  - Toggle:
    - `Ctrl+T` or `:tree`
    - Expect: tree opens/closes as an Editor-pane overlay (editor buffer stays intact under it).
  - Navigation:
    - `j/k` or `Up/Down`, `PageUp/PageDown`, `Home/End`
    - Expect: selection stays visible; directories auto-expand along the selected path and auto-collapse when you leave.
  - Open:
    - `Enter` on a file
    - Expect: file loads into the editor buffer and the tree closes.
  - Filters:
    - `.` toggles dotfiles; `r` refreshes; `.git` never appears.
    - Add an entry to `.gitignore`, refresh, and expect ignored files/dirs to disappear.
- Fuzzy search:
  - File search:
    - `Ctrl+P` (or `:find`)
    - Expect: centered popup with FILES mode, query prompt, and a scrolling file list.
    - Type a few characters; expect: results filter quickly and selection stays visible.
    - `Enter` on a result; expect: file opens in editor and popup closes.
  - Content search:
    - `Ctrl+F` (or `:grep`)
    - Type a query (e.g. `fn main`); expect: matches stream in as they are found.
    - Move selection; expect: preview updates and highlights the match line.
    - `Enter` on a match; expect: file opens and cursor jumps to the matched line/col.
  - Gitignore:
    - Add an entry to `.gitignore`, re-open search and press `Ctrl+R` (or `F5`); expect: ignored paths disappear (unless `F3`/`Ctrl+G` shows ignored).
- Pane focus:
  - `Tab` / `Shift+Tab`
  - `Ctrl+H/J/K/L`
  - Expect: focus moves between panes (border/selection changes).
- Help overlay:
  - `F1` (any time) or `?` (Normal mode)
  - Expect: help popup with keybinds and `:` commands.
- Command prompt:
  - `:` (Normal mode)
  - Expect: a prompt line; Enter executes; status/logs show results.
  - Try `:help` or `:commands`; expect: help popup opens.
  - Try `:q`; expect: quit confirmation if dirty, otherwise app exits.
- Quit confirmation:
  - Make an edit, then `Ctrl+Q`
  - Expect: confirmation prompt; `Y` quits, `N` cancels.
- Save:
  - `Ctrl+S`
  - Expect: dirty indicator clears; file writes to disk.
- Debug mode:
  - `Ctrl+B`
  - Expect: debug information appears (and toggles back off).

## Agent Station (Codex/MCP)

- Preconditions:
  - `codex` installed and accessible on `$PATH`
  - `~/.codex/models_cache.json` present (for `--agents codex`)
- Launch with Codex lanes:
  - `cargo run -- --agents codex`
  - Expect: Agent Ops roster populated with Codex models.
- Verify MCP transport (default runtime):
  - In Agent Ops: switch to the MCP tab.
  - Expect: status transitions to CONNECTED and endpoint shows `stdio://...` (backed by `codex mcp-server`).
  - Press `x` (stop), `s` (start), `r` (reconnect); expect status updates accordingly.
- Verify a turn over MCP:
  - Focus Agent Chat (from Agent Ops: `Enter`).
  - Send a short prompt; expect: stage updates while running and an agent reply appended to the thread.
- Verify parallel turns (multi-agent):
  - Launch with `cargo run -- --agents codex --codex-max-parallel-turns 2`.
  - In Agent Ops (Roster): select a model, `Enter` to Agent Chat, send a prompt.
  - While the first turn is running: return to Agent Ops, select a different model, `Enter`, send another prompt.
  - Expect: both models show `RUNNING` in the roster, Agent Chat shows a multi-agent “Working” table, and replies arrive independently.
  - Optional: create a mission (`n` in Agent Ops), then in Agent Chat send `@all <prompt>` to broadcast to the mission’s assigned Codex agents.
- Verify `@swarm` orchestration (task splitting + synthesis):
  - In Agent Chat (any Codex model selected): send `@swarm 4 template=lab <prompt>` (or omit `template=...` since `lab` is the default).
  - Expect: a new mission is created (Missions tab shows `SWM yes`) and the planner runs first (phase `PLAN`).
  - Expect: after the planner returns a JSON plan, tasks run as a DAG (phase `EXECUTE`, status like `EXEC 1/6`), with some tasks queued until their deps finish.
  - Expect: when all task agents finish, nit runs a verifier turn (phase `VERIFY`, status `VERIFY`) to execute a built-in gate bundle when detected.
  - Expect: after verification completes, the planner runs a synthesis turn (status `SYNTH`) and the mission status becomes:
    - `DONE` when gates pass (or no gates were detected)
    - `FAILED` when gates ran and failed
    - `ERROR` when verification errored (e.g., missing/invalid gate report JSON)
- Failure mode sanity:
  - If Codex is offline/misconfigured, expect MCP state ERROR and details in Agent diagnostics/logs.

## Editor + Notes

- Mode switching:
  - `Esc` -> Normal
  - `i`/`a`/`o`/`Shift+O` -> Insert
  - `v` -> Visual
  - Expect: vim-like modal behavior for movement vs editing.
- Editing:
  - Type in Insert mode; use `Backspace`/`Delete`; press `Enter` for newlines.
  - Expect: stable cursor movement and correct text edits.
- Selection ops:
  - Visual mode + `y` (yank), `d` (delete), then `p`/`Shift+P` (paste).
  - Expect: selection transforms correctly; paste respects line/inline modes.
- Undo/redo:
  - `u` (undo), `Shift+R` (redo)
  - Expect: edits revert/reapply.
- Syntax highlight:
  - With Editor focused (not Insert): `Shift+S`
  - Expect: syntax highlighting toggles on/off (Gate Monitor reflects status).

## GoL Lab (Visualizer + Petri Dish)

- Launch GoL lab:
  - `cargo run -- gol`
  - or `cargo run -- --lab gol`
  - Expect: GoL Visualizer pane active.
- Open Petri Dish popup:
  - `Ctrl+Enter`
  - Expect: GoL simulation popup opens and starts stepping.
- Pause/step/speed:
  - `Space` pause/resume
  - `Enter` steps (when paused)
  - `+` / `-` adjusts speed
  - Expect: generation counter behaves as described.
- Hide/show popup:
  - `H` hides (sim continues)
  - `Ctrl+^` shows hidden popup
  - Expect: popup visibility toggles without stopping the sim.
- Reseed from code:
  - In popup: `Ctrl+R`
  - Expect: seed derived from current editor/scratchpad content; sim restarts on new seed.
- Rule picker:
  - In popup: `F2` or `Ctrl+P`
  - Expect: rule list + custom input; selecting updates active rule.
- Protocol picker:
  - In popup: `P`
  - Expect: protocol picker opens; selecting applies a protocol.
- Rule search:
  - In popup: `G` toggles rule search; `A` applies best rule
  - Expect: leaderboard updates; applying swaps the live rule.
- Snapshots:
  - Visualizer: `Ctrl+N` (seed snapshot)
  - Popup: `S` (sim snapshot)
  - Expect: snapshot files appear under `gol-snapshots/` in the workspace.

## Games Lab (TUI Tournament + Inspector)

- Launch Games lab:
  - `cargo run -- games`
  - or `cargo run -- --lab games`
  - Expect: Games UI is active (reads `games.toml` by default).
- Run tournament:
  - `Ctrl+Enter` or `:games run`
  - Expect: Games tournament popup appears; run output written under `runs/games/...`.
- Pause/step/speed:
  - `Space` pause/resume
  - `Enter` steps one round (when paused)
  - `+` / `-` adjusts speed
  - Expect: round counter responds correctly.
- Hide/show popup:
  - `H` hides (tournament continues)
  - `Ctrl+^` shows hidden popup
- Run browser:
  - `:games runs`
  - Expect: list of saved runs; selecting loads a run.
- Replay:
  - Load a run, then `:games replay`
  - Expect: replay selector opens and shows per-match data.
- Strategy inspector:
  - `:games strategy` (from loaded run)
  - `:games strategies all` or `:games strategies config`
  - Expect: list of strategies is browseable.
- Strategy inspect (single):
  - `:games inspect <strategy_id>`
  - Expect: introspection text + details.
  - Rule tuple override (one-sided TM):
    - `:games inspect <id> {rule,states,symbols}`
    - `:games inspect {rule,states,symbols}`
    - Expect: inspector shows the generated TM's decoded transitions/metadata.
- TM simulation:
  - `:games tm {rule,states,symbols} <input> [steps]`
  - Expect: TM simulation view opens and displays the trace/summary.
- History analysis:
  - Enable history in `games.toml` (e.g. `[history] enabled = true`), run a tournament, then:
    - `:games analyze`
  - Expect: analysis outputs written next to the history log (JSON + CSV + NDJSON).

## Games Lab (Headless CLI)

- Headless run:
  - `cargo run -- games run --config games.toml --out . --format pretty`
  - Expect: run directory with `run_summary.json` and related outputs.
- Sweep:
  - `cargo run -- games sweep --config games.toml --rounds 200,500 --noise 0.0,0.05 --repetitions 1,3`
  - Expect: multiple runs produced (parameter grid).
- Enumerate FSMs:
  - `cargo run -- games enumerate fsm --states 2..4 --out ./generated --canonical --limit 1000`
  - Expect: NDJSON strategy file(s) produced under `./generated`.
- Inspect strategy:
  - `cargo run -- games inspect --config games.toml --id <strategy_id> --format pretty`
  - Expect: introspection printed to stdout (or `--out <path>`).
- Export strategy graph:
  - `cargo run -- games graph --config games.toml --id <strategy_id> --out ./graph.dot`
  - Expect: DOT (or JSON) written to the output path.
