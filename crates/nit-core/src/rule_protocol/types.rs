//! Type definitions split into `protocol` (driver state) and `refs`
//! (catalogue references). Kept separate so the driver's mutability and
//! advance logic don't leak into the value types serde sees.

mod protocol;
mod refs;

pub use protocol::{RuleMode, RuleProtocol};
pub use refs::{RulePhase, RuleRef};
