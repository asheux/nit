//! Platform-aware application directory resolution.
//!
//! Uses the [`directories`] crate to locate standard OS directories for
//! configuration, data, state, and caches. All functions return `None` when
//! the platform cannot determine a suitable base directory.

use std::path::PathBuf;

use directories::ProjectDirs;

/// XDG-style qualifier (domain segment).
const QUALIFIER: &str = "dev";

/// Organisation segment used by the `directories` crate for path construction.
///
/// # Historical note
///
/// This value is `"openai"` for backward compatibility. Changing it would alter
/// the resolved directory paths on every platform (e.g.
/// `~/Library/Application Support/dev.openai.nit` on macOS), silently orphaning
/// existing user configuration, data, state, and cache directories. A future
/// migration must create a relocation function before this constant can be
/// updated.
const ORGANIZATION: &str = "openai";

/// Application name used as the final directory component.
const APPLICATION: &str = "nit";

/// Resolves the platform-specific [`ProjectDirs`] for this application.
///
/// Returns `None` if the OS cannot determine a valid home directory.
fn project_dirs() -> Option<ProjectDirs> {
    ProjectDirs::from(QUALIFIER, ORGANIZATION, APPLICATION)
}

/// Returns the platform-specific configuration directory.
///
/// On Linux this is typically `$XDG_CONFIG_HOME/nit` or `~/.config/nit`.
/// On macOS: `~/Library/Application Support/dev.openai.nit`.
///
/// Returns `None` if the home directory cannot be determined.
#[inline]
#[must_use]
pub fn config_dir() -> Option<PathBuf> {
    project_dirs().map(|pd| pd.config_dir().to_path_buf())
}

/// Returns the platform-specific persistent data directory.
///
/// On Linux this is typically `$XDG_DATA_HOME/nit` or `~/.local/share/nit`.
/// On macOS: `~/Library/Application Support/dev.openai.nit`.
///
/// Returns `None` if the home directory cannot be determined.
#[inline]
#[must_use]
pub fn data_dir() -> Option<PathBuf> {
    project_dirs().map(|pd| pd.data_dir().to_path_buf())
}

/// Returns the platform-specific state directory.
///
/// On Linux this is typically `$XDG_STATE_HOME/nit` or `~/.local/state/nit`.
///
/// Returns `None` if the platform does not define a state directory (e.g. macOS
/// has no state-dir equivalent) or if the home directory cannot be determined.
#[inline]
#[must_use]
pub fn state_dir() -> Option<PathBuf> {
    project_dirs().and_then(|pd| pd.state_dir().map(|d| d.to_path_buf()))
}

/// Returns the platform-specific cache directory.
///
/// On Linux this is typically `$XDG_CACHE_HOME/nit` or `~/.cache/nit`.
/// On macOS: `~/Library/Caches/dev.openai.nit`.
///
/// Returns `None` if the home directory cannot be determined.
#[inline]
#[must_use]
pub fn cache_dir() -> Option<PathBuf> {
    project_dirs().map(|pd| pd.cache_dir().to_path_buf())
}
