#![forbid(unsafe_code)]

pub mod actions;
pub mod buffer;
pub mod config;
pub mod cursor;
pub mod io;
pub mod gol_rules;
pub mod rule_config;
pub mod mode;
pub mod pane;
pub mod prompt;
pub mod seed;
pub mod state;
pub mod viewport;

pub use actions::Action;
pub use buffer::Buffer;
pub use buffer::{BufferEdit, BufferPoint};
pub use config::{
    EditorConfig, GolConfig, GolRuleConfig, GolRulesConfig, GolSearchConfig, GolSearchIntensity,
    GolSeedSource, GolSnapshotsConfig, GolUserRule, HighlightConfig, HighlightEngine, Settings,
    SnapshotPrunePolicy,
};
pub use gol_rules::{load_rule_catalog, NamedRule, RuleCatalog, RuleSelectError, SelectedRule};
pub use rule_config::{load_rule_config, persist_rule_selection, RuleConfigLoad, RulePersistence};
pub use cursor::Cursor;
pub use io::{load_to_string, save_buffer};
pub use mode::Mode;
pub use pane::PaneId;
pub use prompt::Prompt;
pub use seed::{
    encode_seed, EncodedSeed, SeedEncoderId, SeedParams, SeedPlacement, SeedPreviewMode, SeedStats,
    SeedSymmetry,
};
pub use state::{
    apply_action, AppState, GolRenderMode, JobState, LogBuffer, Metrics, VisualizerMode,
    VisualizerRuleEntry, VisualizerState, YankKind,
};
pub use viewport::Viewport;
