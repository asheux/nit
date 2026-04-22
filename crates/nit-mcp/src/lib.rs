//! Bridges Codex subprocess agents into nit's substrate via MCP tools:
//! the `nit-mcp-server` binary speaks MCP stdio JSON-RPC with its parent
//! Codex process and forwards each `tools/call` over a UDS/TCP back-channel
//! to nit-tui's listener thread.

pub mod backchannel;
pub mod jsonrpc;
pub mod protocol;
pub mod server;
pub mod tools;

pub use backchannel::{Backchannel, BackchannelClient};
pub use protocol::{BackchannelRequest, BackchannelResponse};
