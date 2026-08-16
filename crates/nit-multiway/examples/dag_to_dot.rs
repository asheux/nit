//! Load a persisted multiway DAG (`<mission>.json`) and print its Graphviz DOT —
//! the exact `Graph::to_dot` Phase 6a feeds to `dot -Tpng`. Lets you eyeball or
//! render a real run's graph without the TUI:
//!
//!   cargo run -p nit-multiway --example dag_to_dot -- <mission.json> | dot -Tpng -o graph.png

use nit_multiway::graph::Graph;

fn main() {
    let path = match std::env::args().nth(1) {
        Some(p) => p,
        None => {
            eprintln!("usage: dag_to_dot <mission.json>");
            std::process::exit(2);
        }
    };
    let json = std::fs::read_to_string(&path).expect("read mission json");
    let graph: Graph = serde_json::from_str(&json).expect("parse Graph json");
    print!("{}", graph.to_dot());
}
