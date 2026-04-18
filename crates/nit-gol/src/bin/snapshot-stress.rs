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

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    let params = StressParams::from_args(&args);

    let stack_bytes = resolve_stack_bytes();
    let handle = thread::Builder::new()
        .name("snapshot-stress".into())
        .stack_size(stack_bytes)
        .spawn(move || run_stress(&params))
        .expect("spawn snapshot stress thread");

    if let Err(err) = handle.join().expect("join snapshot stress thread") {
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
    /// Parse positional CLI arguments, falling back to default values
    /// for any that are missing or fail to parse.
    fn from_args(args: &[String]) -> Self {
        let d = Self::default();
        Self {
            width: parse_arg(args, 0).unwrap_or(d.width),
            height: parse_arg(args, 1).unwrap_or(d.height),
            iterations: parse_arg(args, 2).unwrap_or(d.iterations),
            dir: args.get(3).map(PathBuf::from).unwrap_or(d.dir),
            max_files: parse_arg(args, 4).unwrap_or(d.max_files),
            seed: parse_arg(args, 5).unwrap_or(d.seed),
        }
    }
}

fn parse_arg<T: std::str::FromStr>(args: &[String], idx: usize) -> Option<T> {
    args.get(idx).and_then(|s| s.parse().ok())
}

/// Generate a deterministic alive/dead pattern from `(x, y, seed)`.
///
/// The mixing function is a cheap multiplicative hash — not
/// cryptographic, but reproducible across runs with the same seed so
/// that stress-test snapshots are byte-stable.
#[must_use]
fn generate_seed_grid(width: usize, height: usize, seed: u64) -> Grid {
    let mut grid = Grid::new(width, height);
    for y in 0..height {
        for x in 0..width {
            let mix = (x as u64).wrapping_mul(31) ^ (y as u64).wrapping_mul(17) ^ seed;
            grid.set(x, y, mix % 7 < 2);
        }
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

    for iteration in 0..params.iterations {
        let name_base = format!("stress-{iteration:05}");
        let meta = build_meta(rule, &grid, params.seed, iteration);
        write_snapshot(&params.dir, &name_base, &grid, rule, &meta)?;
        if params.max_files > 0 {
            prune_oldest(&params.dir, params.max_files)?;
        }
    }
    Ok(())
}

/// Resolve the worker thread stack size from environment.
///
/// Checks `NIT_GOL_IO_STACK_MB` first, falls back to `NIT_GOL_STACK_MB`,
/// then defaults to 256 MB with a 32 MB minimum so snapshot writes never
/// starve for stack.
fn resolve_stack_bytes() -> usize {
    let from_env = env::var("NIT_GOL_IO_STACK_MB")
        .or_else(|_| env::var("NIT_GOL_STACK_MB"))
        .ok()
        .and_then(|value| value.parse::<usize>().ok());
    let mb = from_env.unwrap_or(DEFAULT_STACK_MB).max(MIN_STACK_MB);
    mb.saturating_mul(1024 * 1024)
}
