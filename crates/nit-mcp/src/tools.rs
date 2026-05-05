//! MCP tool semantics: initialize handshake, tools/list catalog, and the
//! `tools/call` path that lifts argument bags into typed [`BackchannelRequest`]s
//! and forwards them across the [`Backchannel`] trait.

use std::sync::LazyLock;

use serde::de::DeserializeOwned;
use serde_json::{json, Value};

use crate::backchannel::Backchannel;
use crate::jsonrpc::{JsonRpcError, INTERNAL_ERROR, INVALID_PARAMS, METHOD_NOT_FOUND};
use crate::protocol::BackchannelRequest;

// Substrate enums round-trip through `serde_json` at the handler boundary,
// so the JSON-Schema `target` / `kind` descriptions are informational only.
static TOOLS_SCHEMA: LazyLock<Value> = LazyLock::new(|| {
    serde_json::from_str(include_str!("tools_schema.json"))
        .expect("embedded tools_schema.json must be valid JSON")
});

pub(crate) fn handle_initialize(_params: Value) -> Result<Value, JsonRpcError> {
    Ok(json!({
        "protocolVersion": "2024-11-05",
        "capabilities": { "tools": {} },
        "serverInfo": {
            "name": "nit-mcp",
            "version": env!("CARGO_PKG_VERSION"),
        },
    }))
}

pub(crate) fn handle_tools_list() -> Result<Value, JsonRpcError> {
    Ok(TOOLS_SCHEMA.clone())
}

pub(crate) fn handle_tools_call<B: Backchannel>(
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
        Ok(json!({ "content": [{ "type": "text", "text": "ok" }] }))
    } else {
        Err(JsonRpcError {
            code: INTERNAL_ERROR,
            message: resp.error.unwrap_or_else(|| "unknown error".into()),
        })
    }
}

pub(crate) fn build_backchannel_request(
    tool: &str,
    request_id: u64,
    agent_id: &str,
    args: &Value,
) -> Result<BackchannelRequest, JsonRpcError> {
    let owned_agent = agent_id.to_string();
    match tool {
        "emit_signal" => build_emit_signal(request_id, owned_agent, args),
        "assert_claim" => build_assert_claim(request_id, owned_agent, args),
        "assert_assumption" => build_assert_assumption(request_id, owned_agent, args),
        other => Err(JsonRpcError {
            code: METHOD_NOT_FOUND,
            message: format!("unknown tool: {other}"),
        }),
    }
}

fn build_emit_signal(
    request_id: u64,
    agent_id: String,
    args: &Value,
) -> Result<BackchannelRequest, JsonRpcError> {
    Ok(BackchannelRequest::EmitSignal {
        request_id,
        agent_id,
        kind: parse_arg(args, "kind")?,
        target: parse_arg(args, "target")?,
        payload: args.get("payload").cloned().unwrap_or(Value::Null),
        strength: args
            .get("strength")
            .and_then(|v| v.as_f64())
            .map(|f| f as f32),
    })
}

fn build_assert_claim(
    request_id: u64,
    agent_id: String,
    args: &Value,
) -> Result<BackchannelRequest, JsonRpcError> {
    Ok(BackchannelRequest::AssertClaim {
        request_id,
        agent_id,
        kind: parse_arg(args, "kind")?,
        target: parse_arg(args, "target")?,
        ttl_gens: args.get("ttl_gens").and_then(|v| v.as_u64()).unwrap_or(3),
        rationale: args
            .get("rationale")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
    })
}

fn build_assert_assumption(
    request_id: u64,
    agent_id: String,
    args: &Value,
) -> Result<BackchannelRequest, JsonRpcError> {
    Ok(BackchannelRequest::AssertAssumption {
        request_id,
        agent_id,
        target: parse_arg(args, "target")?,
        fact: args.get("fact").cloned().unwrap_or(Value::Null),
        ttl_gens: args.get("ttl_gens").and_then(|v| v.as_u64()).unwrap_or(3),
        rationale: args
            .get("rationale")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
    })
}

fn parse_arg<T: DeserializeOwned>(args: &Value, field: &str) -> Result<T, JsonRpcError> {
    serde_json::from_value(args.get(field).cloned().unwrap_or(Value::Null)).map_err(|e| {
        JsonRpcError {
            code: INVALID_PARAMS,
            message: format!("invalid {field}: {e}"),
        }
    })
}
