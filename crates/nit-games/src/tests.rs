//! Test suite for the `nit-games` crate, organised by topic.
//!
//! Each per-topic file lives under `src/test_modules/`; this aggregator
//! pulls them in via `#[path]` so existing files like
//! `test_modules/fast_eval.rs` (paired with `fast_eval.rs`) keep their
//! source-aligned location.

#[path = "test_modules/shared.rs"]
mod shared;

#[path = "test_modules/metal.rs"]
mod metal;

#[path = "test_modules/strategies_ca.rs"]
mod strategies_ca;
#[path = "test_modules/strategies_fsm.rs"]
mod strategies_fsm;
#[path = "test_modules/strategies_tm.rs"]
mod strategies_tm;

#[path = "test_modules/tournament_aggregation.rs"]
mod tournament_aggregation;
#[path = "test_modules/tournament_halting.rs"]
mod tournament_halting;
#[path = "test_modules/tournament_metal.rs"]
mod tournament_metal;
#[path = "test_modules/tournament_progress.rs"]
mod tournament_progress;
#[path = "test_modules/tournament_reference.rs"]
mod tournament_reference;

#[path = "test_modules/config_parsing.rs"]
mod config_parsing;
