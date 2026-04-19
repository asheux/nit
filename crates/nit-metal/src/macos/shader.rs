//! Metal shader compilation and per-variant GPU context caching.

use super::MetalResult;
use crate::{BatchPayload, CA_MAX_WINDOW, FSM_MAX_STATES, TM_MAX_WIDTH};
use metal::{CompileOptions, ComputePipelineState, Device, Library};
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

const SHADER_SOURCE: &str = include_str!("../batch_eval.metal");

/// Widens a compile-time array bound so the kernel can handle the caller's
/// runtime requirement. A single helper here encodes the "never below the
/// compiled default, never below 1" invariant for all three kernel families.
fn widen_limit(required: u32, compiled_default: u32) -> u32 {
    let floor = compiled_default.max(1);
    required.max(1).max(floor)
}

/// Specialized-shader identity: every distinct combination of bounds yields a
/// distinct Metal library, compiled once and cached for the process lifetime.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct ShaderKey {
    pub(crate) ca_max_window: u32,
    pub(crate) tm_max_width: u32,
    pub(crate) fsm_max_states: u32,
}

impl ShaderKey {
    pub(crate) fn defaults() -> Self {
        Self {
            ca_max_window: CA_MAX_WINDOW.max(1),
            tm_max_width: TM_MAX_WIDTH.max(1),
            fsm_max_states: FSM_MAX_STATES.max(1),
        }
    }

    pub(crate) fn for_fsm(required_states: u32) -> Self {
        Self {
            fsm_max_states: widen_limit(required_states, FSM_MAX_STATES),
            ..Self::defaults()
        }
    }

    /// Scratch width must be at least `max_steps + 1` for the longest tape
    /// expansion the kernel may encounter.
    pub(crate) fn for_tm(max_steps: u32) -> Self {
        Self {
            tm_max_width: widen_limit(max_steps.saturating_add(1), TM_MAX_WIDTH),
            ..Self::defaults()
        }
    }

    pub(crate) fn for_ca(window_size: u32) -> Self {
        Self {
            ca_max_window: widen_limit(window_size, CA_MAX_WINDOW),
            ..Self::defaults()
        }
    }

    pub(crate) fn for_payload(payload: &BatchPayload) -> Self {
        match payload {
            BatchPayload::Fsm(fsm) => Self::for_fsm(fsm.states),
            BatchPayload::Tm(tm) => Self::for_tm(tm.max_steps),
            BatchPayload::Ca(ca) => {
                Self::for_ca(ca.two_r.saturating_mul(ca.steps).saturating_add(1))
            }
        }
    }
}

fn shader_source_with_defines(key: ShaderKey) -> String {
    format!(
        "#define CA_MAX_WINDOW {}u\n#define TM_MAX_WIDTH {}u\n#define FSM_MAX_STATES {}u\n{}",
        key.ca_max_window, key.tm_max_width, key.fsm_max_states, SHADER_SOURCE
    )
}

fn compile_kernel(
    device: &Device,
    library: &Library,
    kernel_name: &str,
) -> MetalResult<ComputePipelineState> {
    let function = library
        .get_function(kernel_name, None)
        .map_err(|err| err.to_string())?;
    device.new_compute_pipeline_state_with_function(&function)
}

pub(super) struct MetalContext {
    pub(super) device: Device,
    pub(super) queue: metal::CommandQueue,
    /// Held to keep the Metal library alive; pipeline state references the
    /// library internally and may segfault if it is dropped first.
    _library: Library,
    pub(super) fsm_pipeline: ComputePipelineState,
    pub(super) ca_pipeline: ComputePipelineState,
    pub(super) tm_pipeline: ComputePipelineState,
}

impl MetalContext {
    fn compile(key: ShaderKey) -> MetalResult<Self> {
        let device =
            Device::system_default().ok_or_else(|| "Metal device unavailable".to_string())?;

        let source = shader_source_with_defines(key);
        let library = device.new_library_with_source(&source, &CompileOptions::new())?;

        let fsm_pipeline = compile_kernel(&device, &library, "fsm_batch")?;
        let ca_pipeline = compile_kernel(&device, &library, "ca_batch")?;
        let tm_pipeline = compile_kernel(&device, &library, "tm_batch")?;

        Ok(Self {
            device: device.clone(),
            queue: device.new_command_queue(),
            _library: library,
            fsm_pipeline,
            ca_pipeline,
            tm_pipeline,
        })
    }
}

/// Per-key context cache. Contexts are intentionally leaked to provide a
/// `'static` lifetime — GPU pipeline objects are expensive to build and the
/// key space is bounded (a handful of specialized variants per process).
static CONTEXTS: OnceLock<Mutex<HashMap<ShaderKey, MetalResult<&'static MetalContext>>>> =
    OnceLock::new();

pub(super) fn context_for_key(key: ShaderKey) -> MetalResult<&'static MetalContext> {
    let cache = CONTEXTS.get_or_init(|| Mutex::new(HashMap::new()));
    let mut guard = cache
        .lock()
        .map_err(|_| "Metal context cache lock poisoned".to_string())?;

    if let Some(existing) = guard.get(&key) {
        return existing.clone();
    }

    let result =
        MetalContext::compile(key).map(|ctx| Box::leak(Box::new(ctx)) as &'static MetalContext);
    guard.insert(key, result.clone());
    result
}

pub fn prewarm_default_batch_shaders() -> MetalResult<()> {
    context_for_key(ShaderKey::defaults()).map(|_| ())
}
