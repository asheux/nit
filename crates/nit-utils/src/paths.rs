use directories::ProjectDirs;
use std::path::PathBuf;

const QUALIFIER: &str = "dev";
const ORGANIZATION: &str = "openai";
const APPLICATION: &str = "nit";

fn project_dirs() -> Option<ProjectDirs> {
    ProjectDirs::from(QUALIFIER, ORGANIZATION, APPLICATION)
}

pub fn config_dir() -> Option<PathBuf> {
    project_dirs().map(|p| p.config_dir().to_path_buf())
}

pub fn data_dir() -> Option<PathBuf> {
    project_dirs().map(|p| p.data_dir().to_path_buf())
}

pub fn state_dir() -> Option<PathBuf> {
    project_dirs().and_then(|p| p.state_dir().map(|d| d.to_path_buf()))
}

pub fn cache_dir() -> Option<PathBuf> {
    project_dirs().map(|p| p.cache_dir().to_path_buf())
}
