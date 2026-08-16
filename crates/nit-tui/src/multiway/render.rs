//! Phase 6a: render a multiway search DAG to an image and open it in the OS
//! viewer, on demand. [`render_and_open`] builds Graphviz DOT via
//! [`Graph::to_dot`], then — when `dot` is on `PATH` — writes a placeholder
//! image, spawns the platform viewer on it immediately, and overwrites the file
//! with the final render so the *same* window reloads to the finished graph (the
//! Preview-reload trick). When graphviz is absent it degrades to saving the
//! `.dot` source and returning that path, never an error.
//!
//! `dot` and the viewer are spawned directly via [`std::process::Command`] — no
//! shell, no new dependency — mirroring the `git` spawn convention in
//! `nit-multiway`. The call is blocking; the caller runs it on a worker thread so
//! the TUI stays responsive, and it reads only the [`Graph`] it is handed, never
//! the operator's working tree.

use std::ffi::{OsStr, OsString};
use std::fmt;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use nit_multiway::graph::Graph;

/// A 24×24 light-gray PNG written to the target path before `dot` runs, so the
/// viewer has a valid image to open immediately; the final render overwrites it
/// in place and the viewer reloads. Embedded as bytes to avoid a draw-time
/// dependency on an external placeholder file.
const PLACEHOLDER_PNG: &[u8] = &[
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x18, 0x00, 0x00, 0x00, 0x18, 0x08, 0x02, 0x00, 0x00, 0x00, 0x6f, 0x15, 0xaa,
    0xaf, 0x00, 0x00, 0x00, 0x1d, 0x49, 0x44, 0x41, 0x54, 0x78, 0xda, 0x63, 0x38, 0x43, 0x25, 0xc0,
    0x30, 0x6a, 0xd0, 0xa8, 0x41, 0xa3, 0x06, 0x8d, 0x1a, 0x34, 0x6a, 0xd0, 0xa8, 0x41, 0x03, 0x6f,
    0x10, 0x00, 0x06, 0xff, 0x61, 0x4c, 0x52, 0x60, 0xf1, 0xcc, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45,
    0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
];

/// Outcome of a successful render.
#[derive(Debug)]
pub enum RenderOutcome {
    /// `dot` rendered the DAG to this PNG and the platform viewer was opened on it.
    Image(PathBuf),
    /// Graphviz was absent: the DOT source was saved here for the caller to surface.
    DotOnly(PathBuf),
}

#[derive(Debug)]
pub enum RenderError {
    /// Creating the output dir, the placeholder, or the `.dot` source failed.
    Io(io::Error),
    /// `dot` ran but exited non-zero; carries its trimmed stderr.
    Dot(String),
    /// The platform viewer could not be spawned.
    Open(io::Error),
}

impl fmt::Display for RenderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RenderError::Io(e) => write!(f, "multiway graph render failed: {e}"),
            RenderError::Dot(stderr) => write!(f, "graphviz dot exited non-zero: {stderr}"),
            RenderError::Open(e) => write!(f, "could not open the rendered graph: {e}"),
        }
    }
}

impl std::error::Error for RenderError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            RenderError::Io(e) | RenderError::Open(e) => Some(e),
            RenderError::Dot(_) => None,
        }
    }
}

/// Render the DAG and open it in the OS image viewer. The entry point the Phase
/// 6a `@multiway-graph` command calls with an (in-memory or loaded) [`Graph`];
/// `state_dir` is the base whose `multiway/` subdir receives the output file.
pub fn render_and_open(graph: &Graph, state_dir: &Path) -> Result<RenderOutcome, RenderError> {
    let dot = locate_on_path(if cfg!(windows) { "dot.exe" } else { "dot" });
    let opener = platform_opener();
    render_with(graph, state_dir, dot.as_deref(), Some(opener.as_os_str()))
}

/// The injectable core of [`render_and_open`]: `dot = None` forces the
/// graphviz-absent degradation and `opener = None` suppresses the viewer, so a
/// test can exercise both paths without depending on the host's `PATH`.
pub fn render_with(
    graph: &Graph,
    state_dir: &Path,
    dot: Option<&Path>,
    opener: Option<&OsStr>,
) -> Result<RenderOutcome, RenderError> {
    let dir = state_dir.join("multiway");
    fs::create_dir_all(&dir).map_err(RenderError::Io)?;
    let dot_src = graph.to_dot();

    let Some(dot_bin) = dot else {
        let dot_path = dir.join("graph.dot");
        fs::write(&dot_path, dot_src).map_err(RenderError::Io)?;
        return Ok(RenderOutcome::DotOnly(dot_path));
    };

    let image = dir.join("graph.png");
    // Placeholder → open → overwrite: the viewer shows a window at once and
    // reloads when `dot` replaces the file in place.
    fs::write(&image, PLACEHOLDER_PNG).map_err(RenderError::Io)?;
    if let Some(program) = opener {
        Command::new(program)
            .arg(&image)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(RenderError::Open)?;
    }
    render_png(dot_bin, &dot_src, &image)?;
    Ok(RenderOutcome::Image(image))
}

/// Pipe `dot_src` into `dot -Tpng -o out`, overwriting `out` with the render.
/// `-o` makes `dot` write the file itself, so stdout is unused and stderr is
/// captured for the error message.
fn render_png(dot_bin: &Path, dot_src: &str, out: &Path) -> Result<(), RenderError> {
    let mut child = Command::new(dot_bin)
        .arg("-Tpng")
        .arg("-o")
        .arg(out)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(RenderError::Io)?;
    child
        .stdin
        .take()
        .ok_or_else(|| RenderError::Io(io::Error::other("dot stdin was not captured")))?
        .write_all(dot_src.as_bytes())
        .map_err(RenderError::Io)?;
    let output = child.wait_with_output().map_err(RenderError::Io)?;
    if output.status.success() {
        Ok(())
    } else {
        Err(RenderError::Dot(
            String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        ))
    }
}

/// A tiny `which`: the first `PATH` entry that holds an executable named
/// `program`, or `None`. Avoids pulling in a third-party crate.
fn locate_on_path(program: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(program))
        .find(|candidate| candidate.is_file())
}

/// The OS "open this file in its default app" launcher, spawned directly (never
/// through a shell). `start` is a `cmd.exe` builtin rather than an executable, so
/// Windows uses `explorer`, which opens a file in its default handler.
fn platform_opener() -> OsString {
    let program = if cfg!(target_os = "macos") {
        "open"
    } else if cfg!(windows) {
        "explorer"
    } else {
        "xdg-open"
    };
    OsString::from(program)
}
