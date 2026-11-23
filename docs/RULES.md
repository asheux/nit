# Choose a Game of Life rule

A rule determines what happens to each cell in the next generation. nit counts the eight neighboring cells, including diagonals. In **B3/S23**, a dead cell with exactly three live neighbors becomes alive (**B** for birth). A live cell survives with two or three live neighbors (**S** for survival). Every cell updates from the same generation.

## Try the same seed under two rules

1. Open a source file with `nit gol path/to/file.rs`. Enter `:gol rule conway`, then `:gol run` to open the Petri Dish.
2. Press **Space** to pause and **Enter** to step. Watch what happens to the starting pattern.
3. In the Petri Dish, press **F2** to open the rule picker. Type `highlife`, select the entry and press **Enter**. Reseed with **Ctrl+R** and step again. HighLife adds birth with six live neighbors, so the two rules can diverge when that neighborhood occurs.

You can also enter a rule directly from Normal mode, for example `:gol rule B2/S`. Its survival list is empty, so every currently live cell dies on the next step. Births can still occur. Enter `:gol rules` to see available choices. The built-in catalog contains 28 named rules. [Built-in rules and descriptions](../crates/nit-gol/assets/rules.toml)

## Keep a workspace default

To make a project start with HighLife, set its default in the project’s `.nit/config.toml`.

```toml
[gol.rule]
default = "highlife"
```

## Add a rule to the picker

Create `rules.toml` in nit’s user configuration directory. On Linux the usual path is `~/.config/nit/rules.toml`. On macOS it is `~/Library/Application Support/dev.arcxlab.nit/rules.toml`. Restart nit after editing the file.

```toml
[[rules]]
id = "birth_four"
display_name = "Birth on four"
rulestring = "B4/S23"
description = "Cells are born with four neighbors and survive with two or three."
tags = ["custom"]
favorite = true
```

Select the new rule with `:gol rule birth_four`. To customize an existing rule, reuse its `id` and supply just the fields you want to set, such as `favorite = true` or `hidden = true`. For an additional name for an existing B/S combination, set the existing entry’s `aliases` list. Keep any aliases you still want to use when replacing that list. [Seed guide](SEEDS.md) · [Petri Dish controls](KEYBINDINGS.md)
