# Rule Catalog

nit's Game of Life uses Life-like rules: outer-totalistic, Moore
neighborhood, 2 states, written in B/S notation. `B3/S23` means a dead cell is
born with 3 neighbors and a live cell survives with 2 or 3. There are 2^18
possible Life-like rules. nit ships a tagged catalog of 28 and lets you layer
your own on top.

## Where the rules live

- Built-in catalog: `crates/nit-gol/assets/rules.toml`. 28 rules: classics, maze, no-death, texture, and literature rules.
- Your overlay (optional): `~/.config/nit/rules.toml`. It can add rules, override the description, tags, or aliases of built-ins, and mark rules `favorite = true` or `hidden = true`.

Overrides can be partial. A new rule needs at least `id`, `display_name`,
`rulestring`, and `description`.

```toml
[[rules]]
id = "my_rule"
display_name = "My Rule"
rulestring = "B3/S23"
description = "My custom Life-like rule."
tags = ["custom"]
aliases = ["mine"]

[[rules]]
id = "labyrinth"
aliases = ["maze"]
hidden = false
```

The rule picker also accepts a B/S string typed by hand, such as `B2/S` or
`B3678/S34678`. From the command prompt, `:gol rule <id|B/S>` sets a rule
and `:gol rules` lists them. See `docs/KEYBINDINGS.md`.

## Sources for new rules

Start with named, well-studied rules:

- Wikipedia, "Life-like cellular automaton": the main named rules with short descriptions.
- MCell / Mirek's Cellebration lexicon, and Eppstein's lists: more studied rules.
- Wolfram Demonstrations: 2D CA glider databases and related demos, for rules with known moving objects.
- Academic papers that list complexity-interesting rules. Cite the paper in `provenance` when you add one.

## Contribution checklist

- Use snake_case ids.
- Give a one-sentence description and short tags.
- Keep aliases short and searchable.
- Avoid duplicates: if the rulestring already exists, add an alias instead.
- If a rule contains B1, mark it hidden or warn prominently.

## Planned

- Random rule sampler (dev-only): generate random B/S rules, run a short simulation, compute quick metrics (growth rate, entropy-like score, stabilization, oscillation), and print a one-line summary to job output.
