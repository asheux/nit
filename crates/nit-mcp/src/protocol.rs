//! Back-channel wire schema. Reuses nit-core substrate enums so target/kind round-trip without a bespoke schema.

use serde::{Deserialize, Serialize};

use nit_core::substrate::{AssumptionTarget, ClaimKind, ClaimTarget, SignalKind, SignalTarget};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "tool", rename_all = "snake_case")]
pub enum BackchannelRequest {
    EmitSignal {
        request_id: u64,
        agent_id: String,
        kind: SignalKind,
        target: SignalTarget,
        #[serde(default)]
        payload: serde_json::Value,
        strength: Option<f32>,
    },
    AssertClaim {
        request_id: u64,
        agent_id: String,
        kind: ClaimKind,
        target: ClaimTarget,
        ttl_gens: u64,
        rationale: String,
    },
    AssertAssumption {
        request_id: u64,
        agent_id: String,
        target: AssumptionTarget,
        #[serde(default)]
        fact: serde_json::Value,
        ttl_gens: u64,
        rationale: String,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BackchannelResponse {
    pub request_id: u64,
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}
