# Rule Catalog

This project uses Life-like (outer-totalistic, Moore-neighborhood, 2-state) rules in B/S notation.
A rulestring like "B3/S23" means a dead cell is born with 3 neighbors, and a live cell survives
with 2 or 3 neighbors.

There are 2^18 possible Life-like rules, so we keep a small, tagged catalog of curated defaults and
let users layer their own rules on top.

## Where the built-ins live

- Built-in catalog: `crates/nit-gol/assets/rules.toml`
- User overlay (optional): `~/.config/nit/rules.toml`
  - Can add new rules or override description/tags/aliases of existing rules.
  - Can mark rules as `favorite = true` or `hidden = true`.

Overlay entries can be partial for overrides. For new rules, include at least:
`id`, `display_name`, `rulestring`, and `description`.

Example overlay:

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

## Curated sources for new rules

Start with named, well-studied rules before diving into random sampling.
Good sources:

- Wikipedia: "Life-like cellular automaton" (canonical named rules and short descriptions).
- MCell / Mirek's Cellebration lexicon, and Eppstein's lists for additional studied rules.
- Wolfram Demonstrations: 2D CA glider databases and related demos for rule strings with known
  moving objects.
- Academic papers that explicitly list complexity-interesting rules (cite the paper in
  `provenance` when you add one).

## Internal exploration workflow

1) Custom rule input
- The rule picker accepts manual B/S input (e.g. "B2/S" or "B3678/S34678").

2) Random rule sampler (dev-only)
- Add a dev-only command that:
  - generates random B/S rules (optionally constrained to B3* or avoiding B1),
  - runs a short simulation budget (N steps),
  - computes quick metrics (growth rate, entropy-ish score, stabilization, oscillation), and
  - prints a one-line summary to job output.
- This gives a repeatable pipeline for discovering new default rules later.

## Contribution checklist for new rules

- Use snake_case ids.
- Provide a 1-sentence description and short tags.
- Keep aliases short and searchable.
- Avoid duplicates: if a rulestring already exists, add an alias instead.
- If a rule contains B1, mark it hidden or warn prominently.
