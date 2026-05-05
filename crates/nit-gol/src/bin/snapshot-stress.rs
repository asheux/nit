//! Stress-test binary for the snapshot writing pipeline.
//!
//! Generates a series of synthetic grids and writes them as snapshots,
//! exercising the I/O path, pruning logic, and RLE encoding under
//! configurable grid sizes and iteration counts.
//!
//! Usage: `snapshot-stress [width] [height] [iterations] [dir] [max_files] [seed]`

use std::env;
use std::path::PathBuf;
use std::thread;

use nit_gol::snapshot::{now_iso8601, prune_oldest, write_snapshot, SnapshotMetadata};
use nit_gol::{Grid, Rule};

const DEFAULT_STACK_MB: usize = 256;
const MIN_STACK_MB: usize = 32;
const BYTES_PER_MIB: usize = 1024 * 1024;
const STACK_ENV_PRIMARY: &str = "NIT_GOL_IO_STACK_MB";
const STACK_ENV_FALLBACK: &str = "NIT_GOL_STACK_MB";
const WORKER_THREAD_NAME: &str = "snapshot-stress";

// Mixing constants for the deterministic seed hash — chosen for coprimality
// with 7 so the `< 2` predicate gives a ~28% alive density.
const MIX_X_PRIME: u64 = 31;
const MIX_Y_PRIME: u64 = 17;
const MIX_MODULUS: u64 = 7;
const MIX_ALIVE_CUTOFF: u64 = 2;

fn main() {
    let argv: Vec<String> = env::args().skip(1).collect();
    let params = StressParams::from_args(&argv);
    let stack_bytes = resolve_stack_bytes();

    let worker = thread::Builder::new()
        .name(WORKER_THREAD_NAME.into())
        .stack_size(stack_bytes)
        .spawn(move || run_stress(&params))
        .expect("spawn snapshot stress thread");

    if let Err(err) = worker.join().expect("join snapshot stress thread") {
        eprintln!("snapshot-stress error: {err}");
        std::process::exit(1);
    }
}

struct StressParams {
    width: usize,
    height: usize,
    iterations: usize,
    dir: PathBuf,
    max_files: usize,
    seed: u64,
}

impl Default for StressParams {
    fn default() -> Self {
        Self {
            width: 120,
            height: 40,
            iterations: 50,
            dir: PathBuf::from("/tmp/nit-snapshot-stress"),
            max_files: 500,
            seed: 1,
        }
    }
}

impl StressParams {
    fn from_args(argv: &[String]) -> Self {
        let fallback = Self::default();
        Self {
            width: pos(argv, 0).unwrap_or(fallback.width),
            height: pos(argv, 1).unwrap_or(fallback.height),
            iterations: pos(argv, 2).unwrap_or(fallback.iterations),
            dir: argv.get(3).map(PathBuf::from).unwrap_or(fallback.dir),
            max_files: pos(argv, 4).unwrap_or(fallback.max_files),
            seed: pos(argv, 5).unwrap_or(fallback.seed),
        }
    }
}

fn pos<T: std::str::FromStr>(argv: &[String], idx: usize) -> Option<T> {
    argv.get(idx).and_then(|raw| raw.parse().ok())
}

/// Cheap reproducible hash — not cryptographic, but stable across runs so
/// stress snapshots stay byte-identical.
#[inline]
fn alive_cell_at(col: usize, row: usize, seed: u64) -> bool {
    let mixed =
        (col as u64).wrapping_mul(MIX_X_PRIME) ^ (row as u64).wrapping_mul(MIX_Y_PRIME) ^ seed;
    mixed % MIX_MODULUS < MIX_ALIVE_CUTOFF
}

#[must_use]
fn generate_seed_grid(width: usize, height: usize, seed: u64) -> Grid {
    let mut grid = Grid::new(width, height);
    let cell_count = width.saturating_mul(height);
    for idx in 0..cell_count {
        let col = idx % width;
        let row = idx / width;
        grid.set(col, row, alive_cell_at(col, row, seed));
    }
    grid
}

fn build_meta(rule: Rule, grid: &Grid, seed: u64, iteration: usize) -> SnapshotMetadata {
    SnapshotMetadata {
        timestamp: now_iso8601(),
        seed_source: "stress".into(),
        seed_hash: seed,
        rule: rule.to_string(),
        generation: iteration as u64,
        alive_count: grid.alive_count(),
        wrap_mode: "dead".into(),
        tick_ms: 120,
        ..Default::default()
    }
}

fn run_stress(params: &StressParams) -> std::io::Result<()> {
    std::fs::create_dir_all(&params.dir)?;
    let rule = Rule::conway();
    let grid = generate_seed_grid(params.width, params.height, params.seed);
    let prune_cap = (params.max_files > 0).then_some(params.max_files);

    for iteration in 0..params.iterations {
        let stem = format!("stress-{iteration:05}");
        let meta = build_meta(rule, &grid, params.seed, iteration);
        write_snapshot(&params.dir, &stem, &grid, rule, &meta)?;
        if let Some(cap) = prune_cap {
            prune_oldest(&params.dir, cap)?;
        }
    }
    Ok(())
}

/// `NIT_GOL_IO_STACK_MB` wins, falls back to `NIT_GOL_STACK_MB`, then 256 MiB
/// with a 32 MiB floor so snapshot writes never starve for stack on large grids.
fn resolve_stack_bytes() -> usize {
    let requested = env::var(STACK_ENV_PRIMARY)
        .or_else(|_| env::var(STACK_ENV_FALLBACK))
        .ok()
        .and_then(|raw| raw.parse::<usize>().ok());
    let mib = requested.unwrap_or(DEFAULT_STACK_MB).max(MIN_STACK_MB);
    mib.saturating_mul(BYTES_PER_MIB)
}
