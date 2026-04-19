//! macOS Metal GPU backend. Each submodule owns a single concern; this file
//! only wires the public surface.

mod cache;
mod device;
mod dispatch;
mod policy;
mod shader;

pub(super) type MetalResult<T> = Result<T, String>;

pub use cache::*;
pub use device::*;
pub use dispatch::*;
pub use policy::*;
pub use shader::*;

#[cfg(test)]
#[path = "../tests/macos.rs"]
mod tests;
