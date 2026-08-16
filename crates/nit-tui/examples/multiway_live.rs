//! Headless live `@multiway` run: real Claude agents fork / value / merge a DAG
//! over a scratch repo — the exact stack `multiway::runtime::run_search` drives
//! from the TUI, minus the chat command (which can't be typed headlessly). The
//! operator tree is read-only; every turn runs in an isolated git worktree; the
//! DAG persists to `<state_dir>/multiway/<mission>.json`.
//!
//!   NIT_MULTIWAY=1 cargo run -p nit-tui --example multiway_live -- <repo> [model] [task...]
//!
//! A tiny budget (k=2, 2 turns) keeps it cheap: one real expansion (two agent
//! turns) plus the adjudicated merge of the two siblings. A wall-clock cap aborts
//! if an agent turn hangs, so it can never run forever.

use std::path::PathBuf;
use std::thread::sleep;
use std::time::{Duration, Instant};

use nit_tui::claude_runner::{ClaudeRunner, ClaudeRunnerConfig};
use nit_tui::multiway::{Budget, Mood, MultiwayEvent, MultiwayRuntime, SearchPolicy, Task};

const WALL_CAP: Duration = Duration::from_secs(720);

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let repo = PathBuf::from(args.get(1).map(String::as_str).unwrap_or("."));
    let model = args.get(2).cloned().unwrap_or_else(|| "sonnet".to_owned());
    let task_prompt = if args.len() > 3 {
        args[3..].join(" ")
    } else {
        "Add a /// doc comment to each public function in src/lib.rs, and add a \
         #[cfg(test)] module with one unit test. Keep the crate compiling."
            .to_owned()
    };

    let mission = "mw-live";
    let policy = SearchPolicy {
        mood: Mood::Explore,
        k: 2,
        budget: Budget {
            max_nodes: 8,
            max_turns: 2,
            max_tokens: None,
        },
    };
    let task = Task {
        prompt: task_prompt.clone(),
        role: "integrate".to_owned(),
    };

    eprintln!("repo   : {}", repo.display());
    eprintln!("model  : {model}");
    eprintln!(
        "policy : mood={:?} k={} budget={}turns/{}nodes",
        policy.mood, policy.k, policy.budget.max_turns, policy.budget.max_nodes
    );
    eprintln!("task   : {task_prompt}");
    eprintln!("--- starting real-agent search (this spawns claude turns) ---");

    // Mirror the real TUI (nit/src/bootstrap.rs): permission_mode = None routes
    // through `--allowedTools Read,Edit,Write,Bash,…`, which auto-approves the
    // editing tools headlessly. (A non-None value must be a CLI-valid mode —
    // acceptEdits/bypassPermissions/… — NOT the stale "dangerously-skip-permissions".)
    let config = ClaudeRunnerConfig {
        max_parallel_turns: 1,
        permission_mode: None,
    };
    let runner = ClaudeRunner::spawn(config);

    let mut rt = MultiwayRuntime::from_env();
    if let Err(err) = rt.start(mission, repo.clone(), runner, model, task, policy) {
        eprintln!("start failed: {err}");
        std::process::exit(1);
    }

    let started = Instant::now();
    loop {
        for ev in rt.poll(mission) {
            print_event(&ev, started);
        }
        if !rt.is_active(mission) {
            break;
        }
        if started.elapsed() > WALL_CAP {
            eprintln!("wall-clock cap ({WALL_CAP:?}) hit — aborting the search");
            rt.abort(mission);
            // Give the cancelled turn a moment to unwind, then stop.
            sleep(Duration::from_secs(3));
            break;
        }
        sleep(Duration::from_millis(400));
    }
    for ev in rt.poll(mission) {
        print_event(&ev, started);
    }
    rt.cleanup_finished();
    eprintln!(
        "--- search finished in {:?}; DAG at <state_dir>/multiway/{mission}.json ---",
        started.elapsed()
    );
}

fn print_event(ev: &MultiwayEvent, started: Instant) {
    let t = started.elapsed().as_secs();
    match ev {
        MultiwayEvent::Progress {
            nodes,
            turns,
            frontier,
            best_score,
            best_tier,
        } => eprintln!(
            "[{t:>3}s] progress: nodes={nodes} turns={turns} frontier={frontier} \
             best={best_score:.2} ({best_tier:?})"
        ),
        MultiwayEvent::Done(outcome) => eprintln!(
            "[{t:>3}s] DONE: stop={:?} expanded={} turns={} best={:?}",
            outcome.stop_reason, outcome.nodes_expanded, outcome.turns_spent, outcome.best
        ),
        MultiwayEvent::View(view) => {
            eprintln!("[{t:>3}s] view: {} nodes (live snapshot)", view.nodes.len())
        }
        MultiwayEvent::Failed(msg) => eprintln!("[{t:>3}s] FAILED: {msg}"),
    }
}
