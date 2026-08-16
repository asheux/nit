//! The real [`nit_multiway::Valuer`]: a genome quality score (from
//! [`nit_multiway::node_value`]) combined with the hard build/test gates run in
//! the worktree. The pure genome scalar lives in `nit_multiway`; this adapter
//! adds the gate half, which needs nit-tui's [`GateBundle`] machinery and so
//! cannot live in the pure engine crate (that would cycle `nit-multiway -> nit-tui`).

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use std::{fmt, fs, io};

use nit_core::compute_genome_report;
use nit_core::languages::detect_by_path;
use nit_multiway::valuer::node_value;
use nit_multiway::{Value, Valuer};

use crate::swarm::GateBundle;

/// Hard ceiling per gate before it is killed and treated as a failure. Generous
/// because a real `cargo test --workspace` is the gate; tests inject instant
/// `true` / `false` commands and never reach it.
const GATE_TIMEOUT: Duration = Duration::from_secs(600);
const GATE_POLL: Duration = Duration::from_millis(50);
/// Bounds on the worktree walk so a node valuation can't fan out unboundedly on
/// a large tree.
const MAX_SOURCE_FILES: usize = 512;
const MAX_WALK_DEPTH: usize = 16;

/// Values a worktree by genome quality and a hard gate verdict. Construct with
/// [`GenomeValuer::for_tree`] in production (detects the language gate bundle) or
/// [`GenomeValuer::new`] with explicit gate command lines (tests inject
/// deterministic commands so valuation is not a flaky multi-second spawn).
pub struct GenomeValuer {
    gate_commands: Vec<String>,
}

impl GenomeValuer {
    /// Each command is whitespace-split into an argv and spawned with NO shell.
    pub fn new(gate_commands: Vec<String>) -> Self {
        Self { gate_commands }
    }

    /// Select the build/test gates for `tree` from the detected language bundle.
    /// `swarm::run_effective_gates` is the swarm's selection entry point but
    /// needs `AppState` / `SwarmRun` context a pure tree valuer can't supply, so
    /// we read straight from [`GateBundle::detect`]. The `Genome` pseudo-bundle is
    /// skipped: its command is a local-eval sentinel, never spawned, and the
    /// genome is already folded into `score`.
    pub fn for_tree(tree: &Path) -> Self {
        let gate_commands = match GateBundle::detect(tree).bundle {
            Some(GateBundle::Genome) | None => Vec::new(),
            Some(bundle) => bundle
                .gates()
                .into_iter()
                .map(|gate| gate.command)
                .collect(),
        };
        Self::new(gate_commands)
    }
}

impl Valuer for GenomeValuer {
    type Error = ValuerError;

    fn value(&self, tree: &Path) -> Result<Value, Self::Error> {
        let mut reports = Vec::new();
        let mut conflicted = false;
        for path in source_files(tree)? {
            let text = fs::read_to_string(&path).map_err(|source| ValuerError::Io {
                path: path.clone(),
                source,
            })?;
            // A tree still carrying both Git conflict bookends is an unresolved
            // (dead) merge; demanding the open and the close marker together rules
            // out a lone marker quoted inside a string. Seeding `gated` with this
            // withholds a botched judge adjudication from the frontier even when no
            // build gate bundle is detected for the language.
            conflicted |= text.lines().any(|l| l.starts_with("<<<<<<< "))
                && text.lines().any(|l| l.starts_with(">>>>>>> "));
            reports.push(compute_genome_report(&text, &path));
        }
        let (score, tier) = node_value(&reports);

        let mut gated = conflicted;
        for command in &self.gate_commands {
            if !run_gate(command, tree)? {
                gated = true;
                break;
            }
        }
        Ok(Value { gated, score, tier })
    }
}

/// Depth/symlink/count-bounded walk collecting the code files under `tree`
/// (markup / data files are skipped — they carry no genome signal). Sorted for
/// deterministic valuation across runs.
fn source_files(tree: &Path) -> Result<Vec<PathBuf>, ValuerError> {
    let mut found = Vec::new();
    let mut stack = vec![(tree.to_path_buf(), 0usize)];
    while let Some((dir, depth)) = stack.pop() {
        if depth > MAX_WALK_DEPTH || found.len() >= MAX_SOURCE_FILES {
            continue;
        }
        let entries = fs::read_dir(&dir).map_err(|source| ValuerError::Io {
            path: dir.clone(),
            source,
        })?;
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if file_type.is_symlink() {
                continue;
            }
            if file_type.is_dir() {
                if !is_noise_dir(&path) {
                    stack.push((path, depth + 1));
                }
            } else if detect_by_path(&path).is_some_and(|lang| lang.is_code) {
                found.push(path);
            }
        }
    }
    found.sort();
    Ok(found)
}

fn is_noise_dir(path: &Path) -> bool {
    match path.file_name().and_then(|name| name.to_str()) {
        Some(name) => name == "target" || name == "node_modules" || name.starts_with('.'),
        None => true,
    }
}

/// De-dupes the gate-spawn-failure warning to a single line per process: a tool
/// missing from every worktree fails its node identically each time, so warning
/// once is enough. A lone flag, not a per-program registry — the message is an
/// operator-config nudge, not an audit trail.
static GATE_SPAWN_WARNED: AtomicBool = AtomicBool::new(false);

/// Spawn one gate (argv-split, no shell) in `tree` and return whether it exited
/// 0. A non-zero exit, a timeout, OR a program that cannot be spawned (ENOENT) is
/// a gate failure (`Ok(false)`): a tool absent from a clean worktree — e.g. bare
/// `python` on macOS — gates its node conservatively instead of aborting the whole
/// search on its first valuation. Only a `wait` failure on a child that *did*
/// spawn is a genuine OS-level infra error and still propagates.
fn run_gate(command: &str, tree: &Path) -> Result<bool, ValuerError> {
    let mut argv = command.split_whitespace();
    let Some(program) = argv.next() else {
        return Ok(true);
    };
    let mut child = match Command::new(program)
        .args(argv)
        .current_dir(tree)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .env("GIT_TERMINAL_PROMPT", "0")
        .spawn()
    {
        Ok(child) => child,
        Err(source) => {
            if !GATE_SPAWN_WARNED.swap(true, Ordering::Relaxed) {
                tracing::warn!(%command, %source, "multiway gate program could not be spawned; gating node");
            }
            return Ok(false);
        }
    };

    let deadline = Instant::now() + GATE_TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Ok(status.success()),
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return Ok(false);
            }
            Ok(None) => std::thread::sleep(GATE_POLL),
            Err(source) => {
                return Err(ValuerError::Gate {
                    command: command.to_string(),
                    source,
                })
            }
        }
    }
}

#[derive(Debug)]
pub enum ValuerError {
    /// Reading the worktree or one of its source files failed.
    Io { path: PathBuf, source: io::Error },
    /// Awaiting an already-spawned gate child failed — a genuine OS-level error.
    /// A program that cannot be spawned at all yields `Ok(false)` instead, gating
    /// the node rather than aborting the search.
    Gate { command: String, source: io::Error },
}

impl fmt::Display for ValuerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ValuerError::Io { path, source } => write!(f, "reading {}: {source}", path.display()),
            ValuerError::Gate { command, source } => write!(f, "gate `{command}`: {source}"),
        }
    }
}

impl std::error::Error for ValuerError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            ValuerError::Io { source, .. } | ValuerError::Gate { source, .. } => Some(source),
        }
    }
}
