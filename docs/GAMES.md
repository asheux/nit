# Run a strategy tournament

In the Games lab, two programs repeatedly choose to cooperate (`C`) or defect (`D`). You choose the opponents, payoffs, number of rounds and chance of an action flipping. The ranking tells you how each program performed under those conditions. Changing the opponents or their incentives can change the winner.

## Play a small tournament

Save the following as `games.toml` in an empty directory. It pits a strategy that always cooperates against one that always defects. Each strategy also plays itself.

```toml
schema_version = 1
game = "ipd"
rounds = 20
repetitions = 1
seed = 42
noise = 0.0
self_play = true
save_data = true

[payoff]
R = 3
S = 0
T = 5
P = 1

[history]
enabled = true

[[strategy]]
id = "always_c"
type = "fsm"
num_states = 1
k = 2
index = 1

[[strategy]]
id = "always_d"
type = "fsm"
num_states = 1
k = 2
index = 0
```

Open the file with `nit games games.toml`, then enter `:games run` in Normal mode. In the tournament popup, **Space** pauses or resumes and **Enter** steps one round while paused. **H** hides the popup while play continues. Use `:games show` to return.

The same tournament runs from a shell.

```sh
nit games run --config games.toml --out .
```

## Read the scores

The payoff table gives the points earned by the **row player** for one round. Both players earn 3 for mutual cooperation. A lone defector earns 5 while its opponent earns 0. Two defectors earn 1 each.

| Row player | Opponent cooperates | Opponent defects |
| --- | ---: | ---: |
| Cooperates | 3 (`R`) | 0 (`S`) |
| Defects | 5 (`T`) | 1 (`P`) |

The example runs four ordered matches of 20 rounds. Each strategy plays in both seats of its self-match and once in each seat against its opponent. The saved ranking gives `always_d` an average payoff of **3.0** and a total of **240**. For `always_c` the values are **1.5** and **120**. A win counts a match where the strategy outscored its opponent. It answers a different question from the average points earned per round.

The default `[engine]` setting `score_aggregation = "mean"` ranks by average payoff per round. In this mode the scoreboard’s aggregate payoff sums matchup means. Use `"total"` for cumulative points. If you enable complexity costs, compare adjusted scores to the raw payoffs to see the effect of the penalty.

## Compare conditions and strategies

Set `noise` to introduce action flips and `repetitions` to replay each matchup. Fix the seed to reproduce a tournament. A sweep runs one tournament per combination. The following sweep compares two lengths at two noise levels.

```sh
nit games sweep --config games.toml --out sweep --rounds 20,40 --noise 0,0.05
```

Inspect a strategy’s definition or export its transitions to see how it chooses an action.

```sh
nit games inspect --config games.toml --id always_c --format json
nit games graph --config games.toml --id always_c --out always_c.dot
```

A finite-state machine chooses its action from its current state and changes state in response to input. You can also enter a transition table directly, use a cellular automaton, or use a Turing machine with a per-round step limit. The bundled configuration includes examples of all three. [Example games.toml](../games.toml)

The following commands generate eight or fewer distinct finite-state strategies and add them to a tournament.

```sh
nit games enumerate fsm --states 2 --canonical --limit 8 --out generated.ndjson
nit games run --config games.toml --strategies generated.ndjson --out .
```

## Inspect a saved match

With the example’s output settings, each run gets a directory under `runs/games/`. Open `run_summary.json` for the seed, ranking, effective configuration and output paths. `history.ndjson` contains the round outcomes by match. In `score_idx`, the digits 0, 1, 2 and 3 represent CC, CD, DC and DD respectively, with player A’s action first. Use `match_id` to identify records when parallel runs write them out of order.

In the TUI, `:games runs` opens the run browser. Use `:games replay` to select a matchup from the loaded run and `:games history` to inspect its rounds. With a history log available, `:games analyze` writes per-match and per-strategy summaries, including cooperation rates. Set `save_data = false` for a normal tournament when you only need the result on screen. [Games controls](KEYBINDINGS.md)
