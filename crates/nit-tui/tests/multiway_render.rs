//! Phase 6a render tests: the DOT a fixture DAG produces is well-formed and gilds
//! the kept path through to the accepted solution; a missing graphviz degrades to a
//! saved `.dot` path (never an error); and when `dot` is present the renderer writes
//! a PNG placeholder, spawns the platform opener on it, then overwrites it in place
//! with the final render.

use std::path::{Path, PathBuf};

use nit_core::{GenomeTier, SubstrateState};
use nit_multiway::edge::{Edge, EdgeKind};
use nit_multiway::graph::Graph;
use nit_multiway::node::{CommitRef, Node, NodeId, NodeStatus};
use nit_multiway::Value;

use nit_tui::multiway::render::{render_with, RenderOutcome};

/// A root that forks into a kept solution and a gate-failed dead end — enough to
/// exercise node statements, parent→child edges, and both valued/unvalued labels.
fn sample_dag() -> Graph {
    let mut graph = Graph::default();
    let root = NodeId::new("root0000");
    let win = NodeId::new("win11111");
    let dead = NodeId::new("dead2222");

    graph.add_node(root.clone(), Node::open(CommitRef("c0".into()), vec![]));
    graph.add_node(
        win.clone(),
        Node::child(
            CommitRef("c1".into()),
            Value {
                gated: false,
                score: 0.91,
                tier: GenomeTier::Replicator,
            },
            NodeStatus::Solution,
            root.clone(),
            SubstrateState::default(),
        ),
    );
    graph.add_node(
        dead.clone(),
        Node::child(
            CommitRef("c2".into()),
            Value {
                gated: true,
                score: 0.10,
                tier: GenomeTier::StillLife,
            },
            NodeStatus::GateFailed,
            root.clone(),
            SubstrateState::default(),
        ),
    );
    graph.add_edge(Edge {
        from: root.clone(),
        to: win,
        kind: EdgeKind::Turn,
    });
    graph.add_edge(Edge {
        from: root,
        to: dead,
        kind: EdgeKind::Fork,
    });
    graph
}

/// A three-deep kept lineage (root → step → solution) beside a gated dead-end
/// fork. Unlike `sample_dag`'s single hop, this forces the renderer to gild a
/// *multi-hop* path and stop the highlight at the gated branch — the "image
/// content matches the run" acceptance.
fn deep_dag() -> Graph {
    let mut graph = Graph::default();
    let root = NodeId::new("root0000");
    let step = NodeId::new("step1111");
    let soln = NodeId::new("soln2222");
    let dead = NodeId::new("dead3333");

    graph.add_node(root.clone(), Node::open(CommitRef("c0".into()), vec![]));
    graph.add_node(
        step.clone(),
        Node::child(
            CommitRef("c1".into()),
            Value {
                gated: false,
                score: 0.55,
                tier: GenomeTier::Spaceship,
            },
            NodeStatus::Expanded,
            root.clone(),
            SubstrateState::default(),
        ),
    );
    graph.add_node(
        soln.clone(),
        Node::child(
            CommitRef("c2".into()),
            Value {
                gated: false,
                score: 0.95,
                tier: GenomeTier::Replicator,
            },
            NodeStatus::Solution,
            step.clone(),
            SubstrateState::default(),
        ),
    );
    graph.add_node(
        dead.clone(),
        Node::child(
            CommitRef("c3".into()),
            Value {
                gated: true,
                score: 0.10,
                tier: GenomeTier::StillLife,
            },
            NodeStatus::GateFailed,
            root.clone(),
            SubstrateState::default(),
        ),
    );
    graph.add_edge(Edge {
        from: root.clone(),
        to: step.clone(),
        kind: EdgeKind::Turn,
    });
    graph.add_edge(Edge {
        from: step,
        to: soln,
        kind: EdgeKind::Turn,
    });
    graph.add_edge(Edge {
        from: root,
        to: dead,
        kind: EdgeKind::Fork,
    });
    graph
}

fn scratch_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("{tag}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

/// The single `"from" -> "to"` statement line from a DOT document, so a test can
/// assert on one edge's attributes (e.g. the gold kept-path highlight) instead of
/// scanning the whole graph.
fn edge_stmt<'a>(dot: &'a str, from: &str, to: &str) -> &'a str {
    dot.lines()
        .find(|line| line.contains(&format!("\"{from}\" -> \"{to}\"")))
        .unwrap_or_else(|| panic!("edge {from}->{to} absent:\n{dot}"))
}

#[test]
fn to_dot_emits_a_wellformed_dag() {
    let dot = sample_dag().to_dot();

    assert!(
        dot.starts_with("digraph multiway {"),
        "missing header:\n{dot}"
    );
    assert!(
        dot.trim_end().ends_with('}'),
        "missing closing brace:\n{dot}"
    );
    for id in ["root0000", "win11111", "dead2222"] {
        assert!(
            dot.contains(&format!("\"{id}\"")),
            "node {id} absent:\n{dot}"
        );
    }
    assert!(
        dot.contains("\"root0000\" -> \"win11111\""),
        "turn edge absent:\n{dot}"
    );
    assert!(
        dot.contains("\"root0000\" -> \"dead2222\""),
        "fork edge absent:\n{dot}"
    );
    // Exactly the two real edges produce `->`; labels and ids carry none.
    assert_eq!(
        dot.matches("->").count(),
        2,
        "unexpected edge count:\n{dot}"
    );
}

#[test]
fn rendered_dot_marks_the_kept_path_through_to_the_solution() {
    let dot = deep_dag().to_dot();

    // All four search nodes are rendered, so the image reflects the run's size.
    assert_eq!(
        dot.matches("shape=").count(),
        4,
        "node count drifted:\n{dot}"
    );

    // The kept lineage root → step → solution is gilded hop by hop, while the gated
    // dead-end fork is left out — the highlight tracks the path the search settled
    // on, not every branch it explored.
    assert!(
        edge_stmt(&dot, "root0000", "step1111").contains("#d4af37"),
        "root → step should be on the gold kept path:\n{dot}"
    );
    assert!(
        edge_stmt(&dot, "step1111", "soln2222").contains("#d4af37"),
        "step → solution should be on the gold kept path:\n{dot}"
    );
    assert!(
        !edge_stmt(&dot, "root0000", "dead3333").contains("#d4af37"),
        "the gated fork must not be gilded:\n{dot}"
    );

    // …and the path ends at the single accepted solution, drawn as a doublecircle.
    assert_eq!(
        dot.matches("shape=doublecircle").count(),
        1,
        "exactly one accepted-solution terminal:\n{dot}"
    );
}

#[test]
fn graphviz_absent_degrades_to_saved_dot_path() {
    let base = scratch_dir("nit_mw_render_dotonly");
    let graph = sample_dag();

    let outcome = render_with(&graph, &base, None, None).expect("degrade, never error");
    let RenderOutcome::DotOnly(path) = outcome else {
        panic!("expected DotOnly when dot is absent");
    };

    assert!(path.exists(), "saved .dot should exist: {}", path.display());
    assert!(
        path.starts_with(base.join("multiway")),
        "saved under <state_dir>/multiway"
    );
    let saved = std::fs::read_to_string(&path).expect("read saved dot");
    assert_eq!(saved, graph.to_dot(), "saved file is the DAG's DOT source");

    let _ = std::fs::remove_dir_all(&base);
}

#[cfg(unix)]
#[test]
fn opener_is_spawned_on_the_rendered_image() {
    let base = scratch_dir("nit_mw_render_open");
    std::fs::create_dir_all(&base).expect("create scratch dir");
    let marker = base.join("opened.txt");

    // Fake `dot`: read DOT from stdin and write it to the `-o` target, so the
    // image path is a real, finished file when the opener fires.
    let fake_dot = base.join("fake-dot");
    write_script(&fake_dot, "#!/bin/sh\nout=\"$3\"\ncat > \"$out\"\n");
    // Fake opener: record the path it was launched with.
    let fake_open = base.join("fake-open");
    write_script(
        &fake_open,
        &format!("#!/bin/sh\nprintf '%s' \"$1\" > '{}'\n", marker.display()),
    );

    let outcome = render_with(
        &sample_dag(),
        &base,
        Some(fake_dot.as_path()),
        Some(fake_open.as_os_str()),
    )
    .expect("render with fake dot");
    let RenderOutcome::Image(image) = outcome else {
        panic!("expected Image when dot is present");
    };
    assert!(
        image.exists(),
        "rendered image should exist: {}",
        image.display()
    );

    // The opener runs detached; wait briefly for its marker to appear.
    let recorded = wait_for_file(&marker).expect("opener marker never appeared");
    assert_eq!(
        recorded.trim(),
        image.to_str().expect("utf-8 image path"),
        "opener was launched on the rendered image"
    );

    let _ = std::fs::remove_dir_all(&base);
}

#[cfg(unix)]
#[test]
fn placeholder_is_written_then_overwritten_by_the_final_render() {
    let base = scratch_dir("nit_mw_render_overwrite");
    std::fs::create_dir_all(&base).expect("create scratch dir");
    let captured = base.join("captured.bin");

    // Fake `dot`: snapshot whatever is already at the `-o` target (the placeholder
    // the renderer wrote before spawning us) into `captured`, then overwrite it
    // with our stdin — the real DOT. `render_png` blocks on us, so both files are
    // settled by the time `render_with` returns: no timing race to observe.
    let fake_dot = base.join("fake-dot");
    write_script(
        &fake_dot,
        &format!(
            "#!/bin/sh\nout=\"$3\"\ncp \"$out\" '{}'\ncat > \"$out\"\n",
            captured.display()
        ),
    );

    let graph = sample_dag();
    let outcome =
        render_with(&graph, &base, Some(fake_dot.as_path()), None).expect("render with fake dot");
    let RenderOutcome::Image(image) = outcome else {
        panic!("expected Image when dot is present");
    };

    // What `dot` saw on entry was a real PNG placeholder, not the final graph — the
    // viewer therefore has a valid image to open before the render finishes.
    let placeholder = std::fs::read(&captured).expect("read captured placeholder");
    assert!(
        placeholder.starts_with(b"\x89PNG\r\n\x1a\n"),
        "the target held a PNG placeholder before the final render"
    );

    // …and it was overwritten in place with the DAG's DOT, so the open window
    // reloads from the loading placeholder to the finished graph.
    let rendered = std::fs::read(&image).expect("read rendered image");
    assert_eq!(
        rendered,
        graph.to_dot().into_bytes(),
        "final render replaced the placeholder with the DAG"
    );
    assert_ne!(
        placeholder, rendered,
        "placeholder was replaced, not appended"
    );

    let _ = std::fs::remove_dir_all(&base);
}

#[cfg(unix)]
fn write_script(path: &Path, body: &str) {
    use std::os::unix::fs::PermissionsExt;

    std::fs::write(path, body).expect("write fixture script");
    let mut perms = std::fs::metadata(path).expect("stat script").permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(path, perms).expect("chmod script");
}

#[cfg(unix)]
fn wait_for_file(path: &Path) -> Option<String> {
    for _ in 0..100 {
        if let Ok(contents) = std::fs::read_to_string(path) {
            if !contents.is_empty() {
                return Some(contents);
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    None
}
