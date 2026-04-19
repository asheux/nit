//! UDS listener that terminates connections from the `nit-mcp-server` child
//! process. Each `BackchannelRequest` is translated into a mint-on-apply
//! `AgentBusEvent::*Request` and forwarded to the main loop's shared event
//! channel, where it drains alongside runner events.
//!
//! Unix-only in v1 — Windows callers can opt into a TCP fallback later; the
//! whole module is gated with `#[cfg(unix)]` so non-Unix builds keep compiling
//! with MCP disabled.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::mpsc::Sender;
use std::thread;

use nit_core::AgentBusEvent;
use nit_mcp::protocol::{BackchannelRequest, BackchannelResponse};

pub struct McpBackchannel {
    pub socket_path: String,
    _listener_handle: thread::JoinHandle<()>,
}

impl McpBackchannel {
    /// Bind a per-process UDS under `/tmp` and spawn an accept loop. Each
    /// incoming connection is handled on its own thread so a slow client cannot
    /// block new `tools/call` requests from arriving.
    pub fn spawn(event_tx: Sender<AgentBusEvent>) -> std::io::Result<Self> {
        let socket_path = format!("/tmp/nit-mcp-{}.sock", std::process::id());
        // Stale socket from a prior run with the same pid (possible on
        // wraparound) would cause `bind` to fail EADDRINUSE.
        let _ = std::fs::remove_file(&socket_path);
        let listener = UnixListener::bind(&socket_path)?;
        let path_for_return = socket_path.clone();
        let handle = thread::Builder::new()
            .name("nit-mcp-backchannel".into())
            .spawn(move || accept_loop(listener, event_tx))?;
        Ok(Self {
            socket_path: path_for_return,
            _listener_handle: handle,
        })
    }
}

impl Drop for McpBackchannel {
    fn drop(&mut self) {
        // Best-effort cleanup. The listener thread is not joined; it exits
        // naturally when the Sender half of the event channel drops.
        let _ = std::fs::remove_file(&self.socket_path);
    }
}

fn accept_loop(listener: UnixListener, event_tx: Sender<AgentBusEvent>) {
    for conn in listener.incoming() {
        let Ok(stream) = conn else {
            // Transient accept errors must not kill the listener.
            continue;
        };
        let tx = event_tx.clone();
        let _ = thread::Builder::new()
            .name("nit-mcp-conn".into())
            .spawn(move || handle_connection(stream, tx));
    }
}

fn handle_connection(stream: UnixStream, event_tx: Sender<AgentBusEvent>) {
    let Some((mut reader, mut writer)) = split_stream(stream) else {
        return;
    };
    let mut line = String::new();
    if reader.read_line(&mut line).is_err() {
        return;
    }
    let raw = line.trim();
    if raw.is_empty() {
        return;
    }
    let req: BackchannelRequest = match serde_json::from_str(raw) {
        Ok(r) => r,
        Err(err) => {
            let _ = write_response(
                &mut writer,
                &BackchannelResponse {
                    request_id: 0,
                    ok: false,
                    error: Some(format!("parse error: {err}")),
                },
            );
            return;
        }
    };
    let (request_id, event) = backchannel_to_event(req);
    let send_ok = event_tx.send(event).is_ok();
    let resp = BackchannelResponse {
        request_id,
        ok: send_ok,
        error: (!send_ok).then(|| "nit event channel closed".to_string()),
    };
    let _ = write_response(&mut writer, &resp);
}

// Clone the stream handle so the reader can own a `BufReader` while the writer
// keeps the original stream for the response. Returns `None` when the clone
// fails (rare; usually means the peer already closed the socket).
fn split_stream(stream: UnixStream) -> Option<(BufReader<UnixStream>, UnixStream)> {
    let reader_stream = stream.try_clone().ok()?;
    Some((BufReader::new(reader_stream), stream))
}

fn write_response<W: Write>(w: &mut W, resp: &BackchannelResponse) -> std::io::Result<()> {
    let line = serde_json::to_string(resp).unwrap_or_else(|_| "{}".into());
    writeln!(w, "{line}")?;
    w.flush()
}

pub(crate) fn backchannel_to_event(req: BackchannelRequest) -> (u64, AgentBusEvent) {
    match req {
        BackchannelRequest::EmitSignal {
            request_id,
            agent_id,
            kind,
            target,
            payload,
            strength,
        } => (
            request_id,
            AgentBusEvent::EmitSignalRequest {
                posted_by: agent_id,
                kind,
                target,
                payload,
                initial_strength: strength,
            },
        ),
        BackchannelRequest::AssertClaim {
            request_id,
            agent_id,
            kind,
            target,
            ttl_gens,
            rationale,
        } => (
            request_id,
            AgentBusEvent::AssertClaimRequest {
                claimed_by: agent_id,
                kind,
                target,
                ttl_gens,
                rationale,
            },
        ),
        BackchannelRequest::AssertAssumption {
            request_id,
            agent_id,
            target,
            fact,
            ttl_gens,
            rationale,
        } => (
            request_id,
            AgentBusEvent::AssertAssumptionRequest {
                posted_by: agent_id,
                target,
                fact,
                ttl_gens,
                rationale,
            },
        ),
    }
}

#[cfg(test)]
#[path = "tests/mcp_backchannel.rs"]
mod tests;
