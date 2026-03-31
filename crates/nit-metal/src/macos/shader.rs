//! Metal shader compilation and GPU context management.
//!
//! Handles compiling specialized Metal shader variants for different
//! payload types (FSM, CA, TM) and caching compiled pipeline states.

use crate::{BatchPayload, CA_MAX_WINDOW, FSM_MAX_STATES, TM_MAX_WIDTH};
use metal::{CompileOptions, ComputePipelineState, Device, Library};
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

const SHADER_SOURCE: &str = include_str!("../batch_eval.metal");

/// Identifies a specialized shader variant by its compile-time constants.
///
/// Each payload type may need different fixed-size array dimensions in the
/// Metal kernel. Variants are compiled and cached per unique key.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct ShaderKey {
    pub(crate) ca_max_window: u32,
    pub(crate) tm_max_width: u32,
    pub(crate) fsm_max_states: u32,
}

impl ShaderKey {
    /// Default key using the crate-level constant limits.
    pub(crate) fn defaults() -> Self {
        Self {
            ca_max_window: CA_MAX_WINDOW.max(1),
            tm_max_width: TM_MAX_WIDTH.max(1),
            fsm_max_states: FSM_MAX_STATES.max(1),
        }
    }

    /// Key for FSM payloads, widening the state table if `states` exceeds the default.
    pub(crate) fn for_fsm(required_states: u32) -> Self {
        let clamped = required_states.max(1);
        let default_states = FSM_MAX_STATES.max(1);
        Self {
            ca_max_window: CA_MAX_WINDOW.max(1),
            tm_max_width: TM_MAX_WIDTH.max(1),
            fsm_max_states: if clamped <= default_states {
                default_states
            } else {
                clamped
            },
        }
    }

    /// Key for TM payloads, widening the tape buffer if `max_steps` exceeds the default.
    pub(crate) fn for_tm(max_steps: u32) -> Self {
        let required_width = max_steps.saturating_add(1).max(1);
        let default_width = TM_MAX_WIDTH.max(1);
        Self {
            ca_max_window: CA_MAX_WINDOW.max(1),
            tm_max_width: if required_width <= default_width {
                default_width
            } else {
                required_width
            },
            fsm_max_states: FSM_MAX_STATES.max(1),
        }
    }

    /// Key for CA payloads, widening the window if the rule radius requires it.
    pub(crate) fn for_ca(window_size: u32) -> Self {
        let required_window = window_size.max(1);
        let default_window = CA_MAX_WINDOW.max(1);
        Self {
            ca_max_window: if required_window <= default_window {
                default_window
            } else {
                required_window
            },
            tm_max_width: TM_MAX_WIDTH.max(1),
            fsm_max_states: FSM_MAX_STATES.max(1),
        }
    }

    /// Derive the appropriate key from a payload's parameters.
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

/// Generates Metal source code with `#define` constants for the given key.
fn shader_source_with_defines(key: ShaderKey) -> String {
    format!(
        "#define CA_MAX_WINDOW {}u\n#define TM_MAX_WIDTH {}u\n#define FSM_MAX_STATES {}u\n{}",
        key.ca_max_window, key.tm_max_width, key.fsm_max_states, SHADER_SOURCE
    )
}

/// Holds a compiled Metal device, command queue, and per-kernel pipeline states.
pub(super) struct MetalContext {
    pub(super) device: Device,
    pub(super) queue: metal::CommandQueue,
    _library: Library,
    pub(super) fsm_pipeline: ComputePipelineState,
    pub(super) ca_pipeline: ComputePipelineState,
    pub(super) tm_pipeline: ComputePipelineState,
}

impl MetalContext {
    /// Compiles all three shader kernels for the given key dimensions.
    fn compile(key: ShaderKey) -> Result<Self, String> {
        let device =
            Device::system_default().ok_or_else(|| "Metal device unavailable".to_string())?;

        let compile_opts = CompileOptions::new();
        let source_code = shader_source_with_defines(key);
        let library = device.new_library_with_source(&source_code, &compile_opts)?;

        let fsm_function = library
            .get_function("fsm_batch", None)
            .map_err(|err| err.to_string())?;
        let ca_function = library
            .get_function("ca_batch", None)
            .map_err(|err| err.to_string())?;
        let tm_function = library
            .get_function("tm_batch", None)
            .map_err(|err| err.to_string())?;

        let fsm_pipeline = device.new_compute_pipeline_state_with_function(&fsm_function)?;
        let ca_pipeline = device.new_compute_pipeline_state_with_function(&ca_function)?;
        let tm_pipeline = device.new_compute_pipeline_state_with_function(&tm_function)?;

        let command_queue = device.new_command_queue();

        Ok(Self {
            device,
            queue: command_queue,
            _library: library,
            fsm_pipeline,
            ca_pipeline,
            tm_pipeline,
        })
    }
}

/// Returns a cached, leaked `MetalContext` for the given shader key.
///
/// The first call for a given key compiles the shader; subsequent calls
/// return the cached reference. Contexts are leaked intentionally to provide
/// a `'static` lifetime for the GPU pipelines.
pub(super) fn context_for_key(key: ShaderKey) -> Result<&'static MetalContext, String> {
    static CONTEXTS: OnceLock<Mutex<HashMap<ShaderKey, Result<&'static MetalContext, String>>>> =
        OnceLock::new();

    let cache = CONTEXTS.get_or_init(|| Mutex::new(HashMap::new()));
    let mut guard = cache
        .lock()
        .map_err(|_| "Metal context cache lock poisoned".to_string())?;

    if let Some(existing) = guard.get(&key) {
        return existing.clone();
    }

    let compiled = MetalContext::compile(key);
    let leaked = compiled.map(|ctx| Box::leak(Box::new(ctx)) as &'static MetalContext);
    guard.insert(key, leaked.clone());
    leaked
}

/// Compiles the default shader variant, warming the pipeline cache.
pub fn prewarm_default_batch_shaders() -> Result<(), String> {
    let _ = context_for_key(ShaderKey::defaults())?;
    Ok(())
}
