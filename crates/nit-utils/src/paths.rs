//! Platform-aware application directory resolution via the `directories` crate.

use std::path::PathBuf;

use directories::ProjectDirs;

const QUALIFIER: &str = "dev";

const ORGANIZATION: &str = "arcxlab";

const APPLICATION: &str = "nit";

fn project_dirs() -> Option<ProjectDirs> {
    ProjectDirs::from(QUALIFIER, ORGANIZATION, APPLICATION)
}

#[must_use]
pub fn config_dir() -> Option<PathBuf> {
    project_dirs().map(|pd| pd.config_dir().to_path_buf())
}

#[must_use]
pub fn data_dir() -> Option<PathBuf> {
    project_dirs().map(|pd| pd.data_dir().to_path_buf())
}

#[must_use]
pub fn state_dir() -> Option<PathBuf> {
    project_dirs().and_then(|pd| pd.state_dir().map(|d| d.to_path_buf()))
}

#[must_use]
pub fn cache_dir() -> Option<PathBuf> {
    project_dirs().map(|pd| pd.cache_dir().to_path_buf())
}
