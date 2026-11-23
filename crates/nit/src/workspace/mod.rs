use std::path::{Path, PathBuf};

use nit_core::Buffer;

mod files;
mod state_paths;

use files::load_file_buffer;

pub(crate) use state_paths::{export_legacy_notes_snapshot, load_notes};

pub(crate) fn open_target_gol(path: Option<&Path>) -> anyhow::Result<(PathBuf, Buffer)> {
    open_target(path, "untitled", |dir| {
        Ok((dir.to_path_buf(), Buffer::empty("untitled", None)))
    })
}

pub(crate) fn open_target_games(path: Option<&Path>) -> anyhow::Result<(PathBuf, Buffer)> {
    open_target(path, "games.toml", games_dir_buffer)
}

fn open_target<F>(
    path: Option<&Path>,
    default_name: &str,
    dir_handler: F,
) -> anyhow::Result<(PathBuf, Buffer)>
where
    F: FnOnce(&Path) -> anyhow::Result<(PathBuf, Buffer)>,
{
    match path {
        Some(p) if p.is_file() => {
            let absolute = std::path::absolute(p)?;
            let root = absolute
                .parent()
                .expect("file has a parent")
                .canonicalize()?;
            let file = root.join(absolute.file_name().expect("file has a name"));
            let buffer = load_file_buffer(&file, default_name)?;
            Ok((root, buffer))
        }
        Some(p) if p.is_dir() => dir_handler(&p.canonicalize()?),
        None => dir_handler(&std::env::current_dir()?),
        Some(missing) => anyhow::bail!("path does not exist: {}", missing.display()),
    }
}

fn games_dir_buffer(dir: &Path) -> anyhow::Result<(PathBuf, Buffer)> {
    let root = dir.to_path_buf();
    let config_path = root.join("games.toml");
    if config_path.exists() {
        let buffer = load_file_buffer(&config_path, "games.toml")?;
        return Ok((root, buffer));
    }
    let buffer = Buffer::from_str(
        "games.toml",
        crate::games::games_template(),
        Some(config_path),
    );
    Ok((root, buffer))
}

pub(crate) fn find_theme() -> Option<PathBuf> {
    let candidate = std::env::current_dir()
        .ok()?
        .join("assets/themes/devs.toml");
    candidate.exists().then_some(candidate)
}
