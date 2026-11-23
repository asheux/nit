use std::path::Path;

use anyhow::Context;
use nit_core::{io as core_io, Buffer};

pub(super) fn load_file_buffer(path: &Path, default_name: &str) -> anyhow::Result<Buffer> {
    let content =
        core_io::load_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| default_name.into());
    Ok(Buffer::from_str(name, &content, Some(path.to_path_buf())))
}
