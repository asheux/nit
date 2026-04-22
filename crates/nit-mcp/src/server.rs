//! MCP stdio server loop. Consumes NDJSON JSON-RPC 2.0 requests on stdin,
//! emits NDJSON replies on stdout until EOF or a closed pipe. Framing and
//! tool semantics live in [`crate::jsonrpc`] and [`crate::tools`]; this
//! module stays thin so the binary entry point in `main.rs` is trivial.

use std::io::{BufRead, Write};

use crate::backchannel::Backchannel;
use crate::jsonrpc;

// Re-exports keep integration tests reaching through a single module path
// (`nit_mcp::server::...`) even though implementations moved out.
pub use crate::jsonrpc::{handle_line, JsonRpcError, INVALID_PARAMS, METHOD_NOT_FOUND};
pub use crate::tools::build_backchannel_request;

pub fn run<R: BufRead, W: Write, B: Backchannel>(
    mut reader: R,
    mut writer: W,
    bc: &B,
    agent_id: &str,
) -> anyhow::Result<()> {
    let mut counter: u64 = 0;
    let mut line = String::new();
    loop {
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) => return Ok(()),
            Ok(_) => {}
            Err(err) => return Err(err.into()),
        }
        let raw = line.trim();
        if raw.is_empty() {
            continue;
        }
        let response = jsonrpc::handle_line(raw, bc, agent_id, &mut counter);
        // Parent may have exited mid-conversation; don't panic on closed stdout.
        if writeln!(writer, "{response}").is_err() {
            return Ok(());
        }
        let _ = writer.flush();
    }
}
