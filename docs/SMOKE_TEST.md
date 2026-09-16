# Smoke Test / Feature Tour

A checklist for quick confidence after changes to the UI, commands, or engine
wiring. Run each step and compare what you see with the "Expect" line. The
full key list is in `docs/KEYBINDINGS.md`.

## Build + CI Checks

- Format: `just fmt`
  - Expect: clean run, no formatting diffs.
- Lint: `just clippy`
  - Expect: success with `-D warnings` (no warnings).
- Tests: `just test`
  - Expect: all tests pass.
- Build: `cargo build`
  - Expect: clean compile.
- Preflight (one shot): `scripts/healthcheck.sh` (add `--deep` for clippy and tests)
  - Expect: every check labelled `[green] pass`; exit code 0.

## Core TUI (Applies To All Labs)

- Launch: `cargo run --`, `cargo run -- path/to/file`, or `cargo run -- .`
  - Expect: a multi-pane editor TUI (Editor / Notes / Logs plus right-side panes).
  - Expect: with no args or a directory target, NITTree opens in the Editor pane, rooted at the cwd or target dir.
- NITTree:
  - Toggle: `Ctrl+T` or `:tree`
    - Expect: the tree opens and closes as an Editor-pane overlay. The editor buffer stays intact under it.
  - Navigate: `j/k` or `Up/Down`, `PageUp/PageDown`, `Home/End`
    - Expect: the selection stays visible. Directories auto-expand along the selected path and auto-collapse when you leave.
  - Open: `Enter` on a file
    - Expect: the file loads into the editor buffer and the tree closes.
  - Filters: `.` toggles dotfiles; `r` refreshes; `.git` never appears.
    - Add an entry to `.gitignore` and refresh. Expect: the ignored files and dirs disappear.
- Fuzzy search:
  - File search: `Ctrl+P` (or `:find`)
    - Expect: a centered popup in FILES mode with a query prompt and a scrolling file list.
    - Type a few characters. Expect: results filter quickly and the selection stays visible.
    - `Enter` on a result. Expect: the file opens in the editor and the popup closes.
  - Content search: `Ctrl+F` (or `:grep`)
    - Type a query such as `fn main`. Expect: matches stream in as they are found.
    - Move the selection. Expect: the preview updates and highlights the match line.
    - `Enter` on a match. Expect: the file opens and the cursor jumps to the matched line and column.
  - Gitignore: add an entry to `.gitignore`, reopen search, and press `Ctrl+R` (or `F5`).
    - Expect: ignored paths disappear, unless `F3` / `Ctrl+G` shows ignored files.
- Pane focus: `Tab` / `Shift+Tab`, `Ctrl+H/J/K/L`
  - Expect: focus moves between panes. The border or selection changes.
- Help overlay: `F1` (any time) or `?` (Normal mode)
  - Expect: a help popup with keybinds and `:` commands.
- Command prompt: `:` (Normal mode)
  - Expect: a prompt line. Enter runs the command; status and logs show the result.
  - Try `:help` or `:commands`. Expect: the help popup opens.
  - Try `:q`. Expect: a quit confirmation if dirty, otherwise the app exits.
- Quit confirmation: make an edit, then `Ctrl+Q`
  - Expect: a confirmation prompt. `Y` quits, `N` cancels.
- Save: `Ctrl+S`
  - Expect: the dirty indicator clears and the file is written to disk.
- Debug mode: `Ctrl+B`
  - Expect: debug information appears, and toggles off again.

## Agent Station (Codex/Claude/MCP)

- Preconditions:
  - `codex` installed and on `$PATH` (for `--agents codex`)
  - `~/.codex/models_cache.json` present (seeds the Codex roster)
  - `claude` installed and on `$PATH` (for `--agents claude`)
- Launch with Codex lanes: `cargo run -- --agents codex`
  - Expect: the Agent Ops roster lists Codex models.
- MCP transport (default runtime):
  - In Agent Ops, switch to the MCP tab.
  - Expect: status becomes CONNECTED and the endpoint shows `stdio://...`, backed by `codex mcp-server`.
  - Press `x` (stop), `s` (start), `r` (reconnect). Expect: the status follows each press.
  - `r` keeps saved Codex thread ids for continuations. `x` clears them, so the next prompt starts a new thread.
- A turn over MCP:
  - Focus Agent Chat (`Enter` from Agent Ops).
  - Send a short prompt. Expect: stage updates while it runs, then `done (see ARTIFACTS)` in the thread.
  - Expect: the full formatted reply in Agent Ops, ARTIFACTS tab (open the REPLY card).
- Parallel turns (multi-agent):
  - Launch with `cargo run -- --agents codex --codex-max-parallel-turns 2`.
  - In Agent Ops (Roster): select a model, `Enter` to Agent Chat, send a prompt.
  - While it runs: return to Agent Ops, select a different model, `Enter`, send another prompt.
  - Expect: both models show `RUNNING` in the roster and Agent Chat shows a multi-agent "Working" table.
  - Expect: each agent finishes on its own, with `done (see ARTIFACTS)` per agent. Full outputs are in Agent Ops, ARTIFACTS tab.
  - Optional: create a mission (`n` in Agent Ops), then send `@all <prompt>` in Agent Chat to broadcast to the mission's assigned Codex agents.
- `@swarm` orchestration (task splitting and synthesis):
  - In Agent Chat, with any Codex or Claude model selected, send `@swarm 4 template=lab <prompt>`. `lab` is the default, so `template=...` is optional.
  - Expect: a new mission (the Missions tab shows `SWM yes`), and the planner runs first (phase `PLAN`).
  - Expect: during planning, Agent Ops DAG tab shows `Planning: waiting for planner output`.
  - Expect: after the planner returns a JSON plan, the DAG tab shows multi-line task cards (they wrap instead of `...`) with correct `Pending`, `Queued`, and `Skipped` states.
  - Expect: Agent Chat keeps the compact "Working/Queued" table, with swarm metadata below it (template, integrator, verifier, gates).
  - Expect: tasks run as a DAG (phase `EXECUTE`, status like `EXEC 1/6`). Some tasks wait until their deps finish.
  - Expect: when all task agents finish, nit runs a verifier turn (phase `VERIFY`, status `VERIFY`) that runs a built-in gate bundle when it detects one.
  - Expect: per-gate outcomes in the DAG tab, as PASS / FAIL (and SKIP when reported).
  - Expect: after verification, the planner runs a synthesis turn (status `SYNTH`) and the mission ends as:
    - `DONE` when gates pass, or no gates were detected
    - `FAILED` when gates ran and failed
    - `ERROR` when verification errored, for example a missing or invalid gate report JSON
  - Template `bulk`:
    - Send `@swarm 5 template=bulk <prompt>`.
    - Expect: several "propose" tasks run first in parallel, then a "judge" task, then an integrator task (`writes=true`) before VERIFY / SYNTH.
    - Expect: Agent Ops switches to the DAG tab by itself when a bulk swarm starts.
  - Bulk roster roles:
    - In Agent Ops Roster, select the `bulk` template (press `3`).
    - Expand a model row. Expect: a `Size` branch and a `Role` branch.
    - Under `Role`, pick `integrate` for a non-planner model.
    - Launch a bulk swarm (`@swarm 5 template=bulk <prompt>`, or implicit bulk).
    - Expect: swarm metadata names that model as the integrator, and the `integrate` task goes to it.
  - Priority roster hint:
    - In Agent Ops Roster, mark one Codex model as priority (`[x]`) with Space on the model row or a mouse click.
    - Launch a `parallel` or `bulk` swarm with a limited size (`@swarm 4 template=parallel <prompt>`).
    - Expect: the swarm mission includes the priority model, even when more models exist than the swarm size.
    - Expect: the planner prompt has a "Priority agents" section listing that model.
  - Implicit bulk launch (no `@swarm`):
    - In Agent Ops Roster, select the `bulk` template (press `3`).
    - In Agent Chat, send a plain prompt such as `do a quick repo health check and suggest next steps`.
    - Expect: the swarm starts as if you had typed `@swarm template=bulk ...`.
  - Deadlock (cyclic plan):
    - If the planner returns a cyclic plan or unknown deps under the default strict DAG mode, expect a `PLAN error` that explains the invalid DAG. The mission stops before `VERIFY` / `SYNTH` with status `FAILED`.
  - Structured artifact persistence:
    - Have a task emit a `swarm_artifacts` JSON block (files, diffs, commands, risks, notes).
    - Expect: the selected mission shows task artifacts in Agent Ops, ARTIFACTS tab.
    - Expect: files under `.nit/swarm/<mission>/`: `run.json`, `tasks/<task-id>/artifacts.json`, `tasks/<task-id>/output.md`, and optionally `gates/report.json`, `gates/output.txt`, `gates/verify.md`.
  - Mission focus:
    - Send `@swarm template=lab mission=research <prompt>`.
    - Expect: the mission is classified `research`, and research roles are allowed in the plan.
    - Send `@swarm template=lab mission=computational-research <prompt>`.
    - Expect: the mission is classified `computational-research`, and both research and computational-research roles are allowed.
  - Gate bundle override:
    - Add `.nit/config.toml` with `[swarm.gates]` and `default = "none"` (or `rust-ci` / `node-ci` / `python-ci` / `go-ci`).
    - Expect: swarm metadata and VERIFY follow the override.
- Claude agent:
  - Launch with `cargo run -- --agents claude`.
  - Expect: the Agent Ops roster lists Claude models, probed with `claude models --json`.
  - Focus Agent Chat and send a short prompt.
  - Expect: stage updates while it runs, then `done (see ARTIFACTS)` in the thread.
  - Expect: the session resumes across prompts (Claude uses `--resume <session_id>`).
- `@shadow` pipeline (single-agent propose / judge / review):
  - Select one Codex or Claude agent in Agent Ops Roster.
  - In Agent Chat, send `@shadow <short prompt>`.
  - Expect: the "breather" above the chat cycles through `Proposing ...`, `Judging ...`, `Reviewing ...`, and `Finalizing ...` before the main agent answers.
  - Expect: no shadow lanes in the roster or chat. Only the main agent's reply appears at the end.
  - Auto-shadow: with no `@` prefix, send a prompt longer than 500 characters, or one that contains a keyword such as `refactor`, `rewrite`, or `implement`. Expect: the same four-stage breather before the final reply.
  - Suppression: `@swarm`, `@all`, `@new`, `@queue`, and `@q` prompts must not trigger shadows. Shadows never run inside an active swarm mission.
  - Failure fallback: if any shadow turn fails, nit re-dispatches the main agent with the plain prompt.
- Mixed agents:
  - Launch with `cargo run -- --agents all` (the default).
  - Expect: the roster shows Codex, Claude, and, if detected, Gemini models.
  - Expect: Gemini models appear but cannot run (no runtime runner).
  - Send prompts to both Codex and Claude agents. Expect: both work independently.
- Failure modes:
  - If Codex is offline or misconfigured, expect MCP state ERROR with details in Agent diagnostics and logs.
  - If the Claude CLI is not on PATH, expect no Claude lanes in the roster and no crash.

## Editor + Notes

- Mode switching: `Esc` to Normal; `i` / `a` / `o` / `Shift+O` to Insert; `v` to Visual
  - Expect: vim-like modal behaviour for movement versus editing.
- Editing: type in Insert mode; use `Backspace` / `Delete`; press `Enter` for newlines
  - Expect: stable cursor movement and correct text edits.
- Selection ops: Visual mode plus `y` (yank) or `d` (delete), then `p` / `Shift+P` (paste)
  - Expect: the selection transforms correctly. Paste respects line and inline modes.
- Undo / redo: `u` (undo), `Shift+R` (redo)
  - Expect: edits revert and reapply.
- Syntax highlight: with the Editor focused and not in Insert mode, `Shift+S`
  - Expect: highlighting toggles on and off. The Gate Monitor shows the status.

## GoL Lab (Visualizer + Petri Dish)

- Launch: `cargo run -- gol` or `cargo run -- --lab gol`
  - Expect: the GoL Visualizer pane is active.
- Open the Petri Dish popup: `Ctrl+Enter`
  - Expect: the GoL simulation popup opens and starts stepping.
- Pause / step / speed: `Space` pauses and resumes; `Enter` steps when paused; `+` / `-` changes speed
  - Expect: the generation counter follows.
- Hide / show: `H` hides (the sim continues); `Ctrl+^` shows the hidden popup
  - Expect: visibility toggles without stopping the sim.
- Reseed from code: in the popup, `Ctrl+R`
  - Expect: a seed derived from the current editor or scratchpad content. The sim restarts on it.
- Rule picker: in the popup, `F2` or `Ctrl+P`
  - Expect: a rule list plus custom input. Selecting updates the active rule.
- Protocol picker: in the popup, `P`
  - Expect: the protocol picker opens. Selecting applies a protocol.
- Rule search: in the popup, `G` toggles rule search and `A` applies the best rule
  - Expect: the leaderboard updates. Applying swaps the live rule.
- Snapshots: Visualizer `Ctrl+N` (seed snapshot); popup `S` (sim snapshot)
  - Expect: snapshot files under `gol-snapshots/` in the workspace.

## Games Lab (TUI Tournament + Inspector)

- Launch: `cargo run -- games` or `cargo run -- --lab games`
  - Expect: the Games UI is active. It reads `games.toml` by default.
- Run a tournament: `Ctrl+Enter` or `:games run`
  - Expect: the tournament popup appears, and run output lands under `runs/games/...`.
- Pause / step / speed: `Space` pauses and resumes; `Enter` steps one round when paused; `+` / `-` changes speed
  - Expect: the round counter follows.
- Hide / show: `H` hides (the tournament continues); `Ctrl+^` shows the hidden popup
- Run browser: `:games runs`
  - Expect: a list of saved runs. Selecting one loads it.
- Replay: load a run, then `:games replay`
  - Expect: the replay selector opens with per-match data.
- Strategy inspector: `:games strategy` (from a loaded run), `:games strategies all`, or `:games strategies config`
  - Expect: a browseable list of strategies.
- Single strategy: `:games inspect <strategy_id>`
  - Expect: introspection text and details.
  - Rule tuple override (one-sided TM): `:games inspect <id> {rule,states,symbols}` or `:games inspect {rule,states,symbols}`
    - Expect: the inspector shows the generated TM's decoded transitions and metadata.
- TM simulation: `:games tm {rule,states,symbols} <input> [steps]`
  - Expect: the TM simulation view opens with a trace and summary.
- History analysis: enable history in `games.toml` (`[history] enabled = true`), run a tournament, then `:games analyze`
  - Expect: analysis outputs next to the history log (JSON, CSV, and NDJSON).

## Games Lab (Headless CLI)

- Headless run: `cargo run -- games run --config games.toml --out . --format pretty`
  - Expect: a run directory with `run_summary.json` and related outputs.
- Sweep: `cargo run -- games sweep --config games.toml --rounds 200,500 --noise 0.0,0.05 --repetitions 1,3`
  - Expect: one run per point in the parameter grid.
- Enumerate FSMs: `cargo run -- games enumerate fsm --states 2..4 --out ./generated --canonical --limit 1000`
  - Expect: NDJSON strategy files under `./generated`.
- Inspect a strategy: `cargo run -- games inspect --config games.toml --id <strategy_id> --format pretty`
  - Expect: introspection on stdout, or in `--out <path>`.
- Export a strategy graph: `cargo run -- games graph --config games.toml --id <strategy_id> --out ./graph.dot`
  - Expect: DOT (or JSON) written to the output path.
