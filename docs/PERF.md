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

## How much text do NitSkills save?

Nit puts required instructions in each request and lets agents fetch detailed references when needed. We counted four representative prompt bodies from the initial six-playbook rollout at `3b82579` against the earlier implementation at `69657d0`, using the `o200k_base` tokenizer. The NitSkills counts include the Codex hint pointing to the local skills directory. The catalog later expanded to 16 playbooks. These figures describe the original measurement.

| Prompt | Earlier input tokens | With NitSkills |
| --- | ---: | ---: |
| Plain turn | 210 | 117 |
| Swarm proposer | 3,249 | 1,349 |
| Swarm judge | 3,527 | 1,605 |
| Swarm integrator | 4,637 | 1,787 |

One of each totals 11,623 and 4,858 estimated input tokens respectively, a 58.2% reduction in this sample. A real mission can have different numbers of each role. Provider framing, tool definitions, replies, later skill reads and provider caching affect usage and billing. For a cost comparison, keep the model, task and verification requirements fixed and compare provider-reported usage across completed runs. [How to use NitSkills](../README.md#give-agents-only-the-guidance-they-need)

## Compare game throughput

From a source checkout, `cargo bench -p nit-games` runs the tournament benchmarks. To compare settings, keep the strategy list, seed, number of matches, and rounds fixed. Record whether history, event logging and acceleration are enabled. Check that scores agree before interpreting elapsed time. [Tournament guide](GAMES.md)
