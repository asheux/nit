//! `nit-mcp-server` binary — spawned as a child of `codex mcp-server` when
//! the Codex model invokes one of the nit MCP tools.  Reads a back-channel
//! socket path + agent id from env, then loops on stdin.

use std::io::{self, BufReader};

use nit_mcp::backchannel::BackchannelClient;
use nit_mcp::server;

fn main() -> anyhow::Result<()> {
    let bc = BackchannelClient::from_env()?;
    let agent_id =
        std::env::var("NIT_MCP_AGENT_ID").unwrap_or_else(|_| "codex-session".to_string());
    let stdin = io::stdin();
    let stdout = io::stdout();
    let reader = BufReader::new(stdin.lock());
    let writer = stdout.lock();
    server::run(reader, writer, &bc, &agent_id)
}
