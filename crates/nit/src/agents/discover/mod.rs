mod parse;
mod path;
mod runner;

pub(in crate::agents) use path::find_executable_in_path;
pub(in crate::agents) use runner::{
    capture_cli_help_text, probe_models_from_cli, DEFAULT_MODEL_LIST_ARG_SETS,
};

// `*_cli_available()` helpers were collapsed into `init_agents` once it
// started caching `Option<PathBuf>` for cache invalidation purposes:
// keeping just `find_executable_in_path` avoids resolving each binary
// twice (once for "is it there?" and once for "where is it?").
