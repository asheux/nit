//! Capture name → highlight group mapping and tree-sitter config builders.

mod category;
mod config;
mod table;

pub use category::{CaptureCategory, Categorizable, CATEGORY_COUNT};
pub use table::capture_entry_count;

pub(crate) use config::{build_highlight_configs, build_query_configs, QueryConfig};
pub(crate) use table::capture_group;
