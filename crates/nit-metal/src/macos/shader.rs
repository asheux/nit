//! Metal shader compilation and GPU context management.
//!
//! Compiles specialized Metal shader variants for different payload types
//! (FSM, CA, TM), caching compiled pipeline states behind a process-global
//! singleton keyed by [`ShaderKey`].

use crate::{BatchPayload, CA_MAX_WINDOW, FSM_MAX_STATES, TM_MAX_WIDTH};
use metal::{CompileOptions, ComputePipelineState, Device, Library};
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

/// Raw Metal shader source embedded at compile time.
const SHADER_SOURCE: &str = include_str!("../batch_eval.metal");

// ---------------------------------------------------------------------------
// Compile-time constant management
// ---------------------------------------------------------------------------

/// Returns the larger of `required` and `compiled_default`, both floored at 1.
///
/// Used when a payload's actual dimension exceeds the default constant
/// compiled into the shader — the shader must be recompiled with a wider
/// array bound to accommodate the larger dimension.
fn widen_limit(required: u32, compiled_default: u32) -> u32 {
    let floor = compiled_default.max(1);
    required.max(1).max(floor)
}

// ---------------------------------------------------------------------------
// Shader variant identification
// ---------------------------------------------------------------------------

/// Identifies a specialized shader variant by its compile-time constants.
///
/// Each combination of array bounds produces a distinct Metal library.
/// Variants are compiled once and cached for the lifetime of the process.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct ShaderKey {
    pub(crate) ca_max_window: u32,
    pub(crate) tm_max_width: u32,
    pub(crate) fsm_max_states: u32,
}

impl ShaderKey {
    /// Default key using the crate-level constant limits.
    ///
    /// Suitable for payloads whose dimensions fit within the compiled defaults.
    pub(crate) fn defaults() -> Self {
        Self {
            ca_max_window: CA_MAX_WINDOW.max(1),
            tm_max_width: TM_MAX_WIDTH.max(1),
            fsm_max_states: FSM_MAX_STATES.max(1),
        }
    }

    /// Key for FSM payloads, widening the state table if needed.
    pub(crate) fn for_fsm(required_states: u32) -> Self {
        let mut key = Self::defaults();
        key.fsm_max_states = widen_limit(required_states, FSM_MAX_STATES);
        key
    }

    /// Key for TM payloads, widening the tape scratch buffer if needed.
    ///
    /// The scratch width must be at least `max_steps + 1` to accommodate
    /// the longest possible tape expansion during simulation.
    pub(crate) fn for_tm(max_steps: u32) -> Self {
        let mut key = Self::defaults();
        let required_width = max_steps.saturating_add(1);
        key.tm_max_width = widen_limit(required_width, TM_MAX_WIDTH);
        key
    }

    /// Key for CA payloads, widening the evolution window if needed.
    pub(crate) fn for_ca(window_size: u32) -> Self {
        let mut key = Self::defaults();
        key.ca_max_window = widen_limit(window_size, CA_MAX_WINDOW);
        key
    }

    /// Derive the appropriate key from a payload's runtime parameters.
    ///
    /// Dispatches to the variant-specific constructor, which determines
    /// whether the default constants are sufficient or need widening.
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

// ---------------------------------------------------------------------------
// Shader source generation
// ---------------------------------------------------------------------------

/// Generates Metal source code with `#define` constants for the given key.
///
/// Prepends the three dimension constants before the embedded shader source
/// so that array bounds in the Metal kernels match the payload requirements.
fn shader_source_with_defines(key: ShaderKey) -> String {
    format!(
        "#define CA_MAX_WINDOW {}u\n#define TM_MAX_WIDTH {}u\n#define FSM_MAX_STATES {}u\n{}",
        key.ca_max_window, key.tm_max_width, key.fsm_max_states, SHADER_SOURCE
    )
}

// ---------------------------------------------------------------------------
// Pipeline compilation
// ---------------------------------------------------------------------------

/// Compiles a single named kernel function into a compute pipeline state.
///
/// Looks up the function by name in the compiled Metal library, then creates
/// a compute pipeline optimized for the current device.
fn compile_kernel(
    device: &Device,
    library: &Library,
    kernel_name: &str,
) -> Result<ComputePipelineState, String> {
    let function = library
        .get_function(kernel_name, None)
        .map_err(|err| err.to_string())?;
    device.new_compute_pipeline_state_with_function(&function)
}

/// Holds a compiled Metal device, command queue, and per-kernel pipeline states.
///
/// Each `MetalContext` represents a fully compiled shader variant: three
/// compute pipelines (FSM, CA, TM) sharing a single device and queue.
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
    ///
    /// Acquires the system default Metal device, compiles the shader source
    /// with dimension-specific `#define` constants, and creates a pipeline
    /// state for each kernel variant.
    fn compile(key: ShaderKey) -> Result<Self, String> {
        let device =
            Device::system_default().ok_or_else(|| "Metal device unavailable".to_string())?;

        let options = CompileOptions::new();
        let source = shader_source_with_defines(key);
        let library = device.new_library_with_source(&source, &options)?;

        let fsm_pipeline = compile_kernel(&device, &library, "fsm_batch")?;
        let ca_pipeline = compile_kernel(&device, &library, "ca_batch")?;
        let tm_pipeline = compile_kernel(&device, &library, "tm_batch")?;

        let queue = device.new_command_queue();

        Ok(Self {
            device,
            queue,
            _library: library,
            fsm_pipeline,
            ca_pipeline,
            tm_pipeline,
        })
    }
}

// ---------------------------------------------------------------------------
// Process-global context cache
// ---------------------------------------------------------------------------

/// Returns a cached `MetalContext` for the given shader key.
///
/// The first call for a given key compiles the shader; subsequent calls
/// return the cached reference. Contexts are intentionally leaked to provide
/// a `'static` lifetime — GPU pipeline objects are expensive to create and
/// are reused for the entire process lifetime.
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

    let result =
        MetalContext::compile(key).map(|ctx| Box::leak(Box::new(ctx)) as &'static MetalContext);
    guard.insert(key, result.clone());
    result
}

/// Compiles the default shader variant, warming the pipeline cache.
///
/// Call this at startup to avoid compilation latency on the first batch
/// dispatch. The compiled pipelines are cached for the process lifetime.
pub fn prewarm_default_batch_shaders() -> Result<(), String> {
    let _ = context_for_key(ShaderKey::defaults())?;
    Ok(())
}
