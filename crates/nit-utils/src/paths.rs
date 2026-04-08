//! Platform-aware application directory resolution via the `directories` crate.

use std::path::PathBuf;

use directories::ProjectDirs;

const QUALIFIER: &str = "dev";

/// Set to `"openai"` for backward compatibility — changing it would orphan
/// existing user directories. Requires a migration function before updating.
const ORGANIZATION: &str = "openai";

const APPLICATION: &str = "nit";

fn project_dirs() -> Option<ProjectDirs> {
    ProjectDirs::from(QUALIFIER, ORGANIZATION, APPLICATION)
}

/// Returns the platform-specific configuration directory.
#[must_use]
pub fn config_dir() -> Option<PathBuf> {
    project_dirs().map(|pd| pd.config_dir().to_path_buf())
}

/// Returns the platform-specific persistent data directory.
#[must_use]
pub fn data_dir() -> Option<PathBuf> {
    project_dirs().map(|pd| pd.data_dir().to_path_buf())
}

/// Returns the platform-specific state directory, if the OS defines one.
#[must_use]
pub fn state_dir() -> Option<PathBuf> {
    project_dirs().and_then(|pd| pd.state_dir().map(|d| d.to_path_buf()))
}

/// Returns the platform-specific cache directory.
#[must_use]
pub fn cache_dir() -> Option<PathBuf> {
    project_dirs().map(|pd| pd.cache_dir().to_path_buf())
}
