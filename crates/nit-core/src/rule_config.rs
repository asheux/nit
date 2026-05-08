//! Persistent storage for the user's GoL rule selection.
//!
//! Loading walks two TOML layers (global, then per-workspace override);
//! persisting writes back to whichever path the `workspace_override` flag
//! resolves to. Parse errors are surfaced as warnings so a corrupt config
//! file never blocks startup.

mod load;
mod parse_rules;
mod persist;
mod toml_io;

pub use load::{load_rule_config, RuleConfigLoad};
pub use persist::{persist_rule_selection, RulePersistence};

#[cfg(test)]
#[path = "tests/rule_config.rs"]
mod tests;
