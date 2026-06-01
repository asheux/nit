//! `AppState` tests grouped by concern. The mounted entry: each submodule
//! isolates one command/feature surface so growth in one area does not
//! balloon the parent file.

use super::*;
use crate::buffer::Buffer;
use crate::test_helpers::temp_dir;
use std::fs;

#[path = "state/auto_pair.rs"]
mod auto_pair;
#[path = "state/change_ops.rs"]
mod change_ops;
#[path = "state/count_prefix.rs"]
mod count_prefix;
#[path = "state/definition.rs"]
mod definition;
#[path = "state/games_family_build.rs"]
mod games_family_build;
#[path = "state/games_inspect.rs"]
mod games_inspect;
#[path = "state/games_matches_estimate.rs"]
mod games_matches_estimate;
#[path = "state/games_metal_diagnostics.rs"]
mod games_metal_diagnostics;
#[path = "state/games_tm_family.rs"]
mod games_tm_family;
#[path = "state/help_and_commands.rs"]
mod help_and_commands;
#[path = "state/indent_tab.rs"]
mod indent_tab;
#[path = "state/jumplist.rs"]
mod jumplist;
#[path = "state/quit_and_buffers.rs"]
mod quit_and_buffers;
#[path = "state/rule_and_picker.rs"]
mod rule_and_picker;
#[path = "state/search_prompt.rs"]
mod search_prompt;
#[path = "state/yank_register.rs"]
mod yank_register;

fn empty_state(label: &str) -> (std::path::PathBuf, AppState) {
    let root = temp_dir(label);
    let state = AppState::new(
        root.clone(),
        Buffer::empty("x", None),
        Buffer::empty("n", None),
    );
    (root, state)
}

fn games_state_with_config(label: &str, config: &str) -> (std::path::PathBuf, AppState) {
    let root = temp_dir(label);
    let mut state = AppState::new(
        root.clone(),
        Buffer::from_str("x", config, None),
        Buffer::empty("n", None),
    );
    state.app_kind = AppKind::Games;
    (root, state)
}
