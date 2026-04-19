//! MCP stdio JSON-RPC 2.0 server.  Consumes NDJSON requests on stdin,
//! emits NDJSON responses on stdout.  Supports exactly three methods:
//! `initialize`, `tools/list`, `tools/call` — enough for Codex to discover
//! and invoke the three nit substrate tools.
//!
//! `tools/call` delegates to an injected [`Backchannel`] so tests can run
//! the server end-to-end without touching the real socket transport.

use std::io::{BufRead, Write};

use serde_json::{json, Value};

use crate::backchannel::Backchannel;
use crate::protocol::BackchannelRequest;

/// JSON-RPC error codes per spec.
const PARSE_ERROR: i64 = -32700;
const INVALID_REQUEST: i64 = -32600;
const METHOD_NOT_FOUND: i64 = -32601;
const INVALID_PARAMS: i64 = -32602;
const INTERNAL_ERROR: i64 = -32603;

/// Minimal JSON-RPC error — serialised into the `error` slot of a response.
#[derive(Debug)]
pub struct JsonRpcError {
    pub code: i64,
    pub message: String,
}

impl JsonRpcError {
    fn to_json(&self) -> Value {
        json!({ "code": self.code, "message": self.message })
    }
}

/// Entry point: read one JSON-RPC request per line, dispatch, write one
/// response per line.  Exits when stdin reaches EOF.  A single per-process
/// request counter keeps synthetic `request_id`s collision-free on the
/// back-channel wire.
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
        let response = handle_line(raw, bc, agent_id, &mut counter);
        // Avoid panicking the server on a closed stdout — the parent may have
        // exited mid-conversation; just return cleanly.
        if writeln!(writer, "{response}").is_err() {
            return Ok(());
        }
        let _ = writer.flush();
    }
}

/// Parse one NDJSON line and dispatch; returns the response as a JSON string.
/// Notifications (no `id`) still get a response written in the error path so
/// operators never see silent drops — production MCP clients ignore spurious
/// responses, and Codex in practice always sends an `id`.
pub fn handle_line<B: Backchannel>(raw: &str, bc: &B, agent_id: &str, counter: &mut u64) -> String {
    let value: Value = match serde_json::from_str(raw) {
        Ok(v) => v,
        Err(err) => {
            return error_response(
                Value::Null,
                JsonRpcError {
                    code: PARSE_ERROR,
                    message: format!("parse error: {err}"),
                },
            );
        }
    };
    let id = value.get("id").cloned().unwrap_or(Value::Null);
    let method = match value.get("method").and_then(|m| m.as_str()) {
        Some(m) => m.to_string(),
        None => {
            return error_response(
                id,
                JsonRpcError {
                    code: INVALID_REQUEST,
                    message: "missing method".into(),
                },
            );
        }
    };
    let params = value.get("params").cloned().unwrap_or(Value::Null);

    let result = match method.as_str() {
        "initialize" => handle_initialize(params),
        "initialized" | "notifications/initialized" => {
            // Pure notification — no response content needed, but we still
            // emit a minimal ack so line-oriented clients stay in lockstep.
            Ok(json!({}))
        }
        "tools/list" => handle_tools_list(),
        "tools/call" => handle_tools_call(params, bc, agent_id, counter),
        other => Err(JsonRpcError {
            code: METHOD_NOT_FOUND,
            message: format!("unknown method: {other}"),
        }),
    };
    match result {
        Ok(value) => success_response(id, value),
        Err(err) => error_response(id, err),
    }
}

fn success_response(id: Value, result: Value) -> String {
    json!({ "jsonrpc": "2.0", "id": id, "result": result }).to_string()
}

fn error_response(id: Value, err: JsonRpcError) -> String {
    json!({ "jsonrpc": "2.0", "id": id, "error": err.to_json() }).to_string()
}

fn handle_initialize(_params: Value) -> Result<Value, JsonRpcError> {
    Ok(json!({
        "protocolVersion": "2024-11-05",
        "capabilities": { "tools": {} },
        "serverInfo": {
            "name": "nit-mcp",
            "version": env!("CARGO_PKG_VERSION"),
        },
    }))
}

/// Tool schemas published to Codex.  Kept minimal — the substrate enums
/// themselves live in nit-core and round-trip through `serde_json` at the
/// handler boundary, so the JSON-Schema `target` / `kind` descriptions
/// are informational only.
fn handle_tools_list() -> Result<Value, JsonRpcError> {
    Ok(json!({
        "tools": [
            {
                "name": "emit_signal",
                "description": "Emit a stigmergic signal into nit's substrate. Visible to observers, arbiters, and other agents.",
                "inputSchema": {
                    "type": "object",
                    "required": ["kind", "target"],
                    "properties": {
                        "kind": {
                            "type": "string",
                            "enum": [
                                "warning", "lead", "deadend", "help_needed",
                                "claim_violation", "done_marker", "intervention_emitted",
                            ],
                            "description": "Signal kind — see nit-core SignalKind.",
                        },
                        "target": {
                            "type": "object",
                            "description": "SignalTarget: {\"kind\":\"file\",\"path\":...} | {\"kind\":\"agent\",\"agent_id\":...} | {\"kind\":\"global\"}.",
                        },
                        "payload": { "description": "Opaque JSON payload; schema is signal-kind specific." },
                        "strength": { "type": "number", "description": "Initial strength override; defaults to 1.0." },
                    },
                },
            },
            {
                "name": "assert_claim",
                "description": "Assert a claim over a substrate resource. Conflicts emit ClaimViolation signals; ttl is mood-adjusted at apply time.",
                "inputSchema": {
                    "type": "object",
                    "required": ["kind", "target", "ttl_gens", "rationale"],
                    "properties": {
                        "kind": {
                            "type": "string",
                            "enum": ["exclusive_write", "shared_read", "append_only", "soft"],
                        },
                        "target": { "type": "object" },
                        "ttl_gens": { "type": "integer", "minimum": 1 },
                        "rationale": { "type": "string" },
                    },
                },
            },
            {
                "name": "assert_assumption",
                "description": "Assert a read-time belief about a resource. Invalidated automatically on overlapping FileWrite.",
                "inputSchema": {
                    "type": "object",
                    "required": ["target", "ttl_gens", "rationale"],
                    "properties": {
                        "target": { "type": "object" },
                        "fact": { "description": "Opaque JSON fact the assumption asserts." },
                        "ttl_gens": { "type": "integer", "minimum": 1 },
                        "rationale": { "type": "string" },
                    },
                },
            },
        ],
    }))
}

fn handle_tools_call<B: Backchannel>(
    params: Value,
    bc: &B,
    agent_id: &str,
    counter: &mut u64,
) -> Result<Value, JsonRpcError> {
    let tool = params
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| JsonRpcError {
            code: INVALID_PARAMS,
            message: "missing tool name".into(),
        })?;
    let args = params.get("arguments").cloned().unwrap_or(Value::Null);
    *counter = counter.saturating_add(1);
    let req = build_backchannel_request(tool, *counter, agent_id, &args)?;
    let resp = bc.send(&req).map_err(|e| JsonRpcError {
        code: INTERNAL_ERROR,
        message: format!("back-channel error: {e}"),
    })?;
    if resp.ok {
        Ok(json!({
            "content": [{ "type": "text", "text": "ok" }],
        }))
    } else {
        Err(JsonRpcError {
            code: INTERNAL_ERROR,
            message: resp.error.unwrap_or_else(|| "unknown error".into()),
        })
    }
}

/// Translate an MCP `tools/call` argument bag into the typed `BackchannelRequest`
/// that travels over the UDS to nit-tui.  Extracted so tests can drive it
/// directly without routing through a whole `handle_line` cycle.
pub fn build_backchannel_request(
    tool: &str,
    request_id: u64,
    agent_id: &str,
    args: &Value,
) -> Result<BackchannelRequest, JsonRpcError> {
    match tool {
        "emit_signal" => {
            let kind = serde_json::from_value(args.get("kind").cloned().unwrap_or(Value::Null))
                .map_err(|e| JsonRpcError {
                    code: INVALID_PARAMS,
                    message: format!("invalid kind: {e}"),
                })?;
            let target = serde_json::from_value(args.get("target").cloned().unwrap_or(Value::Null))
                .map_err(|e| JsonRpcError {
                    code: INVALID_PARAMS,
                    message: format!("invalid target: {e}"),
                })?;
            let payload = args.get("payload").cloned().unwrap_or(Value::Null);
            let strength = args
                .get("strength")
                .and_then(|v| v.as_f64())
                .map(|f| f as f32);
            Ok(BackchannelRequest::EmitSignal {
                request_id,
                agent_id: agent_id.to_string(),
                kind,
                target,
                payload,
                strength,
            })
        }
        "assert_claim" => {
            let kind = serde_json::from_value(args.get("kind").cloned().unwrap_or(Value::Null))
                .map_err(|e| JsonRpcError {
                    code: INVALID_PARAMS,
                    message: format!("invalid kind: {e}"),
                })?;
            let target = serde_json::from_value(args.get("target").cloned().unwrap_or(Value::Null))
                .map_err(|e| JsonRpcError {
                    code: INVALID_PARAMS,
                    message: format!("invalid target: {e}"),
                })?;
            let ttl_gens = args.get("ttl_gens").and_then(|v| v.as_u64()).unwrap_or(3);
            let rationale = args
                .get("rationale")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            Ok(BackchannelRequest::AssertClaim {
                request_id,
                agent_id: agent_id.to_string(),
                kind,
                target,
                ttl_gens,
                rationale,
            })
        }
        "assert_assumption" => {
            let target = serde_json::from_value(args.get("target").cloned().unwrap_or(Value::Null))
                .map_err(|e| JsonRpcError {
                    code: INVALID_PARAMS,
                    message: format!("invalid target: {e}"),
                })?;
            let fact = args.get("fact").cloned().unwrap_or(Value::Null);
            let ttl_gens = args.get("ttl_gens").and_then(|v| v.as_u64()).unwrap_or(3);
            let rationale = args
                .get("rationale")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            Ok(BackchannelRequest::AssertAssumption {
                request_id,
                agent_id: agent_id.to_string(),
                target,
                fact,
                ttl_gens,
                rationale,
            })
        }
        other => Err(JsonRpcError {
            code: METHOD_NOT_FOUND,
            message: format!("unknown tool: {other}"),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::BackchannelResponse;
    use std::sync::Mutex;

    /// Mock that captures requests and replies with a canned response.
    /// Uses Mutex for interior mutability so the trait can stay `&self`.
    struct MockBackchannel {
        captured: Mutex<Vec<BackchannelRequest>>,
        reply_ok: bool,
    }

    impl MockBackchannel {
        fn new() -> Self {
            Self {
                captured: Mutex::new(Vec::new()),
                reply_ok: true,
            }
        }
    }

    impl Backchannel for MockBackchannel {
        fn send(&self, req: &BackchannelRequest) -> anyhow::Result<BackchannelResponse> {
            let request_id = match req {
                BackchannelRequest::EmitSignal { request_id, .. }
                | BackchannelRequest::AssertClaim { request_id, .. }
                | BackchannelRequest::AssertAssumption { request_id, .. } => *request_id,
            };
            self.captured.lock().unwrap().push(req.clone());
            Ok(BackchannelResponse {
                request_id,
                ok: self.reply_ok,
                error: None,
            })
        }
    }

    fn run_once(mock: &MockBackchannel, request: &str) -> Value {
        let mut counter = 0u64;
        let resp_line = handle_line(request, mock, "test-agent", &mut counter);
        serde_json::from_str(&resp_line).unwrap()
    }

    #[test]
    fn initialize_returns_capabilities() {
        let mock = MockBackchannel::new();
        let resp = run_once(
            &mock,
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#,
        );
        assert_eq!(resp["id"], 1);
        let result = &resp["result"];
        assert_eq!(result["protocolVersion"], "2024-11-05");
        assert!(result["capabilities"]["tools"].is_object());
        assert_eq!(result["serverInfo"]["name"], "nit-mcp");
    }

    #[test]
    fn tools_list_returns_three_tools() {
        let mock = MockBackchannel::new();
        let resp = run_once(
            &mock,
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}"#,
        );
        let tools = resp["result"]["tools"].as_array().expect("tools array");
        assert_eq!(tools.len(), 3);
        let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert!(names.contains(&"emit_signal"));
        assert!(names.contains(&"assert_claim"));
        assert!(names.contains(&"assert_assumption"));
    }

    #[test]
    fn tools_call_emit_signal_builds_backchannel_request() {
        let mock = MockBackchannel::new();
        let body = r#"{
            "jsonrpc":"2.0","id":3,"method":"tools/call","params":{
                "name":"emit_signal",
                "arguments":{
                    "kind":"warning",
                    "target":{"kind":"global"},
                    "payload":{"msg":"hello"},
                    "strength":0.8
                }
            }
        }"#;
        let resp = run_once(&mock, body);
        assert!(resp["error"].is_null(), "unexpected error: {resp}");
        let captured = mock.captured.lock().unwrap();
        assert_eq!(captured.len(), 1);
        match &captured[0] {
            BackchannelRequest::EmitSignal {
                agent_id,
                kind,
                target,
                payload,
                strength,
                ..
            } => {
                assert_eq!(agent_id, "test-agent");
                assert_eq!(*kind, nit_core::substrate::SignalKind::Warning);
                assert_eq!(*target, nit_core::substrate::SignalTarget::Global);
                assert_eq!(payload["msg"], "hello");
                assert!((strength.unwrap() - 0.8).abs() < f32::EPSILON * 10.0);
            }
            other => panic!("expected EmitSignal, got {other:?}"),
        }
    }

    #[test]
    fn tools_call_assert_claim_builds_request() {
        let mock = MockBackchannel::new();
        let body = r#"{
            "jsonrpc":"2.0","id":4,"method":"tools/call","params":{
                "name":"assert_claim",
                "arguments":{
                    "kind":"exclusive_write",
                    "target":{"kind":"file","path":"/tmp/foo"},
                    "ttl_gens":5,
                    "rationale":"integration mission"
                }
            }
        }"#;
        let resp = run_once(&mock, body);
        assert!(resp["error"].is_null(), "unexpected error: {resp}");
        let captured = mock.captured.lock().unwrap();
        match &captured[0] {
            BackchannelRequest::AssertClaim {
                agent_id,
                kind,
                ttl_gens,
                rationale,
                ..
            } => {
                assert_eq!(agent_id, "test-agent");
                assert_eq!(*kind, nit_core::substrate::ClaimKind::ExclusiveWrite);
                assert_eq!(*ttl_gens, 5);
                assert_eq!(rationale, "integration mission");
            }
            other => panic!("expected AssertClaim, got {other:?}"),
        }
    }

    #[test]
    fn tools_call_malformed_args_returns_invalid_params() {
        let mock = MockBackchannel::new();
        // `kind` is not a valid SignalKind variant.
        let body = r#"{
            "jsonrpc":"2.0","id":5,"method":"tools/call","params":{
                "name":"emit_signal",
                "arguments":{"kind":"nonsense","target":{"kind":"global"}}
            }
        }"#;
        let resp = run_once(&mock, body);
        assert_eq!(resp["error"]["code"], INVALID_PARAMS);
    }

    #[test]
    fn tools_call_unknown_tool_returns_method_not_found() {
        let mock = MockBackchannel::new();
        let body = r#"{
            "jsonrpc":"2.0","id":6,"method":"tools/call","params":{
                "name":"nope","arguments":{}
            }
        }"#;
        let resp = run_once(&mock, body);
        assert_eq!(resp["error"]["code"], METHOD_NOT_FOUND);
    }
}
