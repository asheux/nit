//! Back-channel client — the side of the UDS (Unix domain socket) connection
//! living inside the `nit-mcp-server` binary.  Each `tools/call` opens a fresh
//! connection, writes one NDJSON request line, and reads one NDJSON response.
//!
//! The `Backchannel` trait exists so tests can inject a mock in place of the
//! real socket transport.

use std::io::{BufRead, BufReader, Write};
use std::time::Duration;

use crate::protocol::{BackchannelRequest, BackchannelResponse};

/// Transport abstraction for forwarding tool calls to nit-tui.  The real
/// implementation round-trips over a Unix socket (or TCP on non-Unix); tests
/// supply a mock that captures requests without any I/O.
pub trait Backchannel {
    fn send(&self, req: &BackchannelRequest) -> anyhow::Result<BackchannelResponse>;
}

pub struct BackchannelClient {
    #[cfg(unix)]
    socket_path: String,
    #[cfg(not(unix))]
    tcp_port: u16,
}

impl BackchannelClient {
    pub fn from_env() -> anyhow::Result<Self> {
        #[cfg(unix)]
        {
            let path = std::env::var("NIT_MCP_BACKCHANNEL_SOCKET")
                .map_err(|_| anyhow::anyhow!("NIT_MCP_BACKCHANNEL_SOCKET not set"))?;
            Ok(Self { socket_path: path })
        }
        #[cfg(not(unix))]
        {
            let port: u16 = std::env::var("NIT_MCP_BACKCHANNEL_PORT")
                .map_err(|_| anyhow::anyhow!("NIT_MCP_BACKCHANNEL_PORT not set"))?
                .parse()?;
            Ok(Self { tcp_port: port })
        }
    }
}

impl Backchannel for BackchannelClient {
    fn send(&self, req: &BackchannelRequest) -> anyhow::Result<BackchannelResponse> {
        #[cfg(unix)]
        let stream = {
            use std::os::unix::net::UnixStream;
            let s = UnixStream::connect(&self.socket_path)?;
            s.set_read_timeout(Some(Duration::from_secs(2)))?;
            s.set_write_timeout(Some(Duration::from_secs(2)))?;
            s
        };
        #[cfg(not(unix))]
        let stream = {
            use std::net::TcpStream;
            let s = TcpStream::connect(("127.0.0.1", self.tcp_port))?;
            s.set_read_timeout(Some(Duration::from_secs(2)))?;
            s.set_write_timeout(Some(Duration::from_secs(2)))?;
            s
        };
        let line = serde_json::to_string(req)?;
        // Writing and reading use two halves of the same stream; keep a
        // mutable handle for the write then wrap the read side in a BufReader.
        let mut write_half = stream.try_clone()?;
        writeln!(write_half, "{}", line)?;
        write_half.flush()?;
        drop(write_half);
        let mut reader = BufReader::new(stream);
        let mut response = String::new();
        reader.read_line(&mut response)?;
        let resp: BackchannelResponse = serde_json::from_str(response.trim())?;
        Ok(resp)
    }
}
