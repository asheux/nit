# Permissions, network access and local records

Choose what a session can access before sending it a repository question. Opening a file in the editor, launching a model and starting a shell have different effects.

| Action | What it can access |
| --- | --- |
| Edit and save | Files your OS account can write. Saving an existing symlink updates its target. The target can be outside the workspace. Existing regular files retain their Unix read, write and execute bits. |
| Open **Changes** | Local Git status and the selected path’s staged and unstaged patches. Opening the pane leaves the index and working tree alone. |
| Use the embedded terminal | A shell running with your account’s permissions. Commands you enter can change files and make network requests. |
| Send a Codex or Claude turn | An external provider CLI using its configured credentials. The prompt, repository material read by that CLI and tool results may reach the provider. |
| Run a swarm verification command | The project’s tests or commands specified in `.nit/config.toml`. Read custom commands before running a mission in an unfamiliar checkout. |
| Start or update nit | The release check can contact `download.nit.tools`. Set `NIT_NO_VERSION_CHECK=1` to skip the startup check. Updating downloads release software. |

## Review a repository

For a local editing session, use `nit --agents local .`. For a live Codex review where the agent should only read files, start a session from the project directory.

```sh
nit --agents codex --codex-sandbox read-only .
```

Codex uses its own sandbox configuration when you omit the flag. Nit defaults the Codex approval policy to `never` for noninteractive turns. The sandbox controls what commands may do, and the approval setting controls requests for escalation. Claude’s standard writing turns allow file editing, shell commands and web tools subject to the CLI and account configuration. File claims displayed by the station are advisory. Other processes can still write the same files. [Agent coordination](SUBSTRATE.md)

## Decide what to share

| Location | What to inspect |
| --- | --- |
| `.nit/agents/runs/` | Mission records, messages and patch evidence |
| `.nit/swarm/` | Task outputs, verification results, final reports and follow-up receipts containing the operator’s note and selected source |
| `.nit/chat/followups/` | Direct chat corrections, their recipient and delivery status |
| `.nit/followup-drafts/` | Unsent follow-ups saved after a routing error, with their original text and source context |
| nit’s application state or data directory | Workspace logs and saved sessions. `NIT_LOG_PATH` selects a path for the workspace log. |
| `gol-snapshots/` and `runs/games/` | Simulation snapshots and tournament output when you use those labs |

Review generated files before committing them or attaching them to an issue. Provider sessions and credentials are also subject to the provider CLI’s storage settings. [Configuration](ENVIRONMENT.md)

## Report a security issue

Contact the project maintainers privately for a vulnerability involving active credentials, private repository contents or a risk to other users. A version, platform and minimal example with synthetic data help maintainers reproduce the problem.
