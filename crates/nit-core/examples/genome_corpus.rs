//! Corpus harness for the GoL-fitness novelty experiment.
//!
//! Walks one or more root directories, samples source files per language,
//! runs the REAL `nit_core::compute_genome_report` on each (Conway B3/S23,
//! MAX_GENERATIONS=3000, the production path), and writes one JSON Lines
//! record per file: the full `GenomeReport` (per-encoder generations
//! survived, density, components, peak population, cycle period, growth
//! class, tier, cross-encoder consistency, parsimony block, function
//! scores) plus path / language / size metadata.
//!
//! The downstream analysis asks the reviewer's question: does
//! `generations_survived` carry signal independent of the scalar code
//! metrics that feed the encoders, or does it merely re-encode them?
//!
//! Usage:
//!   cargo run -p nit-core --release --example genome_corpus -- \
//!       <out.jsonl> <root1> [root2 ...]
//!
//! Env knobs:
//!   GC_EXTS          comma list of extensions (default "rs,py,ts,tsx,go,js")
//!   GC_MAX_PER_LANG  cap files per language, strided sample (default 0 = all)
//!   GC_MIN_BYTES     skip files smaller than this (default 64)
//!   GC_MAX_BYTES     skip files larger than this (default 262144 = 256 KiB)
//!   GC_THREADS       worker threads (default = available_parallelism)

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use serde::Serialize;

use nit_core::compute_genome_report;
use nit_core::GenomeReport;

#[derive(Serialize)]
struct Row {
    /// Stable, release-safe id: first 12 hex of blake3(path). Lets the
    /// published CSV reference a file without leaking private paths.
    id: String,
    /// Full path — kept for the LOCAL manifest only; the analysis layer
    /// drops it from anything released.
    path: String,
    lang: String,
    bytes: usize,
    lines: usize,
    is_test: bool,
    #[serde(flatten)]
    report: GenomeReport,
}

fn lang_for_ext(ext: &str) -> Option<&'static str> {
    Some(match ext {
        "rs" => "rust",
        "py" => "python",
        "ts" | "tsx" => "typescript",
        "go" => "go",
        "js" | "jsx" | "mjs" | "cjs" => "javascript",
        _ => return None,
    })
}

/// Directory components we never descend into (build output, deps, VCS,
/// caches). Any path containing one of these as a component is skipped.
const DIR_DENY: &[&str] = &[
    "target",
    "node_modules",
    "vendor",
    "__pycache__",
    ".venv",
    "venv",
    "site-packages",
    "dist",
    "build",
    ".next",
    ".cache",
    ".mypy_cache",
    ".pytest_cache",
    ".tox",
    "coverage",
    "Pods",
    "DerivedData",
];

/// File-name suffixes/markers that indicate generated or minified code,
/// which the proposal's corpus protocol excludes.
fn is_generated_name(name: &str) -> bool {
    name.ends_with(".d.ts")
        || name.ends_with(".min.js")
        || name.ends_with(".min.ts")
        || name.ends_with("_pb2.py")
        || name.ends_with("_pb2_grpc.py")
        || name.ends_with(".pb.go")
        || name.ends_with("_pb.go")
        || name.ends_with(".gen.go")
        || name.ends_with("_generated.go")
        || name.contains(".generated.")
}

fn is_test_path(path: &Path) -> bool {
    let p = path.to_string_lossy();
    p.contains("/tests/")
        || p.contains("/test/")
        || p.contains("/__tests__/")
        || p.contains("_test.")
        || p.contains(".test.")
        || p.contains(".spec.")
        || p.contains("/spec/")
        || p.contains("test_")
}

fn collect(root: &Path, allowed: &[String], out: &mut Vec<(PathBuf, &'static str)>) {
    let entries = match fs::read_dir(root) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(ft) = entry.file_type() else { continue };
        let name = entry.file_name().to_string_lossy().to_string();
        if ft.is_dir() {
            // Skip denied dirs and hidden dirs (.git, .idea, ...).
            if name.starts_with('.') || DIR_DENY.contains(&name.as_str()) {
                continue;
            }
            collect(&path, allowed, out);
        } else if ft.is_file() {
            if name.starts_with('.') || is_generated_name(&name) {
                continue;
            }
            let Some(ext) = path.extension().and_then(|e| e.to_str()) else {
                continue;
            };
            if !allowed.iter().any(|a| a == ext) {
                continue;
            }
            if let Some(lang) = lang_for_ext(ext) {
                out.push((path, lang));
            }
        }
    }
}

fn env_usize(key: &str, default: usize) -> usize {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        eprintln!("usage: genome_corpus <out.jsonl> <root1> [root2 ...]");
        std::process::exit(2);
    }
    let out_path = PathBuf::from(&args[1]);
    let roots: Vec<PathBuf> = args[2..].iter().map(PathBuf::from).collect();

    let exts: Vec<String> = std::env::var("GC_EXTS")
        .unwrap_or_else(|_| "rs,py,ts,tsx,go,js".to_string())
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    let max_per_lang = env_usize("GC_MAX_PER_LANG", 0);
    let min_bytes = env_usize("GC_MIN_BYTES", 64);
    let max_bytes = env_usize("GC_MAX_BYTES", 262_144);
    let threads = env_usize(
        "GC_THREADS",
        std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4),
    )
    .max(1);

    // 1. Collect candidate files.
    let mut files: Vec<(PathBuf, &'static str)> = Vec::new();
    for root in &roots {
        collect(root, &exts, &mut files);
    }
    // Dedup + deterministic order.
    files.sort();
    files.dedup();

    // 2. Size filter.
    files.retain(|(p, _)| match fs::metadata(p) {
        Ok(m) => {
            let len = m.len() as usize;
            len >= min_bytes && len <= max_bytes
        }
        Err(_) => false,
    });

    // 3. Per-language strided sampling (deterministic; spreads across dirs
    //    because the list is path-sorted).
    if max_per_lang > 0 {
        let mut by_lang: std::collections::BTreeMap<&'static str, Vec<(PathBuf, &'static str)>> =
            Default::default();
        for f in files.drain(..) {
            by_lang.entry(f.1).or_default().push(f);
        }
        for (_, group) in by_lang.iter_mut() {
            if group.len() > max_per_lang {
                let stride = group.len() / max_per_lang;
                let sampled: Vec<_> = group
                    .iter()
                    .step_by(stride.max(1))
                    .take(max_per_lang)
                    .cloned()
                    .collect();
                *group = sampled;
            }
        }
        for (_, group) in by_lang {
            files.extend(group);
        }
        files.sort();
    }

    eprintln!(
        "genome_corpus: {} files, {} threads, gens=3000 (production)",
        files.len(),
        threads
    );
    {
        let mut counts: std::collections::BTreeMap<&str, usize> = Default::default();
        for (_, l) in &files {
            *counts.entry(l).or_default() += 1;
        }
        eprintln!("  by language: {counts:?}");
    }

    // 4. Parallel compute. Each thread handles a contiguous chunk so output
    //    order is deterministic after we concatenate in chunk order.
    let done = AtomicUsize::new(0);
    let total = files.len();
    let chunk_size = total.div_ceil(threads).max(1);
    let chunks: Vec<&[(PathBuf, &'static str)]> = files.chunks(chunk_size).collect();

    let results: Vec<Vec<String>> = std::thread::scope(|scope| {
        let handles: Vec<_> = chunks
            .iter()
            .map(|chunk| {
                let done = &done;
                scope.spawn(move || {
                    let mut lines = Vec::with_capacity(chunk.len());
                    for (path, lang) in chunk.iter() {
                        let Ok(text) = fs::read_to_string(path) else {
                            continue;
                        };
                        let report = compute_genome_report(&text, path);
                        let row = Row {
                            id: short_id(path),
                            path: path.to_string_lossy().to_string(),
                            lang: (*lang).to_string(),
                            bytes: text.len(),
                            lines: text.lines().count(),
                            is_test: is_test_path(path),
                            report,
                        };
                        if let Ok(line) = serde_json::to_string(&row) {
                            lines.push(line);
                        }
                        let n = done.fetch_add(1, Ordering::Relaxed) + 1;
                        if n % 50 == 0 || n == total {
                            eprintln!("  {n}/{total}");
                        }
                    }
                    lines
                })
            })
            .collect();
        handles.into_iter().map(|h| h.join().unwrap()).collect()
    });

    // 5. Write JSONL.
    let mut buf = String::new();
    for chunk_lines in &results {
        for line in chunk_lines {
            buf.push_str(line);
            buf.push('\n');
        }
    }
    fs::write(&out_path, buf).expect("write output");
    let written: usize = results.iter().map(|c| c.len()).sum();
    eprintln!(
        "genome_corpus: wrote {written} records -> {}",
        out_path.display()
    );
}

fn short_id(path: &Path) -> String {
    let h = blake3::hash(path.to_string_lossy().as_bytes());
    h.as_bytes()[..6]
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
