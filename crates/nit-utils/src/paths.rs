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

macro_rules! define_project_dir {
    ($name:ident, $extract:expr) => {
        #[must_use]
        pub fn $name() -> Option<PathBuf> {
            project_path($extract)
        }
    };
}

define_project_dir!(config_dir, |pd| Some(pd.config_dir()));
define_project_dir!(data_dir, |pd| Some(pd.data_dir()));
define_project_dir!(state_dir, |pd| pd.state_dir());
define_project_dir!(cache_dir, |pd| Some(pd.cache_dir()));
