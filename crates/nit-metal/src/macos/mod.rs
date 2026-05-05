//! macOS Metal GPU backend.

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
