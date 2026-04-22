//! MCP stdio JSON-RPC 2.0 server. Consumes NDJSON requests on stdin, emits
//! NDJSON responses on stdout. Supports exactly three methods — `initialize`,
//! `tools/list`, `tools/call` — enough for Codex to discover and invoke the
//! nit substrate tools. `tools/call` delegates to an injected [`Backchannel`]
//! so tests can exercise the server end-to-end without real socket I/O.

use std::io::{BufRead, Write};

use serde::de::DeserializeOwned;
use serde_json::{json, Value};

use crate::backchannel::Backchannel;
use crate::protocol::BackchannelRequest;

// JSON-RPC 2.0 error codes.
const PARSE_ERROR: i64 = -32700;
const INVALID_REQUEST: i64 = -32600;
pub const METHOD_NOT_FOUND: i64 = -32601;
pub const INVALID_PARAMS: i64 = -32602;
const INTERNAL_ERROR: i64 = -32603;

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

/// Read one JSON-RPC request per line, dispatch, write one response per line.
/// Exits cleanly on stdin EOF or closed stdout. The per-process counter keeps
/// synthetic `request_id`s collision-free on the back-channel wire.
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
        // Parent may have exited mid-conversation; don't panic on closed stdout.
        if writeln!(writer, "{response}").is_err() {
            return Ok(());
        }
        let _ = writer.flush();
    }
}

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
        // Minimal ack keeps line-oriented clients in lockstep; real MCP
        // notifications carry no `id`, so clients tolerate a spurious reply.
        "initialized" | "notifications/initialized" => Ok(json!({})),
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

// Substrate enums round-trip through `serde_json` at the handler boundary,
// so the JSON-Schema `target` / `kind` descriptions are informational only.
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

/// Deserialise a required typed field from a `tools/call` argument bag,
/// mapping serde errors to `INVALID_PARAMS` with a field-tagged message.
fn parse_arg<T: DeserializeOwned>(args: &Value, field: &str) -> Result<T, JsonRpcError> {
    serde_json::from_value(args.get(field).cloned().unwrap_or(Value::Null)).map_err(|e| {
        JsonRpcError {
            code: INVALID_PARAMS,
            message: format!("invalid {field}: {e}"),
        }
    })
}

/// Translate a `tools/call` argument bag into a typed `BackchannelRequest`.
/// Extracted so tests can drive it directly without a full `handle_line` cycle.
pub fn build_backchannel_request(
    tool: &str,
    request_id: u64,
    agent_id: &str,
    args: &Value,
) -> Result<BackchannelRequest, JsonRpcError> {
    let agent_id = agent_id.to_string();
    let ttl_gens = || args.get("ttl_gens").and_then(|v| v.as_u64()).unwrap_or(3);
    let rationale = || {
        args.get("rationale")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string()
    };
    match tool {
        "emit_signal" => Ok(BackchannelRequest::EmitSignal {
            request_id,
            agent_id,
            kind: parse_arg(args, "kind")?,
            target: parse_arg(args, "target")?,
            payload: args.get("payload").cloned().unwrap_or(Value::Null),
            strength: args
                .get("strength")
                .and_then(|v| v.as_f64())
                .map(|f| f as f32),
        }),
        "assert_claim" => Ok(BackchannelRequest::AssertClaim {
            request_id,
            agent_id,
            kind: parse_arg(args, "kind")?,
            target: parse_arg(args, "target")?,
            ttl_gens: ttl_gens(),
            rationale: rationale(),
        }),
        "assert_assumption" => Ok(BackchannelRequest::AssertAssumption {
            request_id,
            agent_id,
            target: parse_arg(args, "target")?,
            fact: args.get("fact").cloned().unwrap_or(Value::Null),
            ttl_gens: ttl_gens(),
            rationale: rationale(),
        }),
        other => Err(JsonRpcError {
            code: METHOD_NOT_FOUND,
            message: format!("unknown tool: {other}"),
        }),
    }
}
