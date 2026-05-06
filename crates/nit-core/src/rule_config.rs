mod load;
mod persist;
mod toml_io;

pub use load::{load_rule_config, RuleConfigLoad};
pub use persist::{persist_rule_selection, RulePersistence};

#[cfg(test)]
#[path = "tests/rule_config.rs"]
mod tests;
