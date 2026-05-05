//! Metal GPU acceleration for game-theory tournament batch evaluation.
//!
//! On non-macOS platforms every public function is a no-op stub so the
//! workspace compiles unconditionally. The authoritative public surface is
//! the `pub use` list in [`macos`] (macOS) or [`stubs`] (elsewhere).

mod types;
pub use types::*;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
pub use macos::*;

#[cfg(not(target_os = "macos"))]
mod stubs;
#[cfg(not(target_os = "macos"))]
pub use stubs::*;
