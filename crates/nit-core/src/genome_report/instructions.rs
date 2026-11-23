//! Compact contract shared by genome-aware turns.

pub const GENOME_AGENT_INSTRUCTIONS: &str = "\
Aim for Tier III or better with the simplest correct solution. Keep small files minimal. \
Do not inflate scores with trivial helpers, duplicate bodies, gratuitous types, signature \
changes or comment padding. Preserve useful comments when investigating regressions. \
nit evaluates saved changes automatically; do not call evaluate_genome. \
NitSkills: for tier and parsimony rules read genome-quality; for encoder targets and \
regressions read genome-encoders. Use nit_skill(name) or run `nit skills NAME`.";
