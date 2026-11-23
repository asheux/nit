# Run a shell inside nit

Press **F4** to open a shell over your current work. Run a command, then press **F4** again to return to the editor. Open the popup again to see the same shell, its output, and any command still running.

| Where you want the shell | How to open it | How to return |
|---|---|---|
| Popup over the station or pane grid | `F4` or `Ctrl+Shift+T` | Use the same key, or press `Esc` twice. The first Escape reaches the shell program. |
| In place of Agent Chat | `Ctrl+\` | Click the chat tab or toggle back from the shell. |
| In one multipane pane | Click its **TERM** tab or press `Ctrl+\` in that pane | Click **NIT** or toggle back. Other panes keep their own sessions. |

Click inside a pane terminal to give it keyboard focus. You can leave a build running there while you edit elsewhere. Typing `exit` ends the shell. Quitting nit ends its embedded shell sessions. To launch several panes already running commands, see [Multipane](MULTIPANE.md).

## Copy output and paste commands

Drag across terminal output with the left mouse button and release to copy the selection to the system clipboard. Use `Ctrl+Shift+C` to copy it again. Scroll the wheel over a terminal to read up to 10,000 lines of history. Typing returns the view to live output.

Use your host terminal's paste shortcut, such as `Cmd+V` on macOS, or `Ctrl+Shift+V` when the host passes that shortcut to nit. Programs that enable bracketed paste receive pasted text as a block.

## Which shell runs?

On macOS and Linux, nit starts `$SHELL`, with `/bin/sh` as the fallback. On Windows it uses `%COMSPEC%`, with `cmd.exe` as the fallback. A new multipane shell starts in that pane's working directory. Changing directories in a running shell works as usual.
