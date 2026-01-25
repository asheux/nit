use std::env;
use std::path::PathBuf;
use std::thread;

use nit_gol::snapshot::{now_iso8601, prune_oldest, write_snapshot, SnapshotMetadata};
use nit_gol::{Grid, Rule};

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    let width = args.get(0).and_then(|s| s.parse().ok()).unwrap_or(120usize);
    let height = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(40usize);
    let iterations = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(50usize);
    let dir = args
        .get(3)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp/nit-snapshot-stress"));
    let max_files = args.get(4).and_then(|s| s.parse().ok()).unwrap_or(500usize);
    let seed = args.get(5).and_then(|s| s.parse().ok()).unwrap_or(1u64);

    let stack_bytes = stack_bytes();
    let handle = thread::Builder::new()
        .name("snapshot-stress".into())
        .stack_size(stack_bytes)
        .spawn(move || run_stress(width, height, iterations, dir, max_files, seed))
        .expect("spawn snapshot stress thread");

    if let Err(err) = handle.join().expect("join snapshot stress thread") {
        eprintln!("snapshot-stress error: {err}");
        std::process::exit(1);
    }
}

fn run_stress(
    width: usize,
    height: usize,
    iterations: usize,
    dir: PathBuf,
    max_files: usize,
    seed: u64,
) -> std::io::Result<()> {
    std::fs::create_dir_all(&dir)?;
    let rule = Rule::conway();
    let mut grid = Grid::new(width, height);
    for y in 0..height {
        for x in 0..width {
            let v = ((x as u64).wrapping_mul(31)
                ^ (y as u64).wrapping_mul(17)
                ^ seed)
                % 7;
            grid.set(x, y, v < 2);
        }
    }

    for i in 0..iterations {
        let name_base = format!("stress-{i:05}");
        let meta = SnapshotMetadata {
            timestamp: now_iso8601(),
            workspace_root: None,
            file_path: None,
            seed_source: "stress".into(),
            seed_hash: seed,
            rule: rule.to_string(),
            generation: i as u64,
            alive_count: grid.alive_count(),
            period: None,
            score: None,
            wrap_mode: "dead".into(),
            tick_ms: 120,
            attractor: None,
        };
        write_snapshot(&dir, &name_base, &grid, rule, &meta)?;
        if max_files > 0 {
            prune_oldest(&dir, max_files)?;
        }
    }
    Ok(())
}

fn stack_bytes() -> usize {
    const DEFAULT_MB: usize = 256;
    const MIN_MB: usize = 32;
    let from_env = env::var("NIT_GOL_IO_STACK_MB")
        .or_else(|_| env::var("NIT_GOL_STACK_MB"))
        .ok()
        .and_then(|value| value.parse::<usize>().ok());
    let mb = from_env.unwrap_or(DEFAULT_MB).max(MIN_MB);
    mb.saturating_mul(1024 * 1024)
}
