//! Capture name → highlight group mapping and tree-sitter config builders.

mod category;
mod config;
mod table;

pub use category::{CaptureCategory, Categorizable, CATEGORY_COUNT};
pub use table::capture_entry_count;

pub(crate) use config::{highlight_config, markdown_inline_config, query_config, QueryConfig};
pub(crate) use table::capture_group;
