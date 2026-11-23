# Work in several directories at once

Use `nit multipane` when you want to follow several agent conversations or shells in one terminal. Each pane has its own working directory, prompt history, and agent selection. Click a pane to type in it. The others keep running.

## Start a grid

```sh
nit multipane --panes 4
nit multipane --panes 4 --backend claude --cwd .
nit multipane --panes 2 --terminal-command "git status" --terminal-command "git log -5"
```

The first command opens four agent pickers. The second limits each picker to Claude models. The third runs one shell command in each of two panes. A pane returns to its chat or picker when its command exits. For a persistent live display, use a command that keeps running, such as a test watcher.

| Option | What it changes |
|---|---|
| `--panes N` | Opens 1–32 panes. The default is 8 in three columns. Enlarge the terminal or choose fewer panes if the grid cannot fit. |
| `--cwd PATH` | Starting directory for the panes. Defaults to the directory where you launch nit. |
| `--backend ID` | A specific model ID opens chat in each pane. A family name (`codex`, `claude`, `gemini`, or `local`) filters the picker. Omit the flag to see the available roster. |
| `--terminal-command COMMAND` | Launches a shell command directly. When using this option, supply one nonempty command per pane, in pane order. |

## Choose an agent and send work

Use the arrows or `j`/`k` to select a model and press `Enter` to open its chat. Use Left/Right or `h`/`l` to collapse or expand a group. On a reasoning-effort option, press `Space` or `Enter` to select it. `PageUp`/`PageDown` move through long rosters, and `g`/`G` jump to the ends. Click a **Template** or **Mission** choice to set how that pane starts a swarm.

Type a prompt and press `Enter`. Use Up/Down for prompt history and `PageUp`/`PageDown` or the wheel to read replies. Use `Ctrl+R` to pick another agent. This clears that pane's draft and active mission selection. See [Swarm](SWARM.md) for task distribution and [Shadow](SHADOWS.md) for single-agent review.

| While working | Control |
|---|---|
| Focus another pane | Click it, or cycle with `Tab` / `Shift+Tab`. |
| Open a shell in this pane | Click **TERM** or press `Ctrl+\`. Click **NIT** to resume chat. See [Terminal](TERMINAL.md). |
| Cancel work in this pane | Send `/abort`, press `Ctrl+C` with empty input, or press `Esc` twice quickly. |
| Cancel all work owned by this pane | Send `/abort all`. Other panes continue. Use `/abort AGENT-ID` for one agent in this pane. |
| See the grid's shortcuts | Press `F1`, or `?` with empty chat input. |

## Point a pane at another directory

In chat, press **F2** or **Ctrl+/**. An empty query lists the current directory's children. Type part of a name to search descendants. Start with `../` or `../../` to search from a parent, `/` for an absolute path, or `~/` for your home directory.

- Use Up/Down or `Ctrl+K`/`Ctrl+J` to highlight a result. Right/Left or `Ctrl+L`/`Ctrl+H` expands or collapses it.
- Press `Alt+F` to include hidden directories. The search skips common build directories and directories excluded by workspace settings.
- Press `Enter` to use the highlighted directory for subsequent agent work. Press `Esc` to return with the draft intact. Switching panes also closes the search.

## Leave and resume

`Ctrl+Q` exits the grid immediately and ends its shells. Relaunch in the same workspace with the same pane count to restore a saved layout, including working directories, focus, drafts (up to 4 KiB per pane), prompt histories, and swarm settings. A first session in which no pane has run a mission leaves no saved layout. Use plain `nit` to return to the editor and Git change review.
