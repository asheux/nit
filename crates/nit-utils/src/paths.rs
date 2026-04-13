//! Process-lifetime cached project directories; all inputs are compile-time constants.

use std::path::PathBuf;
use std::sync::LazyLock;

use directories::ProjectDirs;

const QUALIFIER: &str = "dev";
const ORGANIZATION: &str = "arcxlab";
const APPLICATION: &str = "nit";

static DIRS: LazyLock<Option<ProjectDirs>> =
    LazyLock::new(|| ProjectDirs::from(QUALIFIER, ORGANIZATION, APPLICATION));

#[must_use]
pub fn config_dir() -> Option<PathBuf> {
    DIRS.as_ref().map(|pd| pd.config_dir().to_path_buf())
}

#[must_use]
pub fn data_dir() -> Option<PathBuf> {
    DIRS.as_ref().map(|pd| pd.data_dir().to_path_buf())
}

#[must_use]
pub fn state_dir() -> Option<PathBuf> {
    DIRS.as_ref()
        .and_then(|pd| pd.state_dir().map(|d| d.to_path_buf()))
}

#[must_use]
pub fn cache_dir() -> Option<PathBuf> {
    DIRS.as_ref().map(|pd| pd.cache_dir().to_path_buf())
}
