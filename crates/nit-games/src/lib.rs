#![forbid(unsafe_code)]

pub mod config;
pub mod events;
pub mod game;
pub mod history;
pub mod history_log;
pub mod output;
pub mod strategy;
pub mod tournament;

pub use config::{ConfigError, GamesConfig, NormalizedConfig, StrategySpec};
pub use events::{EventLogConfig, EventWriter, GameEvent};
pub use game::{Action, Outcome, PayoffMatrix};
pub use history::{History, RoundRecord};
pub use history_log::{HistoryWriter, MatchHistory};
pub use output::{RunSummary, TournamentResults};
pub use strategy::{
    AlwaysCooperate, AlwaysDefect, FsmStrategy, GrimTrigger, MemoryStrategy, RandomStrategy,
    Strategy, StrategyKind, TitForTat, WinStayLoseShift,
};
pub use tournament::{MatchResult, TournamentProgress, TournamentRunner};

#[cfg(test)]
mod tests;
