//! JSON-RPC 2.0 framing: request parsing, dispatch, and response encoding.
//! Tool semantics live in [`crate::tools`] — this module cares only about
//! lifting a wire line into a typed request and lowering a typed outcome
//! back onto the wire.

use serde_json::{json, Value};

use crate::backchannel::Backchannel;
use crate::tools::{handle_initialize, handle_tools_call, handle_tools_list};

const PARSE_ERROR: i64 = -32700;
const INVALID_REQUEST: i64 = -32600;
pub const METHOD_NOT_FOUND: i64 = -32601;
pub const INVALID_PARAMS: i64 = -32602;
pub const INTERNAL_ERROR: i64 = -32603;

#[derive(Debug)]
pub struct JsonRpcError {
    pub code: i64,
    pub message: String,
}

/// Parse one JSON-RPC request line, dispatch, and return the wire-encoded
/// reply. `counter` is bumped once per `tools/call` to keep synthetic
/// `request_id`s collision-free on the back-channel wire.
pub fn handle_line<B: Backchannel>(raw: &str, bc: &B, agent_id: &str, counter: &mut u64) -> String {
    let value: Value = match serde_json::from_str(raw) {
        Ok(v) => v,
        Err(err) => {
            return encode_response(
                Value::Null,
                Err(JsonRpcError {
                    code: PARSE_ERROR,
                    message: format!("parse error: {err}"),
                }),
            );
        }
    };
    let id = value.get("id").cloned().unwrap_or(Value::Null);
    let Some(method) = value.get("method").and_then(|m| m.as_str()) else {
        return encode_response(
            id,
            Err(JsonRpcError {
                code: INVALID_REQUEST,
                message: "missing method".into(),
            }),
        );
    };
    let params = value.get("params").cloned().unwrap_or(Value::Null);
    let outcome = match method {
        "initialize" => handle_initialize(params),
        // Real MCP notifications carry no `id`, so clients tolerate the
        // spurious reply; emitting one keeps line-oriented test clients in lockstep.
        "initialized" | "notifications/initialized" => Ok(json!({})),
        "tools/list" => handle_tools_list(),
        "tools/call" => handle_tools_call(params, bc, agent_id, counter),
        other => Err(JsonRpcError {
            code: METHOD_NOT_FOUND,
            message: format!("unknown method: {other}"),
        }),
    };
    encode_response(id, outcome)
}

fn encode_response(id: Value, outcome: Result<Value, JsonRpcError>) -> String {
    match outcome {
        Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }).to_string(),
        Err(err) => json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": { "code": err.code, "message": err.message },
        })
        .to_string(),
    }
}
