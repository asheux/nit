//! Process-lifetime cached project directories; all inputs are compile-time constants.

use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use directories::ProjectDirs;

const QUALIFIER: &str = "dev";
const ORGANIZATION: &str = "arcxlab";
const APPLICATION: &str = "nit";

static PROJECT_DIRS: LazyLock<Option<ProjectDirs>> =
    LazyLock::new(|| ProjectDirs::from(QUALIFIER, ORGANIZATION, APPLICATION));

fn project_path(extract: fn(&ProjectDirs) -> Option<&Path>) -> Option<PathBuf> {
    PROJECT_DIRS
        .as_ref()
        .and_then(extract)
        .map(Path::to_path_buf)
}

#[must_use]
pub fn config_dir() -> Option<PathBuf> {
    project_path(|pd| Some(pd.config_dir()))
}

#[must_use]
pub fn data_dir() -> Option<PathBuf> {
    project_path(|pd| Some(pd.data_dir()))
}

// `state_dir` is the only accessor whose underlying `ProjectDirs` API already
// returns `Option`; macOS has no XDG state equivalent and yields `None`.
#[must_use]
pub fn state_dir() -> Option<PathBuf> {
    project_path(ProjectDirs::state_dir)
}

#[must_use]
pub fn cache_dir() -> Option<PathBuf> {
    project_path(|pd| Some(pd.cache_dir()))
}
