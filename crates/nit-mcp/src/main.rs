use std::io::{self, BufReader};

use nit_mcp::backchannel::BackchannelClient;
use nit_mcp::server;

fn main() -> anyhow::Result<()> {
    let backchannel = BackchannelClient::from_env()?;
    let agent_id =
        std::env::var("NIT_MCP_AGENT_ID").unwrap_or_else(|_| "codex-session".to_string());
    let stdin = io::stdin();
    let stdout = io::stdout();
    server::run(
        BufReader::new(stdin.lock()),
        stdout.lock(),
        &backchannel,
        &agent_id,
    )
}
