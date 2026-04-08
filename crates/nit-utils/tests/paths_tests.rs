use nit_utils::paths;

#[test]
fn config_dir_resolves() {
    assert!(paths::config_dir().is_some());
}

#[test]
fn data_dir_resolves() {
    assert!(paths::data_dir().is_some());
}

#[test]
fn cache_dir_resolves() {
    assert!(paths::cache_dir().is_some());
}

#[test]
fn state_dir_platform_dependent() {
    // state_dir is None on macOS (no XDG state equivalent), Some on Linux.
    let result = paths::state_dir();
    if cfg!(target_os = "linux") {
        assert!(result.is_some(), "Linux should have a state directory");
    }
    // On macOS/Windows, None is acceptable — just verify no panic.
}

#[test]
fn all_dirs_contain_app_name() {
    let app = "nit";
    for dir in [paths::config_dir(), paths::data_dir(), paths::cache_dir()] {
        let path = dir.expect("directory should resolve");
        let path_str = path.to_string_lossy();
        assert!(
            path_str.contains(app),
            "expected '{app}' in path: {path_str}"
        );
    }
}
