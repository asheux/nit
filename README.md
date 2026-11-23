# nit

nit is a terminal editor for working on a codebase with coding agents. Read the source, ask an agent about a specific decision, inspect its Git diff and run the check that matters to you, all in the same workspace.

## Start with a change

From a Git checkout, run `nit .`. Agent Chat opens in the center, with NITTree on the left and **Changes** on the right. Click a file in NITTree to open it and move the cursor into the editor. Press F6 to return to your conversation. With a clean working tree, select a recent commit to see its patch. You can try this workflow with an edit of your own or one an agent has just made.

1. **Find the decision.** Press `Ctrl+F`, search for a function name or error message, select a match and press Enter. Use `Ctrl+P` when you know the filename.
2. **Edit the source.** Press `i` to type, Esc to return to Normal mode, then `:w` and Enter to save.
3. **Read what changed.** Run `:changes`. Use ↑/↓ to pick a file and PageUp/PageDown to read the patch. A file can have both staged and unstaged edits. Enter opens the selected file.
4. **Check the behavior.** Press F4 and run the repository’s relevant test in the embedded shell. F4 returns to the editor and keeps the shell session alive.

### Bring in an agent

Install and sign in to the Codex or Claude CLI you want to use. Launch `nit --agents codex .` or `nit --agents claude .`, select a model in **Agent Ops** and press Enter to open **Agent Chat**. Give the agent a bounded question, such as “Read the permission check in this file. Which test covers a denied request? Cite the file and line.” Include the actual filename in your prompt. When the turn finishes, click **Artifacts** in its activity panel to read the reply. Follow the cited code and run the test yourself.

For independent reviews, `nit multipane --panes 4` opens four conversations. For a change with dependencies, a swarm can assign tasks, pass their results to another agent and collect verification evidence. Give it an acceptance criterion you can check. [Work with a team of agents](docs/SWARM.md) · [Use independent panes](docs/MULTIPANE.md)

### Follow up while Codex is working

To send a correction to an active Codex turn, launch Nit with the app-server runtime. This example keeps the review read-only.

```sh
nit --agents codex --codex-runtime app-server --codex-sandbox read-only .
```

Select a model in **Agent Ops**, ask a question, then send the correction in the same chat. When Codex confirms delivery to that turn, the follow-up receipt updates. Use `@queue MESSAGE` to reserve a separate turn. If the connection drops before confirmation, check the receipt and original reply before sending the correction again.

This option requires a Codex CLI with `codex app-server` and a model your provider allows for that client. A model listed by the CLI can still be refused by the account. If the provider reports `model_not_found`, select an available model or relaunch with `--codex-runtime auto` and resend your question. The default runtime continues to use MCP when the CLI supports it, otherwise exec. App-server declines requests for additional permissions. [Agent permissions](docs/SECURITY.md)

### Give agents only the guidance they need

The **NitSkills** pane opens with the editor. Its 16 playbooks cover code structure, swarm work, research and shared-workspace decisions. Select a playbook to preview it, or press Enter to read it in full. Click **Use skill** or press `a` to add a reference to your chat draft. When you send the draft, the agent can fetch that playbook. Swarm tasks receive their required instructions automatically. Use `:skills` to focus the pane, `/skills` in chat, or `nit skills NAME` from a shell. [Measured effect on prompt size](docs/PERF.md)

## Install

Install the latest release using one of the commands below.

| Platform | Command |
| --- | --- |
| Homebrew on macOS or Linux | `brew install asheux/tap/nit` |
| macOS, Linux x86-64 or WSL | `curl -fsSL https://download.nit.tools/install.sh \| bash` |
| Windows PowerShell | `irm https://download.nit.tools/install.ps1 \| iex` |

From the repository root, build and launch with the pinned Rust toolchain. The build also produces the companion `nit-mcp-server` executable.

```sh
cargo build --release --locked -p nit -p nit-mcp
./target/release/nit .
```

Run `nit --version` to identify your installed build and `nit update` to upgrade a release. If your shell cannot find nit after installation, add the directory printed by the installer to your PATH and open a new shell.

## Choose a workspace

| What you want to do | Where to start |
| --- | --- |
| Read unfamiliar code or review an agent’s edits | `nit .` or `nit path/to/file` |
| Try editing without a live model | `nit --agents local .` |
| Give Codex a read-only review | `nit --agents codex --codex-sandbox read-only .` |
| Examine how source text behaves in a cellular automaton | `nit gol path/to/file`, or `:gol` from the editor |
| Compare strategy scores in repeated games | `nit games` |

**About the experimental scores.** The Game of Life lab encodes source text as a grid and observes how the pattern evolves under a chosen rule. A survival time describes that simulation. To establish whether a change fixes a bug, use a reproducing test, a diff review and checks against the intended behavior. [Interpret the lab measurements](docs/SEEDS.md)

Press F1 in the editor for controls. The guides cover daily workflows, what agents can access, and how to interpret results. [Browse the documentation](docs/README.md) · [Tour the editor](https://nit.tools/#features) · [MIT license](LICENSE)
