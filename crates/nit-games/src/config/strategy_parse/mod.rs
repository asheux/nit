//! Strategy specification parsing: FSM, CA, TM, and generated normalization.
//!
//! `fsm/` and `tm/` are split into per-encoding submodules (indexed/explicit
//! for FSM; explicit/table/wolfram for TM); the rest are flat single-file
//! parsers wired in here.

mod ca;
mod common;
mod fsm;
mod generated;
mod tm;

pub(super) use ca::normalize_ca_kind;
pub(super) use fsm::normalize_fsm_kind;
pub(super) use generated::load_generated_strategies;
pub(super) use tm::normalize_tm_kind;
