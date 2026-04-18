//! Listener thread that terminates the UDS connection from the
//! `nit-mcp-server` child process.  Converts each `BackchannelRequest`
//! into a mint-on-apply `AgentBusEvent::*Request` and pushes it onto
//! the shared event channel that the main loop drains with the other
//! runner events.
//!
//! Unix-only in v1 — Windows callers can opt-in later via a TCP
//! fallback; the whole module is gated with `#[cfg(unix)]` so
//! non-Unix builds keep compiling with MCP disabled.

#![cfg(unix)]

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
    /// Bind a per-process UDS under `/tmp` and spawn an accept loop.  Each
    /// incoming connection is handled on its own thread so slow clients
    /// can't block new `tools/call` requests from arriving.
    pub fn spawn(event_tx: Sender<AgentBusEvent>) -> std::io::Result<Self> {
        let pid = std::process::id();
        let socket_path = format!("/tmp/nit-mcp-{pid}.sock");
        // Clean up a stale socket from a previous run with the same pid
        // wraparound — rare, but the bind would otherwise fail with EADDRINUSE.
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
        // Best-effort cleanup; if the file is already gone, the error is
        // harmless.  We don't join the listener thread — it'll fall out
        // when the Sender half of the event channel is dropped.
        let _ = std::fs::remove_file(&self.socket_path);
    }
}

fn accept_loop(listener: UnixListener, event_tx: Sender<AgentBusEvent>) {
    for conn in listener.incoming() {
        match conn {
            Ok(stream) => {
                let tx = event_tx.clone();
                let _ = thread::Builder::new()
                    .name("nit-mcp-conn".into())
                    .spawn(move || handle_connection(stream, tx));
            }
            Err(_) => {
                // Transient accept errors shouldn't kill the listener;
                // just keep looping.
                continue;
            }
        }
    }
}

fn handle_connection(stream: UnixStream, event_tx: Sender<AgentBusEvent>) {
    // Split the stream so we can read the request line before echoing
    // the response back on the same socket.
    let reader_stream = match stream.try_clone() {
        Ok(s) => s,
        Err(_) => return,
    };
    let mut reader = BufReader::new(reader_stream);
    let mut writer = stream;
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
            let resp = BackchannelResponse {
                request_id: 0,
                ok: false,
                error: Some(format!("parse error: {err}")),
            };
            let _ = write_response(&mut writer, &resp);
            return;
        }
    };
    let (request_id, event) = backchannel_to_event(req);
    let send_ok = event_tx.send(event).is_ok();
    let resp = BackchannelResponse {
        request_id,
        ok: send_ok,
        error: if send_ok {
            None
        } else {
            Some("nit event channel closed".into())
        },
    };
    let _ = write_response(&mut writer, &resp);
}

fn write_response<W: Write>(w: &mut W, resp: &BackchannelResponse) -> std::io::Result<()> {
    let line = serde_json::to_string(resp).unwrap_or_else(|_| "{}".into());
    writeln!(w, "{}", line)?;
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
mod tests {
    use super::*;
    use std::sync::mpsc;

    #[test]
    fn backchannel_to_event_maps_emit_signal() {
        let req = BackchannelRequest::EmitSignal {
            request_id: 42,
            agent_id: "agent-x".into(),
            kind: nit_core::substrate::SignalKind::Warning,
            target: nit_core::substrate::SignalTarget::Global,
            payload: serde_json::json!({"k":"v"}),
            strength: Some(0.7),
        };
        let (id, event) = backchannel_to_event(req);
        assert_eq!(id, 42);
        match event {
            AgentBusEvent::EmitSignalRequest {
                posted_by,
                initial_strength,
                ..
            } => {
                assert_eq!(posted_by, "agent-x");
                assert!((initial_strength.unwrap() - 0.7).abs() < f32::EPSILON * 10.0);
            }
            other => panic!("expected EmitSignalRequest, got {other:?}"),
        }
    }

    #[test]
    fn backchannel_to_event_maps_assert_claim() {
        let req = BackchannelRequest::AssertClaim {
            request_id: 9,
            agent_id: "agent-y".into(),
            kind: nit_core::substrate::ClaimKind::Soft,
            target: nit_core::substrate::ClaimTarget::Global,
            ttl_gens: 2,
            rationale: "r".into(),
        };
        let (id, event) = backchannel_to_event(req);
        assert_eq!(id, 9);
        assert!(matches!(event, AgentBusEvent::AssertClaimRequest { .. }));
    }

    #[test]
    fn spawn_and_drop_removes_socket() {
        let (tx, _rx) = mpsc::channel();
        let bc = McpBackchannel::spawn(tx).expect("spawn");
        let path = bc.socket_path.clone();
        assert!(std::path::Path::new(&path).exists());
        drop(bc);
        // Give the listener a moment to unblock; Drop cleans up the file.
        std::thread::sleep(std::time::Duration::from_millis(50));
        assert!(!std::path::Path::new(&path).exists());
    }
}
