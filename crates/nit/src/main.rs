#![forbid(unsafe_code)]

use std::collections::HashMap;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;

use anyhow::Context;
use clap::{Parser, Subcommand, ValueEnum};
use nit_core::{io as core_io, AppKind, Buffer, LabId, Mode, PaneId, SelectedRule};
use nit_games::config::EngineMode;
use nit_games::events::{EventWriter, GameEvent};
use nit_games::history_log::MatchHistory;
use nit_games::output::{
    write_summary, RunLayout, RunPaths, RunSummary, RUN_SUMMARY_SCHEMA_VERSION,
};
use nit_games::tournament::{KernelRunMode, Parallelism, TournamentKernel};
use nit_games::{
    enumerate_fsms, format_strategy_introspection, introspect_strategy, run_id_from_seed_config,
    Action, FsmDefinition, GamesConfig, HistoryWriter, InputMode, StrategyIntrospection,
    StrategyIntrospectionKind, StrategyIntrospectionParameters, StrategySpec, TmTransitionRecord,
};
use nit_tui::{run, Theme};
use nit_utils::hashing::stable_hash_bytes;
use nit_utils::paths;
use serde::{Deserialize, Serialize};
use tracing_subscriber::fmt::MakeWriter;

#[derive(Parser, Debug)]
#[command(
    name = "nit",
    version,
    about = "Neural Interface Terminal",
    subcommand_precedence_over_arg = true
)]
struct Cli {
    /// File or directory to open
    path: Option<PathBuf>,
    /// Start in the specified lab (gol or games)
    #[arg(long, value_enum, default_value_t = LabArg::Gol)]
    lab: LabArg,
    /// Agent station backend (defaults to Codex when available, else mock)
    #[arg(long, value_enum)]
    agents: Option<AgentsArg>,
    /// Codex automation runtime (exec spawns per-turn; mcp uses a persistent `codex mcp-server`)
    #[arg(long, value_enum, default_value_t = CodexRuntimeArg::Mcp)]
    codex_runtime: CodexRuntimeArg,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Explicit GoL mode (current behavior)
    Gol {
        /// File or directory to open
        path: Option<PathBuf>,
    },
    /// Games mode (games between programs)
    Games {
        /// File or directory to open
        path: Option<PathBuf>,
        #[command(subcommand)]
        command: Option<GamesCommand>,
    },
}

#[derive(Subcommand, Debug)]
enum GamesCommand {
    /// Headless batch runner for games
    #[command(alias = "tournament")]
    Run {
        /// Config path (defaults to games.toml)
        #[arg(long)]
        config: Option<PathBuf>,
        /// NDJSON strategy list to append
        #[arg(long)]
        strategies: Option<PathBuf>,
        /// Output directory (defaults to ./output)
        #[arg(long)]
        out: Option<PathBuf>,
        /// Override seed
        #[arg(long)]
        seed: Option<u64>,
        /// Output format for stdout summary
        #[arg(long, value_enum, default_value_t = OutputFormat::Pretty)]
        format: OutputFormat,
        /// Suppress stdout summary
        #[arg(long)]
        quiet: bool,
        /// Verbose logging to stderr
        #[arg(long)]
        verbose: bool,
    },
    /// Sweep runner for games (parameter grids)
    Sweep {
        /// Config path (defaults to games.toml)
        #[arg(long)]
        config: Option<PathBuf>,
        /// NDJSON strategy list to append
        #[arg(long)]
        strategies: Option<PathBuf>,
        /// Output directory root (defaults to config directory)
        #[arg(long)]
        out: Option<PathBuf>,
        /// Override base seed
        #[arg(long)]
        seed: Option<u64>,
        /// Rounds grid (comma-separated)
        #[arg(long, value_delimiter = ',')]
        rounds: Vec<u32>,
        /// Noise grid (comma-separated)
        #[arg(long, value_delimiter = ',')]
        noise: Vec<f32>,
        /// Repetitions grid (comma-separated)
        #[arg(long, value_delimiter = ',')]
        repetitions: Vec<u32>,
        /// Payoff preset (pd, stag_hunt, snowdrift, chicken)
        #[arg(long)]
        payoff_preset: Option<String>,
        /// Payoff R grid (comma-separated)
        #[arg(long, value_delimiter = ',')]
        payoff_r: Vec<i32>,
        /// Payoff S grid (comma-separated)
        #[arg(long, value_delimiter = ',')]
        payoff_s: Vec<i32>,
        /// Payoff T grid (comma-separated)
        #[arg(long, value_delimiter = ',')]
        payoff_t: Vec<i32>,
        /// Payoff P grid (comma-separated)
        #[arg(long, value_delimiter = ',')]
        payoff_p: Vec<i32>,
        /// Force rerun even if cell output exists
        #[arg(long)]
        force: bool,
        /// Output format for stdout summary
        #[arg(long, value_enum, default_value_t = OutputFormat::Pretty)]
        format: OutputFormat,
        /// Suppress stdout summary
        #[arg(long)]
        quiet: bool,
        /// Verbose logging to stderr
        #[arg(long)]
        verbose: bool,
    },
    /// Enumerate strategies (FSMs)
    Enumerate {
        #[command(subcommand)]
        kind: EnumerateCommand,
    },
    /// Inspect a strategy definition
    Inspect {
        /// Config path (defaults to games.toml)
        #[arg(long)]
        config: Option<PathBuf>,
        /// Strategy id to inspect
        #[arg(long)]
        id: String,
        /// Output format (json or pretty text)
        #[arg(long, value_enum, default_value_t = OutputFormat::Pretty)]
        format: OutputFormat,
        /// Optional output path (defaults to stdout)
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Export strategy graph (DOT or JSON)
    Graph {
        /// Config path (defaults to games.toml)
        #[arg(long)]
        config: Option<PathBuf>,
        /// Run summary path (optional, overrides --config)
        #[arg(long)]
        run: Option<PathBuf>,
        /// Strategy id to graph (alias: --fsm)
        #[arg(long, alias = "fsm")]
        id: String,
        /// Output path (.dot/.gv or .json)
        #[arg(long)]
        out: PathBuf,
    },
}

#[derive(Subcommand, Debug)]
enum EnumerateCommand {
    /// Enumerate FSM strategies and write NDJSON
    Fsm {
        /// State range (e.g. 2..4)
        #[arg(long)]
        states: String,
        /// Output directory or NDJSON path
        #[arg(long)]
        out: PathBuf,
        /// De-duplicate isomorphic FSMs via canonicalization
        #[arg(long)]
        canonical: bool,
        /// Limit total outputs
        #[arg(long)]
        limit: Option<usize>,
        /// Input mode (opponent_last_action, self_last_action, joint_last_action)
        #[arg(long)]
        input_mode: Option<String>,
    },
}

#[derive(Copy, Clone, Debug, ValueEnum)]
enum OutputFormat {
    Json,
    Pretty,
}

#[derive(Copy, Clone, Debug, ValueEnum)]
enum AgentsArg {
    /// Seed the Agent Station with mock planner/coder/reviewer lanes.
    Mock,
    /// Seed the Agent Station roster from Codex's cached model list (~/.codex/models_cache.json).
    Codex,
}

#[derive(Copy, Clone, Debug, ValueEnum)]
enum CodexRuntimeArg {
    Exec,
    Mcp,
}

#[derive(Copy, Clone, Debug, ValueEnum)]
enum LabArg {
    Gol,
    Games,
}

impl From<LabArg> for LabId {
    fn from(value: LabArg) -> Self {
        match value {
            LabArg::Gol => LabId::Gol,
            LabArg::Games => LabId::Games,
        }
    }
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse_from(normalize_lab_args(std::env::args()));
    if let Some(Command::Games {
        command:
            Some(GamesCommand::Run {
                config,
                strategies,
                out,
                seed,
                format,
                quiet,
                verbose,
            }),
        ..
    }) = cli.command
    {
        return run_games_headless(config, strategies, out, seed, format, quiet, verbose);
    }
    if let Some(Command::Games {
        command:
            Some(GamesCommand::Sweep {
                config,
                strategies,
                out,
                seed,
                rounds,
                noise,
                repetitions,
                payoff_preset,
                payoff_r,
                payoff_s,
                payoff_t,
                payoff_p,
                force,
                format,
                quiet,
                verbose,
            }),
        ..
    }) = cli.command
    {
        return run_games_sweep(
            config,
            strategies,
            out,
            seed,
            rounds,
            noise,
            repetitions,
            payoff_preset,
            payoff_r,
            payoff_s,
            payoff_t,
            payoff_p,
            force,
            format,
            quiet,
            verbose,
        );
    }
    if let Some(Command::Games {
        command:
            Some(GamesCommand::Inspect {
                config,
                id,
                format,
                out,
            }),
        ..
    }) = cli.command
    {
        return run_games_inspect(config, id, format, out);
    }
    if let Some(Command::Games {
        command:
            Some(GamesCommand::Graph {
                config,
                run,
                id,
                out,
            }),
        ..
    }) = cli.command
    {
        return run_games_graph(config, run, id, out);
    }
    if let Some(Command::Games {
        command: Some(GamesCommand::Enumerate { kind }),
        ..
    }) = cli.command
    {
        match kind {
            EnumerateCommand::Fsm {
                states,
                out,
                canonical,
                limit,
                input_mode,
            } => {
                return run_games_enumerate_fsm(&states, &out, canonical, limit, input_mode);
            }
        }
    }

    let (app_kind, target) = match cli.command {
        Some(Command::Gol { path }) => (AppKind::Gol, path),
        Some(Command::Games { path, .. }) => (AppKind::Games, path),
        None => (LabId::from(cli.lab), cli.path),
    };
    let (workspace_root, editor) = match app_kind {
        AppKind::Gol => open_target_gol(target.as_deref())?,
        AppKind::Games => open_target_games(target.as_deref())?,
    };
    let notes = load_notes(&workspace_root);

    let theme_path = find_theme();
    let theme = Theme::load(theme_path.as_deref());

    let (log_tx, log_rx) = mpsc::channel::<String>();
    let log_path = log_path_for_workspace(&workspace_root);
    init_tracing(log_tx, log_path)?;
    install_panic_hook();

    let mut state = nit_core::AppState::new(workspace_root, editor, notes);
    state.agents = match cli.agents {
        Some(AgentsArg::Mock) => nit_core::AgentsState::default_with_mocks(),
        Some(AgentsArg::Codex) => load_agents_from_codex_models_cache().unwrap_or_else(|err| {
            let mut agents = nit_core::AgentsState::default_with_mocks();
            agents.alerts.push(nit_core::AgentAlert {
                severity: nit_core::AgentAlertSeverity::Warn,
                source: "codex".into(),
                message: format!("Failed to load Codex models: {err}"),
                at: "t+0".into(),
            });
            agents
        }),
        None => {
            try_seed_agents_from_codex().unwrap_or_else(nit_core::AgentsState::default_with_mocks)
        }
    };
    if let Some(path) = export_legacy_notes_snapshot(&state.workspace_root, state.notes_buffer()) {
        state.agents.pending_legacy_notes_alert = Some(format!(
            "Legacy Notes were preserved in {} and are available in Agent Ops > Scratchpad.",
            path.display()
        ));
    }
    state.app_kind = app_kind;
    let seed = stable_hash_bytes(state.editor_buffer().content_as_string().as_bytes());
    state.visualizer.seed = seed;
    state.mode = Mode::Normal;
    if target.as_deref().is_none_or(|p| p.is_dir()) {
        state.file_tree.root = state.workspace_root.clone();
        state.file_tree.open = true;
        state.focus = PaneId::Editor;
        state.mode = Mode::Normal;
    }

    if app_kind == AppKind::Gol {
        let rule_config = nit_core::load_rule_config(&state.workspace_root);
        let (catalog, mut rule_warnings) = nit_core::load_rule_catalog(&rule_config.rules.user);
        rule_warnings.extend(rule_config.warnings.into_iter());
        for warning in rule_warnings {
            tracing::warn!("{warning}");
        }
        let selected_key = if rule_config.rule.workspace_override {
            rule_config
                .workspace_rule
                .clone()
                .unwrap_or_else(|| rule_config.rule.default.clone())
        } else {
            rule_config.rule.default.clone()
        };
        let selected = match catalog.select(&selected_key) {
            Ok(selected) => selected,
            Err(err) => {
                tracing::warn!("Invalid configured GoL rule '{selected_key}': {err}");
                SelectedRule::default()
            }
        };
        state.settings.gol.rule = rule_config.rule.clone();
        state.settings.gol.rules = rule_config.rules.clone();
        state.init_rules(
            catalog,
            selected,
            nit_core::RulePersistence {
                global_path: rule_config.global_path,
                workspace_path: rule_config.workspace_path,
                workspace_override: rule_config.rule.workspace_override,
            },
        );
    }

    let codex_runtime = match cli.codex_runtime {
        CodexRuntimeArg::Exec => nit_tui::codex_runner::CodexRuntimeMode::Exec,
        CodexRuntimeArg::Mcp => nit_tui::codex_runner::CodexRuntimeMode::Mcp,
    };
    run(state, theme, log_rx, codex_runtime)?;
    Ok(())
}

fn normalize_lab_args<I>(args: I) -> Vec<String>
where
    I: IntoIterator<Item = String>,
{
    let mut iter = args.into_iter();
    let mut out = Vec::new();
    if let Some(bin) = iter.next() {
        out.push(bin);
    }
    while let Some(arg) = iter.next() {
        if arg == "--lab" {
            match iter.next() {
                Some(value) => {
                    let value_lc = value.to_ascii_lowercase();
                    if value_lc == "gol" || value_lc == "games" {
                        out.push(format!("--lab={value}"));
                    } else {
                        out.push(arg);
                        out.push(value);
                    }
                }
                None => {
                    out.push(arg);
                }
            }
            continue;
        }
        out.push(arg);
    }
    out
}

fn open_target_gol(path: Option<&Path>) -> anyhow::Result<(PathBuf, Buffer)> {
    match path {
        Some(p) if p.is_file() => {
            let content = core_io::load_to_string(p)
                .with_context(|| format!("failed to read {}", p.display()))?;
            let name = p
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| "untitled".into());
            let buffer = Buffer::from_str(name, &content, Some(p.to_path_buf()));
            let root = p
                .parent()
                .map(|p| p.to_path_buf())
                .unwrap_or(std::env::current_dir()?);
            Ok((root, buffer))
        }
        Some(p) if p.is_dir() => {
            let root = p.to_path_buf();
            let buffer = Buffer::empty("untitled", None);
            Ok((root, buffer))
        }
        None => {
            let root = std::env::current_dir()?;
            let buffer = Buffer::empty("untitled", None);
            Ok((root, buffer))
        }
        Some(other) => anyhow::bail!("path does not exist: {}", other.display()),
    }
}

fn open_target_games(path: Option<&Path>) -> anyhow::Result<(PathBuf, Buffer)> {
    match path {
        Some(p) if p.is_file() => {
            let content = core_io::load_to_string(p)
                .with_context(|| format!("failed to read {}", p.display()))?;
            let name = p
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| "games.toml".into());
            let buffer = Buffer::from_str(name, &content, Some(p.to_path_buf()));
            let root = p
                .parent()
                .map(|p| p.to_path_buf())
                .unwrap_or(std::env::current_dir()?);
            Ok((root, buffer))
        }
        Some(p) if p.is_dir() => open_games_workspace(p),
        None => {
            let root = std::env::current_dir()?;
            open_games_workspace(&root)
        }
        Some(other) => anyhow::bail!("path does not exist: {}", other.display()),
    }
}

fn open_games_workspace(root: &Path) -> anyhow::Result<(PathBuf, Buffer)> {
    let root = root.to_path_buf();
    let config_path = root.join("games.toml");
    if config_path.exists() {
        let content = core_io::load_to_string(&config_path)
            .with_context(|| format!("failed to read {}", config_path.display()))?;
        let buffer = Buffer::from_str("games.toml", &content, Some(config_path));
        return Ok((root, buffer));
    }
    let buffer = Buffer::from_str("games.toml", games_template(), Some(config_path));
    Ok((root, buffer))
}

fn games_template() -> &'static str {
    r#"schema_version = 1
game = "ipd"
rounds = 200
repetitions = 1
self_play = false
seed = 12345
noise = 0.0

[payoff]
R = 3
S = 0
T = 5
P = 1

[history]
enabled = true
include_cycle_metadata = false

[engine]
mode = "interactive"
parallelism = "auto"
progress_interval_ms = 80
fast_eval = true

[engine.complexity_cost]
enabled = false
tm_step_cost = 0.0
fsm_state_cost = 0.0

[[strategy]]
id = "fsm_tft"
type = "auto"
states = 2
start_state = 1
input_index_base = 1
outputs = ["C", "D"]
transitions = [
  [1, 2],
  [1, 2],
]

[[strategy]]
id = "fsm_index_allc"
type = "fsm"
index = 1
num_states = 1
k = 2

[[strategy]]
id = "ca_rule30"
type = "ca"
n = 30
k = 2
r = 1
t = 3

[[strategy]]
id = "tm_rule_3111"
type = "auto"
states = 2
symbols = 2
start_state = 1
blank = 0
max_steps_per_round = 128
rule_code = 3111
"#
}

fn execute_tournament(
    kernel: &TournamentKernel,
    config: &nit_games::NormalizedConfig,
    event_path: Option<PathBuf>,
    history_path: Option<PathBuf>,
) -> anyhow::Result<(
    nit_games::output::TournamentResults,
    Option<String>,
    Option<String>,
)> {
    let parallelism = Parallelism::from_config(&config.engine.parallelism);
    let event_log_enabled = event_path.is_some();
    let history_log_enabled = history_path.is_some();

    let (results, event_log, history_log) = if matches!(parallelism, Parallelism::Off) {
        let mut event_writer = if event_log_enabled {
            Some(EventWriter::new(
                event_path.clone().expect("event path"),
                config.event_log.include_rounds,
            )?)
        } else {
            None
        };
        let mut history_writer = if history_log_enabled {
            Some(HistoryWriter::new(
                history_path.clone().expect("history path"),
            )?)
        } else {
            None
        };
        let results = kernel.run(KernelRunMode::Sequential {
            event_writer: event_writer.as_mut(),
            history_writer: history_writer.as_mut(),
        });
        let event_log = match event_writer {
            Some(writer) => Some(
                writer
                    .finish()
                    .with_context(|| "failed to finalize event log")?
                    .to_string_lossy()
                    .to_string(),
            ),
            None => None,
        };
        let history_log = match history_writer {
            Some(writer) => Some(
                writer
                    .finish()
                    .with_context(|| "failed to finalize history log")?
                    .to_string_lossy()
                    .to_string(),
            ),
            None => None,
        };
        (results, event_log, history_log)
    } else {
        let mut event_sender = None;
        let mut history_sender = None;
        let mut event_handle: Option<thread::JoinHandle<std::io::Result<PathBuf>>> = None;
        let mut history_handle: Option<thread::JoinHandle<std::io::Result<PathBuf>>> = None;

        if event_log_enabled {
            let writer = EventWriter::new(
                event_path.clone().expect("event path"),
                config.event_log.include_rounds,
            )?;
            let (tx, rx) = mpsc::channel::<GameEvent>();
            let handle = thread::spawn(move || {
                let mut writer = writer;
                for event in rx {
                    if let Err(err) = writer.write(&event) {
                        return Err(err);
                    }
                }
                writer.finish()
            });
            event_sender = Some(tx);
            event_handle = Some(handle);
        }

        if history_log_enabled {
            let writer = HistoryWriter::new(history_path.clone().expect("history path"))?;
            let (tx, rx) = mpsc::channel::<MatchHistory>();
            let handle = thread::spawn(move || {
                let mut writer = writer;
                for record in rx {
                    if let Err(err) = writer.write(&record) {
                        return Err(err);
                    }
                }
                writer.finish()
            });
            history_sender = Some(tx);
            history_handle = Some(handle);
        }

        let results = kernel.run(KernelRunMode::Parallel {
            parallelism,
            event_sender: event_sender.clone(),
            include_rounds: config.event_log.include_rounds,
            history_sender: history_sender.clone(),
        });

        drop(event_sender);
        drop(history_sender);

        let event_log = match event_handle {
            Some(handle) => Some(
                handle
                    .join()
                    .map_err(|_| anyhow::anyhow!("event log worker panicked"))?
                    .with_context(|| "failed to finalize event log")?
                    .to_string_lossy()
                    .to_string(),
            ),
            None => None,
        };
        let history_log = match history_handle {
            Some(handle) => Some(
                handle
                    .join()
                    .map_err(|_| anyhow::anyhow!("history log worker panicked"))?
                    .with_context(|| "failed to finalize history log")?
                    .to_string_lossy()
                    .to_string(),
            ),
            None => None,
        };
        (results, event_log, history_log)
    };

    Ok((results, event_log, history_log))
}

fn resolve_relative_path(path: &Path, base_dir: Option<&Path>) -> PathBuf {
    if path.is_absolute() {
        return path.to_path_buf();
    }
    if let Some(base) = base_dir {
        return base.join(path);
    }
    std::env::current_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .join(path)
}

fn append_strategies_from_ndjson(
    config: &mut nit_games::NormalizedConfig,
    path: &Path,
) -> anyhow::Result<()> {
    use std::io::BufRead;
    let file = std::fs::File::open(path)
        .with_context(|| format!("failed to open strategies {}", path.display()))?;
    let reader = std::io::BufReader::new(file);
    for (line_idx, line) in reader.lines().enumerate() {
        let line = line.with_context(|| {
            format!(
                "failed to read strategies {} line {}",
                path.display(),
                line_idx + 1
            )
        })?;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let spec: StrategySpec = serde_json::from_str(trimmed).with_context(|| {
            format!(
                "failed to parse strategies {} line {}",
                path.display(),
                line_idx + 1
            )
        })?;
        config.strategies.push(spec);
    }
    Ok(())
}

fn run_games_headless(
    config_path: Option<PathBuf>,
    strategies_path: Option<PathBuf>,
    out_dir: Option<PathBuf>,
    seed_override: Option<u64>,
    format: OutputFormat,
    quiet: bool,
    verbose: bool,
) -> anyhow::Result<()> {
    let config_path = config_path.unwrap_or_else(|| PathBuf::from("games.toml"));
    let config_text = core_io::load_to_string(&config_path)
        .with_context(|| format!("failed to read {}", config_path.display()))?;
    let mut config = GamesConfig::from_toml_with_root(&config_text, config_path.parent())
        .map_err(|err| anyhow::anyhow!(err))?;

    if let Some(strategies_path) = strategies_path {
        let resolved = resolve_relative_path(&strategies_path, config_path.parent());
        append_strategies_from_ndjson(&mut config, &resolved)?;
    }

    if let Some(seed) = seed_override {
        config.seed = Some(seed);
    }
    config.engine.mode = EngineMode::Batch;

    let timestamp = EventWriter::timestamp();
    let seed = config
        .seed
        .unwrap_or_else(|| stable_hash_bytes(format!("{timestamp}\n{config_text}").as_bytes()));
    config.seed = Some(seed);

    let run_id = run_id_from_seed_config(seed, &config_text);
    let cwd = std::env::current_dir()?;
    let base_dir = config_path
        .parent()
        .map(|p| {
            if p.is_absolute() {
                p.to_path_buf()
            } else {
                cwd.join(p)
            }
        })
        .unwrap_or(cwd);

    let out_dir = out_dir.unwrap_or_else(|| base_dir.clone());
    let out_dir = if out_dir.is_absolute() {
        out_dir
    } else {
        base_dir.join(out_dir)
    };

    let layout = RunLayout::for_base(&out_dir, &timestamp, seed, &run_id);
    fs::create_dir_all(&layout.run_dir)
        .with_context(|| format!("failed to create {}", layout.run_dir.display()))?;

    let summary_path = layout.summary_path.clone();
    let event_path = layout.events_path.clone();
    let history_path = layout.history_path.clone();

    if verbose {
        eprintln!("Games config: {}", config_path.display());
        eprintln!("Games summary: {}", summary_path.display());
    }

    let kernel = TournamentKernel::new(config.clone());
    let event_log_enabled = config.event_log.enabled;
    let history_log_enabled = config.history.enabled;
    let (results, event_log, history_log) = execute_tournament(
        &kernel,
        &config,
        event_log_enabled.then_some(event_path.clone()),
        history_log_enabled.then_some(history_path.clone()),
    )?;

    if let Err(err) = fs::write(&layout.config_path, &config_text) {
        eprintln!("Warning: failed to write config snapshot: {err}");
    }

    let definitions_path = layout.definitions_path.clone();
    if let Err(err) = nit_utils::fs::write_atomic(&definitions_path, |writer| {
        serde_json::to_writer_pretty(writer, kernel.definitions())
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))
    }) {
        eprintln!("Warning: failed to write definitions: {err}");
    }

    let results_path = layout.results_path.clone();
    if let Err(err) = nit_utils::fs::write_atomic(&results_path, |writer| {
        serde_json::to_writer_pretty(writer, &results)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))
    }) {
        eprintln!("Warning: failed to write results: {err}");
    }

    let summary = RunSummary {
        schema_version: RUN_SUMMARY_SCHEMA_VERSION,
        timestamp,
        run_id,
        seed,
        config_text: config_text.clone(),
        config: config.clone(),
        paths: RunPaths {
            summary: Some(summary_path.display().to_string()),
            events: event_log.clone(),
            history: history_log.clone(),
            definitions: Some(definitions_path.display().to_string()),
            results: Some(results_path.display().to_string()),
            config: Some(layout.config_path.display().to_string()),
            analysis_dir: Some(layout.analysis_dir.display().to_string()),
        },
        strategies: kernel.definitions().to_vec(),
        results,
        event_log,
        history_log,
        run_dir: Some(layout.run_dir.display().to_string()),
    };

    write_summary(&summary_path, &summary)
        .with_context(|| format!("failed to write {}", summary_path.display()))?;

    if verbose {
        if let Some(path) = summary.paths.events.as_ref() {
            eprintln!("Events: {}", path);
        }
        if let Some(path) = summary.paths.history.as_ref() {
            eprintln!("History: {}", path);
        }
    }

    if !quiet {
        let out = match format {
            OutputFormat::Json => serde_json::to_string(&summary)?,
            OutputFormat::Pretty => serde_json::to_string_pretty(&summary)?,
        };
        println!("{out}");
    }

    Ok(())
}

fn run_games_sweep(
    config_path: Option<PathBuf>,
    strategies_path: Option<PathBuf>,
    out_dir: Option<PathBuf>,
    seed_override: Option<u64>,
    rounds: Vec<u32>,
    noise: Vec<f32>,
    repetitions: Vec<u32>,
    payoff_preset: Option<String>,
    payoff_r: Vec<i32>,
    payoff_s: Vec<i32>,
    payoff_t: Vec<i32>,
    payoff_p: Vec<i32>,
    force: bool,
    format: OutputFormat,
    quiet: bool,
    verbose: bool,
) -> anyhow::Result<()> {
    let config_path = config_path.unwrap_or_else(|| PathBuf::from("games.toml"));
    let config_text = core_io::load_to_string(&config_path)
        .with_context(|| format!("failed to read {}", config_path.display()))?;
    let mut config = GamesConfig::from_toml_with_root(&config_text, config_path.parent())
        .map_err(|err| anyhow::anyhow!(err))?;

    if let Some(strategies_path) = strategies_path {
        let resolved = resolve_relative_path(&strategies_path, config_path.parent());
        append_strategies_from_ndjson(&mut config, &resolved)?;
    }

    let timestamp = EventWriter::timestamp();
    let base_seed = seed_override
        .or(config.seed)
        .unwrap_or_else(|| stable_hash_bytes(format!("{timestamp}\n{config_text}").as_bytes()));
    config.seed = Some(base_seed);
    config.engine.mode = EngineMode::Batch;

    let rounds_grid = if rounds.is_empty() {
        vec![config.rounds]
    } else {
        rounds
    };
    let noise_grid = if noise.is_empty() {
        vec![config.noise]
    } else {
        noise
    };
    let reps_grid = if repetitions.is_empty() {
        vec![config.repetitions]
    } else {
        repetitions
    };
    let preset_values = match payoff_preset.as_deref() {
        Some(name) => resolve_payoff_preset(name)
            .ok_or_else(|| anyhow::anyhow!("unknown payoff preset '{name}'"))?
            .into(),
        None => (
            config.payoff.r,
            config.payoff.s,
            config.payoff.t,
            config.payoff.p,
        ),
    };
    let (base_r, base_s, base_t, base_p) = preset_values;
    let payoff_r_grid = if payoff_r.is_empty() {
        vec![base_r]
    } else {
        payoff_r
    };
    let payoff_s_grid = if payoff_s.is_empty() {
        vec![base_s]
    } else {
        payoff_s
    };
    let payoff_t_grid = if payoff_t.is_empty() {
        vec![base_t]
    } else {
        payoff_t
    };
    let payoff_p_grid = if payoff_p.is_empty() {
        vec![base_p]
    } else {
        payoff_p
    };

    let cwd = std::env::current_dir()?;
    let base_dir = config_path
        .parent()
        .map(|p| {
            if p.is_absolute() {
                p.to_path_buf()
            } else {
                cwd.join(p)
            }
        })
        .unwrap_or(cwd);
    let out_dir = out_dir.unwrap_or_else(|| base_dir.clone());
    let out_dir = if out_dir.is_absolute() {
        out_dir
    } else {
        base_dir.join(out_dir)
    };

    let stamp = timestamp.replace(':', "-");
    let sweep_root = out_dir
        .join("runs")
        .join("games")
        .join("sweeps")
        .join(format!("{stamp}__seed-{base_seed}"));
    let cells_root = sweep_root.join("cells");
    fs::create_dir_all(&cells_root)
        .with_context(|| format!("failed to create {}", cells_root.display()))?;

    let mut cell_summaries = Vec::new();
    let mut raw_totals_by_strategy: HashMap<String, Vec<f64>> = HashMap::new();
    let mut adjusted_totals_by_strategy: HashMap<String, Vec<f64>> = HashMap::new();
    let mut top_counts: HashMap<String, u32> = HashMap::new();
    let mut cell_id = 0usize;
    let top_k = 3usize;

    let collect_results = |results: &nit_games::output::TournamentResults,
                           raw_totals: &mut HashMap<String, Vec<f64>>,
                           adjusted_totals: &mut HashMap<String, Vec<f64>>,
                           top_counts: &mut HashMap<String, u32>| {
        let mut top_entries = Vec::new();
        for entry in results.ranking.iter().take(top_k) {
            top_entries.push(SweepTopEntry {
                id: entry.id.clone(),
                total_payoff: entry.total_payoff,
                adjusted_total_payoff: entry.adjusted_total_payoff,
            });
        }
        let top_id = top_entries
            .first()
            .map(|entry| entry.id.clone())
            .unwrap_or_else(|| "none".into());
        *top_counts.entry(top_id.clone()).or_insert(0) += 1;

        for strategy in &results.ranking {
            raw_totals
                .entry(strategy.id.clone())
                .or_default()
                .push(strategy.total_payoff as f64);
            if let Some(adj) = strategy.adjusted_total_payoff {
                adjusted_totals
                    .entry(strategy.id.clone())
                    .or_default()
                    .push(adj);
            }
        }

        (top_id, top_entries)
    };

    for rounds in &rounds_grid {
        for noise in &noise_grid {
            for reps in &reps_grid {
                for r in &payoff_r_grid {
                    for s in &payoff_s_grid {
                        for t in &payoff_t_grid {
                            for p in &payoff_p_grid {
                                cell_id += 1;
                                let noise_bits = noise.to_bits();
                                let cell_seed = stable_hash_bytes(
                                    format!(
                                        "{base_seed}:{rounds}:{reps}:{noise_bits}:{r}:{s}:{t}:{p}"
                                    )
                                    .as_bytes(),
                                );
                                let mut cell_config = config.clone();
                                cell_config.rounds = *rounds;
                                cell_config.repetitions = *reps;
                                cell_config.noise = (*noise).clamp(0.0, 1.0);
                                cell_config.payoff = payoff_from_rsp(*r, *s, *t, *p);
                                cell_config.seed = Some(cell_seed);
                                cell_config.engine.mode = EngineMode::Batch;

                                let config_text_cell = toml::to_string(&cell_config)
                                    .unwrap_or_else(|_| config_text.clone());
                                let run_id = run_id_from_seed_config(cell_seed, &config_text_cell);
                                let noise_label = format!("{:.4}", noise).replace('.', "_");
                                let cell_dir = cells_root.join(format!(
                                    "{:04}__r{}__n{}__rep{}__R{}__S{}__T{}__P{}",
                                    cell_id, rounds, noise_label, reps, r, s, t, p
                                ));
                                fs::create_dir_all(&cell_dir).with_context(|| {
                                    format!("failed to create {}", cell_dir.display())
                                })?;

                                let summary_path = cell_dir.join("run_summary.json");
                                let definitions_path = cell_dir.join("definitions.json");
                                let results_path = cell_dir.join("results.json");
                                let events_path = cell_dir.join("events.ndjson");
                                let history_path = cell_dir.join("history.ndjson");
                                let config_path = cell_dir.join("config.toml");
                                let analysis_dir = cell_dir.join("analysis");

                                if summary_path.exists() && !force {
                                    if let Ok(summary_text) = fs::read_to_string(&summary_path) {
                                        if let Ok(summary) =
                                            serde_json::from_str::<RunSummary>(&summary_text)
                                        {
                                            let (top_id, top_entries) = collect_results(
                                                &summary.results,
                                                &mut raw_totals_by_strategy,
                                                &mut adjusted_totals_by_strategy,
                                                &mut top_counts,
                                            );
                                            cell_summaries.push(SweepCellSummary {
                                                cell_id,
                                                rounds: *rounds,
                                                noise: *noise,
                                                repetitions: *reps,
                                                payoff_r: *r,
                                                payoff_s: *s,
                                                payoff_t: *t,
                                                payoff_p: *p,
                                                seed: summary.seed,
                                                run_id: summary.run_id.clone(),
                                                run_dir: summary.run_dir.clone().unwrap_or_else(
                                                    || cell_dir.display().to_string(),
                                                ),
                                                summary_path: summary
                                                    .paths
                                                    .summary
                                                    .clone()
                                                    .unwrap_or_else(|| {
                                                        summary_path.display().to_string()
                                                    }),
                                                top_strategy: top_id,
                                                top_strategies: top_entries,
                                                skipped: true,
                                            });
                                            if verbose {
                                                eprintln!(
                                                    "Skipping existing cell {} ({}): {}",
                                                    cell_id,
                                                    summary.run_id,
                                                    summary_path.display()
                                                );
                                            }
                                            continue;
                                        }
                                    }
                                }

                                if let Err(err) = fs::write(&config_path, &config_text_cell) {
                                    eprintln!("Warning: failed to write config snapshot: {err}");
                                }

                                let kernel = TournamentKernel::new(cell_config.clone());
                                let (results, event_log, history_log) = execute_tournament(
                                    &kernel,
                                    &cell_config,
                                    cell_config.event_log.enabled.then_some(events_path.clone()),
                                    cell_config.history.enabled.then_some(history_path.clone()),
                                )?;

                                if let Err(err) =
                                    nit_utils::fs::write_atomic(&definitions_path, |writer| {
                                        serde_json::to_writer_pretty(writer, kernel.definitions())
                                            .map_err(|e| {
                                                std::io::Error::new(std::io::ErrorKind::Other, e)
                                            })
                                    })
                                {
                                    eprintln!("Warning: failed to write definitions: {err}");
                                }
                                if let Err(err) =
                                    nit_utils::fs::write_atomic(&results_path, |writer| {
                                        serde_json::to_writer_pretty(writer, &results).map_err(
                                            |e| std::io::Error::new(std::io::ErrorKind::Other, e),
                                        )
                                    })
                                {
                                    eprintln!("Warning: failed to write results: {err}");
                                }

                                let summary = RunSummary {
                                    schema_version: RUN_SUMMARY_SCHEMA_VERSION,
                                    timestamp: timestamp.clone(),
                                    run_id: run_id.clone(),
                                    seed: cell_seed,
                                    config_text: config_text_cell.clone(),
                                    config: cell_config.clone(),
                                    paths: RunPaths {
                                        summary: Some(summary_path.display().to_string()),
                                        events: event_log.clone(),
                                        history: history_log.clone(),
                                        definitions: Some(definitions_path.display().to_string()),
                                        results: Some(results_path.display().to_string()),
                                        config: Some(config_path.display().to_string()),
                                        analysis_dir: Some(analysis_dir.display().to_string()),
                                    },
                                    strategies: kernel.definitions().to_vec(),
                                    results: results.clone(),
                                    event_log,
                                    history_log,
                                    run_dir: Some(cell_dir.display().to_string()),
                                };

                                write_summary(&summary_path, &summary).with_context(|| {
                                    format!("failed to write {}", summary_path.display())
                                })?;

                                let (top_id, top_entries) = collect_results(
                                    &results,
                                    &mut raw_totals_by_strategy,
                                    &mut adjusted_totals_by_strategy,
                                    &mut top_counts,
                                );
                                cell_summaries.push(SweepCellSummary {
                                    cell_id,
                                    rounds: *rounds,
                                    noise: *noise,
                                    repetitions: *reps,
                                    payoff_r: *r,
                                    payoff_s: *s,
                                    payoff_t: *t,
                                    payoff_p: *p,
                                    seed: cell_seed,
                                    run_id,
                                    run_dir: cell_dir.display().to_string(),
                                    summary_path: summary_path.display().to_string(),
                                    top_strategy: top_id,
                                    top_strategies: top_entries,
                                    skipped: false,
                                });
                            }
                        }
                    }
                }
            }
        }
    }

    let mut strategies = Vec::new();
    let sort_by_adjusted = config.engine.complexity_cost.enabled;
    for (id, totals) in raw_totals_by_strategy {
        let count = totals.len() as f64;
        let mean = totals.iter().sum::<f64>() / count.max(1.0);
        let var = totals
            .iter()
            .map(|v| {
                let diff = *v - mean;
                diff * diff
            })
            .sum::<f64>()
            / count.max(1.0);
        let std = var.sqrt();
        let (mean_adj, std_adj) = adjusted_totals_by_strategy
            .get(&id)
            .and_then(|vals| {
                if vals.is_empty() {
                    None
                } else {
                    let count = vals.len() as f64;
                    let mean = vals.iter().sum::<f64>() / count.max(1.0);
                    let var = vals
                        .iter()
                        .map(|v| {
                            let diff = *v - mean;
                            diff * diff
                        })
                        .sum::<f64>()
                        / count.max(1.0);
                    Some((mean, var.sqrt()))
                }
            })
            .unwrap_or((0.0, 0.0));
        let adjusted_present = adjusted_totals_by_strategy.get(&id).is_some();
        let top_count = top_counts.get(&id).copied().unwrap_or(0);
        strategies.push(SweepStrategyAggregate {
            id,
            mean_total_payoff: mean,
            std_total_payoff: std,
            mean_adjusted_payoff: adjusted_present.then_some(mean_adj),
            std_adjusted_payoff: adjusted_present.then_some(std_adj),
            top1_count: top_count,
        });
    }
    if sort_by_adjusted {
        strategies.sort_by(|a, b| {
            let a_score = a.mean_adjusted_payoff.unwrap_or(a.mean_total_payoff);
            let b_score = b.mean_adjusted_payoff.unwrap_or(b.mean_total_payoff);
            b_score.partial_cmp(&a_score).unwrap()
        });
    } else {
        strategies.sort_by(|a, b| {
            b.mean_total_payoff
                .partial_cmp(&a.mean_total_payoff)
                .unwrap()
        });
    }

    let summary = SweepSummary {
        schema_version: 1,
        timestamp: timestamp.clone(),
        seed: base_seed,
        config_path: config_path.display().to_string(),
        grid: SweepGrid {
            rounds: rounds_grid.clone(),
            noise: noise_grid.clone(),
            repetitions: reps_grid.clone(),
            payoff_preset: payoff_preset.clone(),
            payoff_r: payoff_r_grid.clone(),
            payoff_s: payoff_s_grid.clone(),
            payoff_t: payoff_t_grid.clone(),
            payoff_p: payoff_p_grid.clone(),
        },
        cells: cell_summaries,
        aggregate: SweepAggregate { strategies },
    };

    let summary_path = sweep_root.join("sweep_summary.json");
    nit_utils::fs::write_atomic(&summary_path, |writer| {
        serde_json::to_writer_pretty(writer, &summary)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))
    })
    .with_context(|| format!("failed to write {}", summary_path.display()))?;

    if verbose {
        eprintln!("Sweep summary: {}", summary_path.display());
    }

    if !quiet {
        let out = match format {
            OutputFormat::Json => serde_json::to_string(&summary)?,
            OutputFormat::Pretty => serde_json::to_string_pretty(&summary)?,
        };
        println!("{out}");
    }

    Ok(())
}

fn run_games_inspect(
    config_path: Option<PathBuf>,
    id: String,
    format: OutputFormat,
    out: Option<PathBuf>,
) -> anyhow::Result<()> {
    let config_path = config_path.unwrap_or_else(|| PathBuf::from("games.toml"));
    let config_text = core_io::load_to_string(&config_path)
        .with_context(|| format!("failed to read {}", config_path.display()))?;
    let config = GamesConfig::from_toml_with_root(&config_text, config_path.parent())
        .map_err(|err| anyhow::anyhow!(err))?;

    let spec = config
        .strategies
        .iter()
        .find(|spec| spec.id == id)
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("strategy '{}' not found", id))?;
    let intro = introspect_strategy(&spec);
    let output = match format {
        OutputFormat::Json => serde_json::to_string(&intro)?,
        OutputFormat::Pretty => format_strategy_introspection(&intro).join("\n"),
    };

    if let Some(out_path) = out {
        if let Some(parent) = out_path.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent)
                    .with_context(|| format!("failed to create directory {}", parent.display()))?;
            }
        }
        fs::write(&out_path, output)
            .with_context(|| format!("failed to write {}", out_path.display()))?;
    } else {
        println!("{output}");
    }

    Ok(())
}

#[derive(Serialize)]
struct GraphNode {
    id: String,
    label: String,
}

#[derive(Serialize)]
struct GraphEdge {
    from: String,
    to: String,
    label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    color: Option<String>,
}

#[derive(Serialize)]
struct StrategyGraph {
    directed: bool,
    strategy_id: String,
    kind: StrategyIntrospectionKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    input_mode: Option<InputMode>,
    #[serde(skip_serializing_if = "Option::is_none")]
    start_state: Option<String>,
    nodes: Vec<GraphNode>,
    edges: Vec<GraphEdge>,
    #[serde(skip_serializing_if = "Option::is_none")]
    notes: Option<Vec<String>>,
}

fn run_games_graph(
    config_path: Option<PathBuf>,
    run_path: Option<PathBuf>,
    strategy_id: String,
    out_path: PathBuf,
) -> anyhow::Result<()> {
    let spec = if let Some(run_path) = run_path {
        let run_text = core_io::load_to_string(&run_path)
            .with_context(|| format!("failed to read {}", run_path.display()))?;
        let summary: RunSummary = serde_json::from_str(&run_text)
            .with_context(|| format!("failed to parse {}", run_path.display()))?;
        summary
            .config
            .strategies
            .iter()
            .find(|spec| spec.id == strategy_id)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("strategy '{}' not found", strategy_id))?
    } else {
        let config_path = config_path.unwrap_or_else(|| PathBuf::from("games.toml"));
        let config_text = core_io::load_to_string(&config_path)
            .with_context(|| format!("failed to read {}", config_path.display()))?;
        let config = GamesConfig::from_toml_with_root(&config_text, config_path.parent())
            .map_err(|err| anyhow::anyhow!(err))?;
        config
            .strategies
            .iter()
            .find(|spec| spec.id == strategy_id)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("strategy '{}' not found", strategy_id))?
    };
    let intro = introspect_strategy(&spec);
    let graph = build_strategy_graph(&intro)?;

    let ext = out_path
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("");
    let is_json = ext.eq_ignore_ascii_case("json");
    let is_dot = ext.eq_ignore_ascii_case("dot") || ext.eq_ignore_ascii_case("gv");
    if !is_json && !is_dot {
        anyhow::bail!("output path must end with .json, .dot, or .gv");
    }

    if let Some(parent) = out_path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)
                .with_context(|| format!("failed to create directory {}", parent.display()))?;
        }
    }

    if is_json {
        nit_utils::fs::write_atomic(&out_path, |writer| {
            serde_json::to_writer_pretty(writer, &graph)
                .map_err(|e| io::Error::new(io::ErrorKind::Other, e))
        })
        .with_context(|| format!("failed to write {}", out_path.display()))?;
    } else {
        let dot = render_strategy_graph_dot(&graph);
        fs::write(&out_path, dot)
            .with_context(|| format!("failed to write {}", out_path.display()))?;
    }

    eprintln!("Graph written: {}", out_path.display());
    Ok(())
}

fn build_strategy_graph(intro: &StrategyIntrospection) -> anyhow::Result<StrategyGraph> {
    match &intro.parameters {
        StrategyIntrospectionParameters::Fsm {
            states,
            start_state,
            outputs,
            transitions,
            index,
            ..
        } => Ok(build_fsm_graph(
            intro.id.clone(),
            intro.kind.clone(),
            *states,
            *start_state,
            outputs,
            transitions,
            index.map(|value| vec![format!("notebook_index={value}")]),
        )),
        StrategyIntrospectionParameters::Ca { n, k, r, t } => Ok(build_ca_graph(
            intro.id.clone(),
            intro.kind.clone(),
            *n,
            *k,
            *r,
            *t,
        )),
        StrategyIntrospectionParameters::OneSidedTm {
            states,
            start_state,
            transitions,
            ..
        } => Ok(build_tm_graph(
            intro.id.clone(),
            intro.kind.clone(),
            *states,
            *start_state,
            transitions,
        )),
    }
}

fn build_fsm_graph(
    strategy_id: String,
    kind: StrategyIntrospectionKind,
    states: usize,
    start_state: usize,
    outputs: &[Action],
    transitions: &[Vec<usize>],
    notes: Option<Vec<String>>,
) -> StrategyGraph {
    let mut nodes = Vec::new();
    for idx in 0..states {
        let output = outputs.get(idx).map(|a| a.as_char()).unwrap_or('?');
        nodes.push(GraphNode {
            id: (idx + 1).to_string(),
            label: format!("{}({output})", idx + 1),
        });
    }
    let mut edges = Vec::new();
    for (state_idx, row) in transitions.iter().enumerate() {
        for (input_idx, next) in row.iter().enumerate() {
            let label = input_idx.to_string();
            edges.push(GraphEdge {
                from: (state_idx + 1).to_string(),
                to: (next + 1).to_string(),
                color: edge_color_for_label(&label),
                label,
            });
        }
    }
    StrategyGraph {
        directed: true,
        strategy_id,
        kind,
        input_mode: Some(InputMode::OpponentLastAction),
        start_state: Some((start_state + 1).to_string()),
        nodes,
        edges,
        notes,
    }
}

fn build_tm_graph(
    strategy_id: String,
    kind: StrategyIntrospectionKind,
    states: u16,
    start_state: u16,
    transitions: &[TmTransitionRecord],
) -> StrategyGraph {
    let mut nodes = Vec::new();
    for state in 1..=states {
        nodes.push(GraphNode {
            id: state.to_string(),
            label: state.to_string(),
        });
    }
    let mut edges = Vec::new();
    let mut uses_halt = false;
    for trans in transitions {
        let label = trans.write.to_string();
        let to_id = if trans.next == 0 {
            uses_halt = true;
            "HALT".to_string()
        } else {
            trans.next.to_string()
        };
        edges.push(GraphEdge {
            from: trans.state.to_string(),
            to: to_id,
            color: edge_color_for_label(&label),
            label,
        });
    }
    if uses_halt {
        nodes.push(GraphNode {
            id: "HALT".to_string(),
            label: "HALT".to_string(),
        });
    }
    StrategyGraph {
        directed: true,
        strategy_id,
        kind,
        input_mode: None,
        start_state: Some(start_state.to_string()),
        nodes,
        edges,
        notes: Some(vec![
            "edges labeled by write symbol (ap)".to_string(),
            "read/move not shown".to_string(),
        ]),
    }
}

fn build_ca_graph(
    strategy_id: String,
    kind: StrategyIntrospectionKind,
    n: u64,
    k: u8,
    r: f32,
    t: u32,
) -> StrategyGraph {
    StrategyGraph {
        directed: true,
        strategy_id,
        kind,
        input_mode: None,
        start_state: None,
        nodes: Vec::new(),
        edges: Vec::new(),
        notes: Some(vec![format!(
            "CA rule tuple {{n={n}, k={k}, r={r}}}, steps={t}"
        )]),
    }
}

fn edge_color_for_label(label: &str) -> Option<String> {
    match label {
        "0" => Some("#e74c3c".to_string()),
        "1" => Some("#2ecc71".to_string()),
        "2" => Some("#3498db".to_string()),
        "3" => Some("#9b59b6".to_string()),
        _ => None,
    }
}

fn render_strategy_graph_dot(graph: &StrategyGraph) -> String {
    let mut dot = String::new();
    dot.push_str("digraph strategy {\n");
    dot.push_str("  rankdir=LR;\n");
    dot.push_str("  node [shape=box];\n");
    if let Some(start) = &graph.start_state {
        dot.push_str("  start [shape=point];\n");
        dot.push_str(&format!("  start -> {};\n", dot_id(start)));
    }
    for node in &graph.nodes {
        let label = node.label.replace('"', "\\\"");
        dot.push_str(&format!("  {} [label=\"{}\"];\n", dot_id(&node.id), label));
    }
    for edge in &graph.edges {
        let label = edge.label.replace('"', "\\\"");
        let mut attrs = vec![format!("label=\"{}\"", label)];
        if let Some(color) = &edge.color {
            attrs.push(format!("color=\"{}\"", color));
            attrs.push(format!("fontcolor=\"{}\"", color));
        }
        dot.push_str(&format!(
            "  {} -> {} [{}];\n",
            dot_id(&edge.from),
            dot_id(&edge.to),
            attrs.join(", ")
        ));
    }
    dot.push_str("}\n");
    dot
}

fn dot_id(raw: &str) -> String {
    let escaped = raw.replace('"', "\\\"");
    format!("\"{}\"", escaped)
}

fn parse_states_range(input: &str) -> anyhow::Result<std::ops::RangeInclusive<usize>> {
    let trimmed = input.trim();
    if let Some((left, right)) = trimmed.split_once("..=") {
        let start: usize = left.trim().parse()?;
        let end: usize = right.trim().parse()?;
        if start > end {
            anyhow::bail!("states range start must be <= end");
        }
        return Ok(start..=end);
    }
    if let Some((left, right)) = trimmed.split_once("..") {
        let start: usize = left.trim().parse()?;
        let end: usize = right.trim().parse()?;
        if start > end {
            anyhow::bail!("states range start must be <= end");
        }
        return Ok(start..=end);
    }
    let value: usize = trimmed.parse()?;
    Ok(value..=value)
}

fn parse_input_mode_arg(input: Option<&str>) -> anyhow::Result<InputMode> {
    let Some(raw) = input else {
        return Ok(InputMode::OpponentLastAction);
    };
    let normalized: String = raw
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_lowercase())
        .collect();
    let mode = match normalized.as_str() {
        "opponentlastaction" | "opponent" | "opp" | "opplastaction" => {
            InputMode::OpponentLastAction
        }
        "selflastaction" | "self" | "selflast" => InputMode::SelfLastAction,
        "jointlastaction" | "joint" | "jointlast" => InputMode::JointLastAction,
        _ => anyhow::bail!(
            "invalid input_mode '{}': expected opponent_last_action, self_last_action, or joint_last_action",
            raw
        ),
    };
    Ok(mode)
}

fn resolve_payoff_preset(name: &str) -> Option<(i32, i32, i32, i32)> {
    let normalized: String = name
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_lowercase())
        .collect();
    match normalized.as_str() {
        "pd" | "prisonersdilemma" | "prisoner" => Some((3, 0, 5, 1)),
        "staghunt" | "stag" => Some((4, 1, 3, 2)),
        "snowdrift" | "snow" | "hawkedove" | "hawkdove" => Some((3, 1, 5, 0)),
        "chicken" => Some((3, 1, 5, 0)),
        _ => None,
    }
}

fn payoff_from_rsp(r: i32, s: i32, t: i32, p: i32) -> nit_games::PayoffMatrix {
    nit_games::PayoffMatrix::from_matrix([[[r, r], [s, t]], [[t, s], [p, p]]])
}

fn run_games_enumerate_fsm(
    states: &str,
    out: &Path,
    canonical: bool,
    limit: Option<usize>,
    input_mode: Option<String>,
) -> anyhow::Result<()> {
    let range = parse_states_range(states)?;
    let mode = parse_input_mode_arg(input_mode.as_deref())?;

    let out_path = if out.extension().map(|ext| ext == "ndjson").unwrap_or(false) {
        out.to_path_buf()
    } else {
        fs::create_dir_all(out)?;
        let filename = format!(
            "fsm_enumeration__states-{}.ndjson",
            states.replace("..", "-")
        );
        out.join(filename)
    };

    let mut writer = std::io::BufWriter::new(
        std::fs::File::create(&out_path)
            .with_context(|| format!("failed to create {}", out_path.display()))?,
    );

    let mut total = 0usize;
    for states in range {
        let remaining = limit.and_then(|limit| limit.checked_sub(total));
        if matches!(remaining, Some(0)) {
            break;
        }
        total += enumerate_fsms(states, mode, remaining, canonical, |def: FsmDefinition| {
            let id = format!("fsm_{:016x}", def.stable_hash());
            let spec = def.to_spec(id);
            serde_json::to_writer(&mut writer, &spec).expect("write fsm strategy");
            writer.write_all(b"\n").expect("write newline");
        });
    }

    writer.flush()?;
    eprintln!("FSM enumeration written: {}", out_path.display());
    Ok(())
}

#[derive(Serialize)]
struct SweepSummary {
    schema_version: u32,
    timestamp: String,
    seed: u64,
    config_path: String,
    grid: SweepGrid,
    cells: Vec<SweepCellSummary>,
    aggregate: SweepAggregate,
}

#[derive(Serialize)]
struct SweepGrid {
    rounds: Vec<u32>,
    noise: Vec<f32>,
    repetitions: Vec<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    payoff_preset: Option<String>,
    payoff_r: Vec<i32>,
    payoff_s: Vec<i32>,
    payoff_t: Vec<i32>,
    payoff_p: Vec<i32>,
}

#[derive(Serialize)]
struct SweepCellSummary {
    cell_id: usize,
    rounds: u32,
    noise: f32,
    repetitions: u32,
    payoff_r: i32,
    payoff_s: i32,
    payoff_t: i32,
    payoff_p: i32,
    seed: u64,
    run_id: String,
    run_dir: String,
    summary_path: String,
    top_strategy: String,
    top_strategies: Vec<SweepTopEntry>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    skipped: bool,
}

#[derive(Serialize)]
struct SweepAggregate {
    strategies: Vec<SweepStrategyAggregate>,
}

#[derive(Serialize)]
struct SweepStrategyAggregate {
    id: String,
    mean_total_payoff: f64,
    std_total_payoff: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    mean_adjusted_payoff: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    std_adjusted_payoff: Option<f64>,
    top1_count: u32,
}

#[derive(Serialize)]
struct SweepTopEntry {
    id: String,
    total_payoff: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    adjusted_total_payoff: Option<f64>,
}

fn find_theme() -> Option<PathBuf> {
    let cwd = std::env::current_dir().ok()?;
    let local = cwd.join("assets/themes/devs.toml");
    if local.exists() {
        return Some(local);
    }
    None
}

fn load_notes(workspace_root: &Path) -> Buffer {
    let Some(path) = notes_path_for_workspace(workspace_root) else {
        return Buffer::empty("notes", None);
    };
    if path.exists() {
        if let Ok(content) = core_io::load_to_string(&path) {
            return Buffer::from_str("notes", &content, Some(path));
        }
    }
    Buffer::empty("notes", Some(path))
}

fn notes_path_for_workspace(workspace_root: &Path) -> Option<PathBuf> {
    let base = paths::state_dir().or_else(paths::data_dir)?;
    let notes_dir = base.join("notes");
    let _ = fs::create_dir_all(&notes_dir);
    let key = workspace_root.to_string_lossy();
    let hash = stable_hash_bytes(key.as_bytes());
    let filename = format!("{:016x}.md", hash);
    Some(notes_dir.join(filename))
}

fn export_legacy_notes_snapshot(workspace_root: &Path, buffer: &Buffer) -> Option<PathBuf> {
    let content = buffer.content_as_string();
    if content.trim().is_empty() {
        return None;
    }
    let path = workspace_root.join(".nit").join("legacy_notes.md");
    if path.exists() {
        return Some(path);
    }
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if fs::write(&path, content).is_ok() {
        Some(path)
    } else {
        None
    }
}

fn init_tracing(tx: mpsc::Sender<String>, log_path: Option<PathBuf>) -> anyhow::Result<()> {
    let file = log_path
        .as_ref()
        .and_then(|path| open_log_file(path).ok())
        .map(|file| Arc::new(Mutex::new(file)));
    let writer = LogWriter { tx, file };
    tracing_subscriber::fmt()
        .with_writer(writer)
        .with_ansi(false)
        .with_env_filter("info,nit_syntax::tree_sitter_engine=error")
        .try_init()
        .ok();
    if let Some(path) = log_path {
        tracing::info!("Log file: {}", path.display());
    }
    Ok(())
}

fn install_panic_hook() {
    std::panic::set_hook(Box::new(|info| {
        tracing::error!("PANIC: {info}");
        let bt = std::backtrace::Backtrace::force_capture();
        tracing::error!("BACKTRACE: {bt:?}");
    }));
}

#[derive(Clone)]
struct LogWriter {
    tx: mpsc::Sender<String>,
    file: Option<Arc<Mutex<std::fs::File>>>,
}

impl<'a> MakeWriter<'a> for LogWriter {
    type Writer = ChannelWriter;

    fn make_writer(&'a self) -> Self::Writer {
        ChannelWriter {
            tx: self.tx.clone(),
            buf: Vec::new(),
            file: self.file.clone(),
        }
    }
}

struct ChannelWriter {
    tx: mpsc::Sender<String>,
    buf: Vec<u8>,
    file: Option<Arc<Mutex<std::fs::File>>>,
}

impl Write for ChannelWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.buf.extend_from_slice(buf);
        self.drain_lines();
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.drain_lines();
        if !self.buf.is_empty() {
            let msg = String::from_utf8_lossy(&self.buf).trim().to_string();
            if !msg.is_empty() {
                if let Some(file) = &self.file {
                    if let Ok(mut file) = file.lock() {
                        let _ = writeln!(file, "{}", msg);
                    }
                }
                let _ = self.tx.send(msg);
            }
            self.buf.clear();
        }
        Ok(())
    }
}

impl ChannelWriter {
    fn drain_lines(&mut self) {
        loop {
            let Some(pos) = self.buf.iter().position(|b| *b == b'\n') else {
                break;
            };
            let line_bytes: Vec<u8> = self.buf.drain(..=pos).collect();
            let line = String::from_utf8_lossy(&line_bytes).trim().to_string();
            if !line.is_empty() {
                if let Some(file) = &self.file {
                    if let Ok(mut file) = file.lock() {
                        let _ = writeln!(file, "{}", line);
                    }
                }
                let _ = self.tx.send(line);
            }
        }
    }
}

fn log_path_for_workspace(workspace_root: &Path) -> Option<PathBuf> {
    if let Ok(path) = std::env::var("NIT_LOG_PATH") {
        return Some(PathBuf::from(path));
    }
    let base = paths::state_dir().or_else(paths::data_dir)?;
    let logs_dir = base.join("logs");
    let _ = fs::create_dir_all(&logs_dir);
    let key = workspace_root.to_string_lossy();
    let hash = stable_hash_bytes(key.as_bytes());
    let filename = format!("{:016x}.log", hash);
    Some(logs_dir.join(filename))
}

#[derive(Deserialize)]
struct CodexModelsCache {
    models: Vec<CodexModelEntry>,
}

#[derive(Deserialize)]
struct CodexModelEntry {
    slug: String,
    #[serde(default)]
    display_name: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    visibility: Option<String>,
    #[serde(default)]
    priority: Option<i64>,
    #[serde(default)]
    context_window: Option<u32>,
    #[serde(default)]
    effective_context_window_percent: Option<u8>,
    #[serde(default)]
    default_reasoning_level: Option<String>,
    #[serde(default)]
    supported_reasoning_levels: Option<Vec<CodexReasoningLevel>>,
}

#[derive(Deserialize)]
struct CodexReasoningLevel {
    effort: String,
}

fn load_agents_from_codex_models_cache() -> anyhow::Result<nit_core::AgentsState> {
    let home = std::env::var("HOME").context("HOME not set")?;
    let path = PathBuf::from(home).join(".codex").join("models_cache.json");
    let raw = fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
    let cache: CodexModelsCache =
        serde_json::from_str(&raw).context("parse ~/.codex/models_cache.json")?;

    let mut entries = cache
        .models
        .into_iter()
        .filter(|m| m.visibility.as_deref().unwrap_or("list") == "list")
        .collect::<Vec<_>>();
    entries.sort_by(|a, b| {
        let pa = a.priority.unwrap_or(i64::MAX);
        let pb = b.priority.unwrap_or(i64::MAX);
        pa.cmp(&pb).then_with(|| a.slug.cmp(&b.slug))
    });

    let mut agents = nit_core::AgentsState::default();
    agents.mcp.state = nit_core::McpConnectionState::Connected;
    agents.mcp.endpoint = format!("codex://cache ({})", path.display());
    agents.mcp.latency_ms = None;
    agents.mcp.last_error = None;

    for model in entries.iter() {
        if let Some(context_window) = model.context_window {
            let effective_pct = model.effective_context_window_percent.unwrap_or(100) as u64;
            let effective_tokens = (context_window as u64)
                .saturating_mul(effective_pct)
                .saturating_div(100) as u32;
            agents
                .codex_effective_context_window_tokens
                .insert(model.slug.clone(), effective_tokens.max(1));
        }

        if let Some(levels) = model.supported_reasoning_levels.as_ref() {
            let mut efforts = levels
                .iter()
                .map(|lvl| lvl.effort.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>();
            efforts.sort_by(|a, b| {
                reasoning_effort_rank(a)
                    .cmp(&reasoning_effort_rank(b))
                    .then_with(|| a.cmp(b))
            });
            efforts.dedup();
            if !efforts.is_empty() {
                agents
                    .codex_supported_reasoning_efforts
                    .insert(model.slug.clone(), efforts);
            }
        }

        if let Some(effort) = pick_codex_reasoning_effort(model) {
            agents
                .codex_default_reasoning_effort
                .insert(model.slug.clone(), effort.clone());
            agents
                .codex_selected_reasoning_effort
                .insert(model.slug.clone(), effort);
        }
    }

    agents.agents = entries
        .into_iter()
        .map(|model| nit_core::AgentLane {
            id: model.slug.clone(),
            role: model
                .display_name
                .clone()
                .unwrap_or_else(|| model.slug.clone()),
            lane: "Codex".into(),
            kind: nit_core::AgentLaneKind::Codex,
            status: nit_core::AgentStatus::Idle,
            heartbeat_age_secs: 0,
            queue_len: 0,
            current_mission: None,
            last_message: model.description.unwrap_or_default(),
        })
        .collect();

    agents.selected_agent = agents.agents.first().map(|a| a.id.clone());
    agents.roster_selected = 0;
    Ok(agents)
}

fn reasoning_effort_rank(effort: &str) -> u8 {
    if effort.eq_ignore_ascii_case("low") {
        0
    } else if effort.eq_ignore_ascii_case("medium") {
        1
    } else if effort.eq_ignore_ascii_case("high") {
        2
    } else if effort.eq_ignore_ascii_case("xhigh") {
        3
    } else {
        10
    }
}

fn pick_codex_reasoning_effort(model: &CodexModelEntry) -> Option<String> {
    let supported = model
        .supported_reasoning_levels
        .as_ref()
        .map(|levels| {
            levels
                .iter()
                .map(|lvl| lvl.effort.trim())
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    let default = model
        .default_reasoning_level
        .as_deref()
        .unwrap_or("medium")
        .trim();
    if supported.is_empty() {
        return Some(default.to_string());
    }

    if let Some(found) = supported
        .iter()
        .find(|effort| effort.eq_ignore_ascii_case(default))
    {
        return Some((*found).to_string());
    }
    for effort in ["medium", "high", "low"] {
        if let Some(found) = supported
            .iter()
            .find(|candidate| candidate.eq_ignore_ascii_case(effort))
        {
            return Some((*found).to_string());
        }
    }

    supported
        .first()
        .copied()
        .map(str::to_string)
        .or_else(|| Some(default.to_string()))
}

fn open_log_file(path: &Path) -> io::Result<std::fs::File> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
}

fn try_seed_agents_from_codex() -> Option<nit_core::AgentsState> {
    if !codex_cli_available() {
        return None;
    }
    match load_agents_from_codex_models_cache() {
        Ok(agents) if !agents.agents.is_empty() => Some(agents),
        Ok(_) => None,
        Err(_) => None,
    }
}

fn codex_cli_available() -> bool {
    is_executable_in_path("codex")
}

fn is_executable_in_path(bin: &str) -> bool {
    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };
    for dir in std::env::split_paths(&path) {
        if dir.as_os_str().is_empty() {
            continue;
        }
        #[cfg(windows)]
        {
            let mut exts = std::env::var_os("PATHEXT")
                .map(|v| {
                    v.to_string_lossy()
                        .split(';')
                        .map(|s| s.trim())
                        .filter(|s| !s.is_empty())
                        .map(|s| s.trim_start_matches('.').to_ascii_lowercase())
                        .collect::<Vec<_>>()
                })
                .unwrap_or_else(|| vec!["exe".into(), "cmd".into(), "bat".into()]);
            if exts.is_empty() {
                exts = vec!["exe".into(), "cmd".into(), "bat".into()];
            }

            let candidate = dir.join(bin);
            if candidate.is_file() {
                return true;
            }
            for ext in exts.iter() {
                let candidate = dir.join(format!("{bin}.{ext}"));
                if candidate.is_file() {
                    return true;
                }
            }
        }
        #[cfg(not(windows))]
        {
            let candidate = dir.join(bin);
            if candidate.is_file() {
                return true;
            }
        }
    }
    false
}
