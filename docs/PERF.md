# Editor performance and prompt size

Use these measurements to decide what to test on your own terminal and with your own agent workload.

## How quickly does an edit appear?

| Measurement | Earlier build | Revised build |
| --- | ---: | ---: |
| One key to visible edit, median | 23.6 ms | 13.2 ms |
| Three keys to visible edit, median | 39.4 ms | 13.2 ms |
| Idle editor, percent of one CPU core | 8.2% | 1.8% |
| Eight idle local chat panes, percent of one CPU core | 14.7% | 0.3% |

**Test conditions.** The two Git revisions were `69657d0` and `ab9ab11`. Both were Rust debug builds on the same Apple M4 Max with 36 GiB RAM, macOS 26.5.2 and tmux 3.6a. Each ran in a separate 160×48 session. The editor opened a tracked UTF-8 file of 68,011 bytes with 2,001 nonempty lines.

**How to repeat the comparison.** Allow startup to settle for at least 22 seconds. Send a key through tmux and measure elapsed time until a pane capture contains the expected edit. Repeat 12 times, use the median, and verify saved file contents. Repeat with three-character bursts. For idle CPU, divide the increase in process CPU time by elapsed wall time over eight seconds for the editor or six seconds for multipane, after 35 seconds of multipane warm-up. One fully occupied core corresponds to 100%.

These are local observations. The latency includes tmux input and capture overhead. Terminal size, workload, build profile and hardware matter. On a slow remote terminal, try `NIT_TUI_FPS=30 nit .` and compare scrolling and idle CPU at the same dimensions. The default frame cap is 60 frames per second. [Configuration](ENVIRONMENT.md)

## How quickly does the first file get syntax colors?

Nit loads the highlighting rules for a language when you first open it. In a release-build comparison on Apple silicon, the first highlighted Rust snapshot took a median of **7.2 ms** for the whole file and **4.6 ms** for the visible region. Loading the language rules together at startup had taken **588.6 ms** and **580.4 ms** on the same file.

The sample was `crates/nit-core/src/buffer/cursor_motion.rs` at `7715252` (584 lines, 19,149 bytes). The comparison used that revision with and without the change in `67b982e`. After one warm-up per build and mode, nine fresh processes per condition ran in alternating order. Timing started after the file was read, as the syntax manager was created, and ended when a snapshot contained a keyword on a visible line. Terminal startup, file reads and drawing the frame take additional time.

### When do the colors actually appear in the terminal?

On small Rust and Python files, the editor could have the colors ready and still wait for its next idle redraw. The following release-build measurements start at the keystroke and end at the first tmux capture with the expected token color. Each build ran in 20 fresh local-agent sessions on the same Mac, split evenly between the two languages and two ways of opening a file.

| Action | Before immediate redraw | With immediate redraw |
| --- | ---: | ---: |
| Open with `:e`, median | 266 ms | 25 ms |
| Open from NITTree, median | 275 ms | 40 ms |
| Type a character and see its syntax color, median | 264 ms | 77 ms |

On a generated 12.9 MB Rust file, an in-editor open also had to wait for the whole file to parse. With immediate redraw alone, the first screen took about 1.46 seconds to show colors in one run. After adding a preview of the opening lines, that same screen showed colors in 54, 54 and 55 ms across three opens. The full parse continues in the background. This measurement covers opening at the top of the file.

## How much text do NitSkills save?

Nit puts required instructions in each request and lets agents fetch detailed references when needed. We counted four representative prompt bodies from the initial six-playbook rollout at `3b82579` against the earlier implementation at `69657d0`, using the `o200k_base` tokenizer. The NitSkills counts include the Codex hint pointing to the local skills directory. The catalog later expanded to 16 playbooks. These figures describe the original measurement.

| Prompt | Earlier input tokens | With NitSkills |
| --- | ---: | ---: |
| Plain turn | 210 | 117 |
| Swarm proposer | 3,249 | 1,349 |
| Swarm judge | 3,527 | 1,605 |
| Swarm integrator | 4,637 | 1,787 |

One of each totals 11,623 and 4,858 estimated input tokens respectively, a 58.2% reduction in this sample. A real mission can have different numbers of each role. Provider framing, tool definitions, replies, later skill reads and provider caching affect usage and billing. For a cost comparison, keep the model, task and verification requirements fixed and compare provider-reported usage across completed runs. [How to use NitSkills](../README.md#give-agents-only-the-guidance-they-need)

For the 16-playbook catalog at `5c97c1c`, the `nit skills` listing is 247 tokens with the same tokenizer. A single playbook ranges from 188 to 520 tokens, with a median of 266. These counts cover the listing or the Markdown body an agent chooses to read. Attaching a skill in chat inserts a reference, so the agent can decide whether to fetch the full text.

## Compare game throughput

From a source checkout, `cargo bench -p nit-games` runs the tournament benchmarks. To compare settings, keep the strategy list, seed, number of matches, and rounds fixed. Record whether history, event logging and acceleration are enabled. Check that scores agree before interpreting elapsed time. [Tournament guide](GAMES.md)
