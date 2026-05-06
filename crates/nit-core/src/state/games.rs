//! `Games*` state cluster — request/response types, picker state,
//! family-run config previews, run history, replay/inspect popups.
//!
//! Stub: the ~360-line cluster (`GamesStatus`, `GamesAnalysisRequest`,
//! `GamesReplayRequest`, `GamesAnalysisState`, `GamesRunEntry`,
//! `GamesRunBrowserState`, `GamesReplayState`, `GamesStrategyInspectState`,
//! `GamesTmSimState`, `GamesCaSimState`, `GamesMatchHistoryState`,
//! `GamesRunOverride`, `FamilyRunBuildTimings`, `GamesFamilyRunRequest`,
//! `GamesConfigPreview`, `GamesState`, `open_games_history_popup`) still
//! lives in `state.rs`. Each type is `pub use`-re-exported via `lib.rs:80-83`,
//! so the move must keep that resolution intact. Deferred to a dedicated
//! turn; tracked in the shard's risks JSON.
