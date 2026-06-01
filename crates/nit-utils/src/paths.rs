//! XDG project directories (process-lifetime cached) and the workspace
//! path-jail checks the file tree uses to confine rename/create edits.

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

/// A proposed leaf name must be a single in-tree path component: the file
/// tree joins it onto a parent directory, so a separator, `.`/`..`, or an
/// empty/whitespace value could redirect or escape the write and is refused.
#[must_use]
pub fn is_safe_leaf_name(name: &str) -> bool {
    if name.trim().is_empty() || name == "." || name == ".." {
        return false;
    }
    !name.contains(['/', '\\'])
}

/// True when `candidate`'s parent canonicalises to a location inside
/// `workspace_root`. The leaf need not exist (it is the create/rename
/// target); the parent must. Canonicalising both ends defeats `..` segments
/// and symlink hops that a lexical prefix check would miss.
#[must_use]
pub fn path_within(workspace_root: &Path, candidate: &Path) -> bool {
    let Some(parent) = candidate.parent() else {
        return false;
    };
    let (Ok(root), Ok(parent)) = (workspace_root.canonicalize(), parent.canonicalize()) else {
        return false;
    };
    parent.starts_with(&root)
}
