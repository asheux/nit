mod accumulator;
mod match_state;
mod progress;
mod runtime;

pub use match_state::MatchResult;
pub use progress::{MatchHistoryPreview, MatchSnapshot, TournamentProgress};
pub use runtime::{Parallelism, TmHaltingFilterBackend, TmHaltingFilterDiagnostics};

pub(crate) use match_state::{MatchOutcome, Matchup};
pub(crate) use runtime::PreparedMetalBatch;

pub(super) use accumulator::{PairStats, StrategyStats, TournamentAccumulator};
pub(super) use match_state::{MatchRole, MatchSession, RoundOutcome, RoundSnapshot};
pub(super) use runtime::{run_with_parallelism, MetalBatchState, SeedDeriver};
