//! Integration tests for `nit_utils::paths`.

use nit_utils::paths;

#[test]
fn config_dir_returns_some() {
    // Should resolve on all desktop platforms.
    assert!(paths::config_dir().is_some());
}

#[test]
fn data_dir_returns_some() {
    assert!(paths::data_dir().is_some());
}

#[test]
fn cache_dir_returns_some() {
    assert!(paths::cache_dir().is_some());
}
