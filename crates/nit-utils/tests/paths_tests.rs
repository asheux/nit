use nit_utils::paths;

#[test]
fn config_dir_resolves() {
    assert!(paths::config_dir().is_some(), "config dir should resolve");
}

#[test]
fn data_dir_resolves() {
    assert!(paths::data_dir().is_some(), "data dir should resolve");
}

#[test]
fn cache_dir_resolves() {
    assert!(paths::cache_dir().is_some(), "cache dir should resolve");
}

#[test]
fn state_dir_platform_dependent() {
    let result = paths::state_dir();
    if cfg!(target_os = "linux") {
        assert!(result.is_some(), "Linux should have a state directory");
    } else if cfg!(target_os = "macos") {
        assert!(result.is_none(), "macOS has no XDG state equivalent");
    }
}

#[test]
fn all_dirs_contain_app_name() {
    for dir in [paths::config_dir(), paths::data_dir(), paths::cache_dir()] {
        let path = dir.expect("directory should resolve");
        let s = path.to_string_lossy();
        assert!(s.contains("nit"), "expected 'nit' in path: {s}");
    }
}
