//! Exercise the real Phase 6a render path on a persisted DAG: load a
//! `<mission>.json`, call `multiway::render::render_and_open` (the exact code the
//! `@multiway-graph` command runs), and open the resulting PNG in the OS viewer.
//!
//!   cargo run -p nit-tui --example dag_render -- <mission.json> [state_dir]

use std::path::PathBuf;

use nit_multiway::graph::Graph;
use nit_tui::multiway::render::{render_and_open, RenderOutcome};

fn main() {
    let json_path = match std::env::args().nth(1) {
        Some(p) => p,
        None => {
            eprintln!("usage: dag_render <mission.json> [state_dir]");
            std::process::exit(2);
        }
    };
    let state_dir = std::env::args()
        .nth(2)
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("mw-render"));

    let json = std::fs::read_to_string(&json_path).expect("read mission json");
    let graph: Graph = serde_json::from_str(&json).expect("parse Graph json");

    match render_and_open(&graph, &state_dir) {
        Ok(RenderOutcome::Image(path)) => println!("rendered + opened: {}", path.display()),
        Ok(RenderOutcome::DotOnly(path)) => {
            println!("graphviz absent; saved DOT only: {}", path.display())
        }
        Err(err) => {
            eprintln!("render failed: {err}");
            std::process::exit(1);
        }
    }
}
