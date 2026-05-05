mod eval;
mod strategy;

pub use eval::{decode_ca_rule_table, run_shrinking_ca, CaRunResult};
pub use strategy::CaStrategy;
