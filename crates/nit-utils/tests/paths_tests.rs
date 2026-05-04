use nit_utils::paths;

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
    for (name, dir) in [
        ("config", paths::config_dir()),
        ("data", paths::data_dir()),
        ("cache", paths::cache_dir()),
    ] {
        let path = dir.unwrap_or_else(|| panic!("{name} dir should resolve"));
        let display = path.to_string_lossy();
        assert!(
            display.contains("nit"),
            "expected 'nit' in {name} path: {display}"
        );
    }
}
