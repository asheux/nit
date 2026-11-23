# nit

nit is a terminal editor for working on a codebase with coding agents. Read the source, ask an agent about a specific decision, inspect its Git diff and run the check that matters to you, all in the same workspace.

## Start with a change

From a Git checkout, run `nit .`. The file tree opens beside a **Changes** pane. You can try this workflow with an edit of your own or one an agent has just made.

1. **Find the decision.** Press `Ctrl+F`, search for a function name or error message, select a match and press Enter. Use `Ctrl+P` when you know the filename.
2. **Edit the source.** Press `i` to type, Esc to return to Normal mode, then `:w` and Enter to save.
3. **Read what changed.** Run `:changes`. Use ↑/↓ to pick a file and PageUp/PageDown to read the patch. A file can have both staged and unstaged edits. Enter opens the selected file.
4. **Check the behavior.** Press F4 and run the repository’s relevant test in the embedded shell. F4 returns to the editor and keeps the shell session alive.

### Bring in an agent

Install and sign in to the Codex or Claude CLI you want to use. Launch `nit --agents codex .` or `nit --agents claude .`, select a model in **Agent Ops** and press Enter to open **Agent Chat**. Give the agent a bounded question, such as “Read the permission check in this file. Which test covers a denied request? Cite the file and line.” Include the actual filename in your prompt. Open the **REPLY** in **Artifacts**, follow the cited code and run the test yourself.

For independent reviews, `nit multipane --panes 4` opens four conversations. For a change with dependencies, a swarm can assign tasks, pass their results to another agent and collect verification evidence. Give it an acceptance criterion you can check. [Work with a team of agents](docs/SWARM.md) · [Use independent panes](docs/MULTIPANE.md)

### Give agents only the guidance they need

NitSkills are six short references covering code-structure feedback, swarm roles and verification. Agents can read the relevant playbook on demand. The prompt still carries required constraints and output formats. Use `:skills` to browse the catalog in **Code Structural Quality**, `/skills` in Agent Chat, or `nit skills swarm-judge` from your shell. [Measured effect on prompt size](docs/PERF.md)

## Install

The following commands install a published release. To use the features in this checkout, build from source.

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

Press F1 in the editor for controls. The guides cover daily workflows, what agents can access, and how to interpret results. [Browse the documentation](docs/README.md) · [Watch the demo](https://nit.tools/nit-demo.mp4) · [MIT license](LICENSE)
