# Environment settings

nit runs with its defaults. To override a setting for one launch in a POSIX shell, put it before the command.

```sh
NIT_TUI_FPS=30 nit .
```

For a PowerShell session, use `$env:NIT_TUI_FPS = "30"` before launching nit. Remove the variable to restore the default (`Remove-Item Env:NIT_TUI_FPS` in PowerShell).

## Display, logs, and update checks

| Variable | Default | When to use it |
|---|---|---|
| `NIT_TUI_FPS` | About 60 frames/s | Set a redraw cap from 15 to 120 for the station and multipane grid. Values outside the range use the default. |
| `NIT_ASCII_FALLBACK` | Unset | Set to `1` if your terminal has trouble displaying Agent Ops symbols. |
| `NIT_ROSTER_NO_TRUNCATE` | Unset | Set to `1` to show all agent rows in large swarms, including per-mission and chat activity rows. |
| `NIT_LOG_PATH` | Workspace-specific file in nit's state directory under `logs/` | Set a path to collect a session's log in a known location. |
| `NIT_NO_VERSION_CHECK` | Unset | Set to `1` to disable the launch-time update check. |

## Manage agent resources and timeouts

| Variable | Default | Effect |
|---|---|---|
| `NIT_CLAUDE_POOL` | Off | Set to `1` to reuse Claude processes. A compatible direct chat can accept follow-ups while its turn runs. Nit binds the worker to the agent, workspace, session, effort, permissions, and turn limit. |
| `NIT_CLAUDE_POOL_SIZE` | One quarter of the effective swarm limit, bounded to 2–8 | Set a positive worker count when pooling is enabled. Parked workers use file descriptors and reduce capacity for other concurrent turns. |
| `NIT_CLAUDE_TURN_IDLE_TIMEOUT_SECS` | 900 seconds | End a turn after this long without streamed output. A turn that has used a recognized write tool is exempt. Set to `0` to disable. |
| `NIT_MCP_TURN_TIMEOUT_SECS` | No limit | Limit total duration of a Codex MCP turn, in seconds. `0` disables the limit. |
| `NIT_MCP_TURN_IDLE_TIMEOUT_SECS` | No limit | Limit time without Codex turn notifications, in seconds. `0` disables the limit. |

When a Codex MCP timeout fires, nit restarts the shared MCP server. **All turns in flight on that server fail**, including turns that were still making progress.

## Tune planning and verification

| Variable | Default | Effect |
|---|---|---|
| `NIT_INTAKE_DISABLED` | Unset | Set to `1` or `true` to send prompts without the preliminary file-checklist step. The config equivalent is `intake_enabled = false`. See [Prompt intake](INTAKE.md). |
| `NIT_SCOPE_WALK_TIMEOUT_MS` | 200 ms | Set how long a dispatch waits to identify source files mentioned by directory in the prompt. `0` proceeds immediately. |
| `NIT_PROMPT_TIERS` | Enabled | Set to `0`, `false`, `no`, or `off` to disable role-specific prompt size limits. |
| `NIT_PROMPT_BUDGET_<ROLE>` | See below | Set one role's maximum prompt size in decimal bytes. For example, `NIT_PROMPT_BUDGET_INTEGRATE=600000`. |
| `NIT_PLANNER_LEGACY` | Unset | Set to `1`, `true`, `yes`, or `on` to use a single planning pass and skip plan validation and repair. |
| `NIT_STRICT_CHECKLIST` | Unset | Set to `1` to request another integration pass when the agent skips a file on its checklist. |
| `NIT_NO_COMPILE_GATE` | Unset | Set to `1` to skip the automatic post-edit Cargo check for affected Rust crates. |

The default byte limits are **INTEGRATE** 480,000, **JUDGE** 320,000, **RESEARCH** 240,000, **PROPOSE** 160,000, **REVIEW** 120,000, **TEST** 96,000, and **DEFAULT** 96,000. To override a limit for one mission, use the `budget=ROLE:N` option in its `@swarm` prompt. [Swarm](SWARM.md)

## Enable multiway search

| Variable | Default | Effect |
|---|---|---|
| `NIT_MULTIWAY` | Off | Set to `1` to enable `@multiway` and its search graph. |
| `NIT_MULTIWAY_GATES` | Detect checks for each worktree | Set the checks each candidate must pass, separated by semicolons. An explicitly empty value disables command checks. |

For example, `NIT_MULTIWAY_GATES="uv run pytest -q"` runs that command inside each candidate worktree. Commands are executed directly, so shell pipes and redirection require a separate script. See [Multiway search](MULTIWAY.md).

## Run and record simulations

| Variable | Default | Effect |
|---|---|---|
| `NIT_SNAPSHOT_QUEUE` | 64 | Number of snapshots the writer can queue. |
| `NIT_SNAPSHOT_CYCLE` | Unset | Set to `1` to save a snapshot when the simulation detects a repeating cycle. |
| `NIT_SNAPSHOT_DEBUG` | Unset | Set to `1` for diagnostic snapshot output. |
| `NIT_GOL_STACK_MB` | 256 MiB | Game of Life worker thread stack size, with a minimum of 32 MiB. |
| `NIT_GOL_IO_STACK_MB` | Uses `NIT_GOL_STACK_MB`, otherwise 256 MiB | Override the I/O thread stack for the snapshot stress utility. |
| `NIT_GAMES_DISABLE_METAL` | Unset | Set to `1` to disable Metal acceleration. Games with CPU support can use their CPU implementation. See [Games](GAMES.md). |

nit supplies `NIT_MCP_AGENT_ID` and the connection address (`NIT_MCP_BACKCHANNEL_SOCKET` or `NIT_MCP_BACKCHANNEL_PORT`) to its MCP subprocesses automatically.
