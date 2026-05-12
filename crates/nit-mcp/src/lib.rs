//! MCP stdio JSON-RPC bridge. The `nit-mcp-server` binary speaks MCP with its
//! parent Codex process and forwards each `tools/call` over a back-channel
//! socket to the nit TUI so signals/claims/assumptions stay in one substrate.

pub mod backchannel;
pub mod jsonrpc;
pub mod protocol;
pub mod server;
pub(crate) mod tools;

pub use backchannel::Backchannel;
