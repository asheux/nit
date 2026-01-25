#![forbid(unsafe_code)]

pub mod actions;
pub mod buffer;
pub mod config;
pub mod cursor;
pub mod io;
pub mod mode;
pub mod pane;
pub mod prompt;
pub mod state;
pub mod viewport;

pub use actions::Action;
pub use buffer::Buffer;
pub use buffer::{BufferEdit, BufferPoint};
pub use config::{EditorConfig, HighlightConfig, HighlightEngine, Settings};
pub use cursor::Cursor;
pub use io::{load_to_string, save_buffer};
pub use mode::Mode;
pub use pane::PaneId;
pub use prompt::Prompt;
pub use state::{apply_action, AppState, JobState, LogBuffer, Metrics, VisualizerState, YankKind};
pub use viewport::Viewport;
