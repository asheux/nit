//! nit-mcp — bridges Codex subprocess agents into nit's substrate via MCP tools.
//!
//! Architecture:
//! - nit-tui spawns `codex mcp-server` with `-c mcp_servers.nit={...}` config.
//! - When Codex's model calls one of our tools (emit_signal, assert_claim,
//!   assert_assumption), Codex spawns `nit-mcp-server` as its child.
//! - nit-mcp-server speaks MCP stdio JSON-RPC with Codex, and forwards tool
//!   invocations over a UDS/TCP back-channel to nit-tui's listener thread.
//! - nit-tui's listener constructs `AgentBusEvent::*Request` and pushes it onto
//!   the shared mpsc; main thread mints IDs atomically during apply().

pub mod backchannel;
pub mod protocol;
pub mod server;

pub use backchannel::{Backchannel, BackchannelClient};
pub use protocol::{BackchannelRequest, BackchannelResponse};
