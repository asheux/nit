//! Multi-phase GoL rule sequencing.
//!
//! A protocol drives the simulation through an ordered list of phases —
//! each phase pins one rule for a given number of generations. Optional
//! looping turns the sequence into a cycle so long-running simulations
//! can keep advancing past the last phase.

mod parse;
mod presets;
mod types;

pub use parse::parse_protocol_spec;
pub use presets::{builtin_protocols, ProtocolPreset};
pub use types::{RuleMode, RulePhase, RuleProtocol, RuleRef};

#[cfg(test)]
#[path = "tests/rule_protocol.rs"]
mod tests;
