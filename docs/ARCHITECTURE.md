# Work with an agent in the editor

Use the station when you want an agent to read or change a specific checkout and you want to inspect the evidence before accepting its answer.

## Ask a question you can verify

1. Open the checkout with `nit --agents codex .` or `nit --agents claude .`. Use a provider CLI that you have already authenticated. For a read-only Codex session, add `--codex-sandbox read-only`.
2. Select a model in **Agent Ops → Roster**, then press Enter. In **Agent Chat**, name the file, the decision you want explained and the evidence you want back. For example, ask “Read the session-expiry check in `auth.py`. Which test exercises an expired session? Cite the file and line.” Substitute files that exist in your project.
3. Read the answer in **Agent Chat**. Click **Artifacts** on the banner above a reply to open its full text, even after the agent answers another question. Use `Ctrl+F` to find the cited code. Press F4 to run its test in the workspace.
4. For follow-up work, state which files the agent may change and what behavior should hold afterward. Review the diff with `:changes`. The branch name and the two Git status columns help you see what is already staged.

## Find a result

| Question | Where to look |
| --- | --- |
| Which agent is running, and what did it say? | **Roster**, then **Agent Chat** or the full **REPLY** in **Artifacts** |
| What is blocking a team task? | **Missions** and **DAG**. A dependency is a task that must finish before another starts. |
| Did the requested check actually run? | **DAG** for the verification result and **Artifacts** for the gate report or command output |
| What changed on disk? | **Changes** for the Git patch, then Enter to read the file |
| Why did the provider turn fail? | **Diag** and **Alerts** for the provider error. With Codex MCP, the **MCP** tab also shows the connection state. |
| What guidance can the agent read? | **NitSkills** opens by default. Focus it with `:skills`, filter with `/`, then click **Use skill** or press `a` to add that reference to your draft. |

You can inspect a playbook yourself with `/skills NAME` in chat or `nit skills NAME` in a shell. The chat command displays the playbook locally. When you send a task, Codex and Claude receive a hint telling them where to read the built-in references on demand. Choosing **Use skill** names a particular playbook in your request. Include any requirement you need the agent to follow in the task itself.

A swarm can finish with no verification commands configured. Check which commands ran, their exit status and their coverage of your acceptance criterion before treating a mission as verified. [Swarm verification](SWARM.md)

## Choose how many conversations to start

- **One agent** suits a question with a clear owner. A prompt sent to a busy agent can queue for its next turn. Use `@queue` to make that intention explicit.
- **Multipane** suits independent questions. Give each conversation a working directory and a separate task. [Multipane guide](MULTIPANE.md)
- **A swarm** suits a single deliverable that needs independent proposals, implementation and a final check. Use `@swarm` in chat and inspect the plan. More agents can increase both provider usage and coordination time. [Swarm guide](SWARM.md)
- **Shadow review** lets hidden agents propose and critique an answer before the selected agent replies. [Shadow guide](SHADOWS.md)

## Providers and saved records

Codex uses its CLI. In the default `auto` mode nit selects MCP (Model Context Protocol) when the installed CLI provides the server command, and uses `codex exec` on versions that provide only the execution path. Claude uses its CLI and follows its account permissions. The local option is a mock for exploring the interface. A detected Gemini entry can appear in the roster, but this build has no Gemini turn runner.

Agent records may contain prompts, replies, paths, patches and command output. Mission records live under `.nit/agents/runs/` and swarm task output under `.nit/swarm/` in the workspace. Inspect those files before sharing the directory. [Security and local data](SECURITY.md)
