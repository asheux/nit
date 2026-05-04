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
        let Some(path) = dir else {
            // CI sandboxes without a resolvable HOME yield None for every
            // accessor; skip rather than panic so the test stays portable.
            eprintln!("skipping {name}: ProjectDirs unavailable in this environment");
            continue;
        };
        let display = path.to_string_lossy();
        assert!(
            display.contains("nit"),
            "expected 'nit' in {name} path: {display}"
        );
    }
}

// Pins what the `directories` crate actually guarantees per platform: the
// cache directory is always rooted separately from data, and on XDG-style
// systems (Linux) all three of config/data/cache resolve to distinct paths.
// macOS conflates config_dir and data_dir under `Application Support`, so
// the cross-platform invariant is narrower than "all three differ".
#[test]
fn cache_dir_does_not_alias_data_dir() {
    let (Some(data), Some(cache)) = (paths::data_dir(), paths::cache_dir()) else {
        eprintln!("skipping cache/data distinctness: ProjectDirs unavailable");
        return;
    };
    assert_ne!(data, cache, "cache must not alias data");

    if cfg!(target_os = "linux") {
        let config = paths::config_dir().expect("Linux always resolves config_dir");
        assert_ne!(config, data, "Linux: config and data must not alias");
        assert_ne!(config, cache, "Linux: config and cache must not alias");
    }
}
