//! Back-channel client inside `nit-mcp-server`. Each `tools/call` opens a
//! fresh connection, writes one NDJSON request line, reads one NDJSON
//! response. The `Backchannel` trait exists so tests can inject a mock in
//! place of the real socket transport.

use std::io::{BufRead, BufReader, Write};
use std::time::Duration;

use crate::protocol::{BackchannelRequest, BackchannelResponse};

const IO_TIMEOUT: Duration = Duration::from_secs(2);

#[cfg(unix)]
type Stream = std::os::unix::net::UnixStream;
#[cfg(not(unix))]
type Stream = std::net::TcpStream;

pub trait Backchannel {
    fn send(&self, req: &BackchannelRequest) -> anyhow::Result<BackchannelResponse>;
}

pub struct BackchannelClient {
    /// Unix socket path, or decimal TCP port (validated to `u16` in [`Self::from_env`]).
    addr: String,
}

impl BackchannelClient {
    pub fn from_env() -> anyhow::Result<Self> {
        #[cfg(unix)]
        let addr = std::env::var("NIT_MCP_BACKCHANNEL_SOCKET")
            .map_err(|_| anyhow::anyhow!("NIT_MCP_BACKCHANNEL_SOCKET not set"))?;
        #[cfg(not(unix))]
        let addr = {
            let raw = std::env::var("NIT_MCP_BACKCHANNEL_PORT")
                .map_err(|_| anyhow::anyhow!("NIT_MCP_BACKCHANNEL_PORT not set"))?;
            let _: u16 = raw.parse()?;
            raw
        };
        Ok(Self { addr })
    }

    fn connect(&self) -> anyhow::Result<Stream> {
        #[cfg(unix)]
        let stream = Stream::connect(&self.addr)?;
        #[cfg(not(unix))]
        let stream = Stream::connect(("127.0.0.1", self.addr.parse::<u16>()?))?;
        stream.set_read_timeout(Some(IO_TIMEOUT))?;
        stream.set_write_timeout(Some(IO_TIMEOUT))?;
        Ok(stream)
    }
}

impl Backchannel for BackchannelClient {
    fn send(&self, req: &BackchannelRequest) -> anyhow::Result<BackchannelResponse> {
        let stream = self.connect()?;
        let line = serde_json::to_string(req)?;
        // Half-close the write side so the server's line read terminates:
        // writeln! on a cloned handle, flush, then drop it before reading.
        let mut write_half = stream.try_clone()?;
        writeln!(write_half, "{line}")?;
        write_half.flush()?;
        drop(write_half);
        let mut reader = BufReader::new(stream);
        let mut response = String::new();
        reader.read_line(&mut response)?;
        Ok(serde_json::from_str(response.trim())?)
    }
}
