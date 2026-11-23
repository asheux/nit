# Try a review from start to finish

Use a disposable Git checkout to see what nit does with an agent’s edits. The first exercise uses only Git and the local editor. The agent exercise uses your own authenticated provider.

## Read a staged and an unstaged change

Create a practice repository in an empty directory. These commands set an identity for the fixture commit only.

```sh
git init -q
printf 'def denied(code):\n    return code == 401\n' > audit.py
git add audit.py
git -c user.name=Reviewer -c user.email=review@example.invalid commit -qm baseline
printf 'def denied(code):\n    return code == 403\n' > audit.py
git add audit.py
printf 'def denied(code):\n    return code in (401, 403)\n' > audit.py
nit --agents local .
```

1. Run `:changes` and select `audit.py`. The **Staged** patch replaces 401 with 403. The **Unstaged** patch adds 401 back. Press Enter to open the version currently on disk.
2. Press F4 and run `git diff --cached -- audit.py`, then `git diff -- audit.py`. Compare the two outputs with what you saw in **Changes**. Press F4 to return.
3. Press `Ctrl+F`, search for `denied` and open the match. Press `i` to make a small edit, Esc and `:w` to save. Inspect the updated worktree patch. Use `u` to undo and save again.
4. Run `:skills`, press `/` and type `research`. Keep the results with Enter, choose a playbook and open it. Press Esc to return. Press `a` and check that its reference appears in your unsent Agent Chat draft. Return focus to the editor and press Esc. Run `:gol` to see the same file encoded for the simulation, then `:changes` to return to the diff.

## Ask an agent to explain the change

Reopen the same checkout with `nit --agents codex --codex-sandbox read-only .` or `nit --agents claude .`. Select a model in **Agent Ops**, press Enter and send this request.

> Read audit.py. Which status codes does denied() accept in the working file? Compare them with the staged version. Leave the files unchanged.

Open the **REPLY** card under **Artifacts**. Check that it names 401 and 403 in the working file, and only 403 in the staged file. A reply that quotes only one version has missed part of the question. If the provider declines the turn, inspect its message in **Diag**. [Agent station guide](ARCHITECTURE.md)

## Check your own workflow

| Exercise | Observe |
| --- | --- |
| Leave an edit unsaved, then change the same file in another terminal | The buffer retains your edit and reports the external change. |
| Open two files with unsaved edits and run `:wq` on one | The other dirty buffer remains available. |
| On Unix, save a private executable through a symlink | The link still points to the target, the target contains your edit and mode 0700 is preserved. |
| Switch between the editor, inline terminal and F4 popup | Typing and pasting reach the focused pane. A shell command can continue while its popup is closed. |
| Run a swarm with a custom test command in a disposable project | Inspect the command, exit result and final report in **Artifacts**. Compare with running the same command yourself. |
| Open `nit games` in a practice directory and run its example tournament | Open a completed match and compare its rounds with the final score. |

For help with any control, press F1. [Key reference](KEYBINDINGS.md) · [Verification commands](SWARM.md) · [Game tournaments](GAMES.md)
