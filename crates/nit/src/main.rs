#![forbid(unsafe_code)]

use std::collections::HashMap;
use std::ffi::OsString;
use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command as ProcessCommand, Stdio};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

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
    accelerator_run_preflight, enumerate_fsms, format_strategy_introspection, introspect_strategy,
    run_id_from_seed_config, try_select_halting_turing_machine_strategies, Action, FsmDefinition,
    GamesConfig, HistoryWriter, InputMode, ScoreAggregation, StrategyIntrospection,
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
    /// Agent station backend selection (defaults to all available backends)
    #[arg(long, value_enum)]
    agents: Option<AgentsArg>,
    /// Codex automation runtime (exec spawns per-turn; mcp uses a persistent `codex mcp-server`)
    #[arg(long, value_enum, default_value_t = CodexRuntimeArg::Mcp)]
    codex_runtime: CodexRuntimeArg,
    /// Codex sandbox mode (forwarded to Codex runs; default is Codex's own config)
    #[arg(long, value_enum)]
    codex_sandbox: Option<CodexSandboxArg>,
    /// Codex approval policy for executing model-suggested commands.
    ///
    /// nit drives Codex non-interactively (via `codex exec` / `codex mcp-server`), so the safe
    /// default is `never` to avoid hanging on interactive approval prompts.
    #[arg(long, value_enum, default_value_t = CodexApprovalPolicyArg::Never)]
    codex_approval_policy: CodexApprovalPolicyArg,
    /// Maximum number of Codex turns to run concurrently.
    ///
    /// - MCP runtime: caps in-flight `tools/call` requests multiplexed over the persistent server.
    /// - Exec runtime: caps concurrent `codex exec` child processes.
    #[arg(
        long,
        alias = "codex-parallel",
        default_value_t = 2u8,
        value_parser = clap::value_parser!(u8).range(1..=16)
    )]
    codex_max_parallel_turns: u8,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand, Debug)]
#[allow(clippy::large_enum_variant)]
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
    /// Seed the Agent Station with local built-in lanes.
    #[value(alias = "mock")]
    Local,
    /// Seed the Agent Station roster from Codex's cached model list (~/.codex/models_cache.json).
    Codex,
    /// Seed the Agent Station with Claude CLI lane.
    Claude,
    /// Seed all available lanes (local + codex cache + claude).
    All,
}

#[derive(Copy, Clone, Debug, ValueEnum)]
enum CodexRuntimeArg {
    Exec,
    Mcp,
}

#[derive(Copy, Clone, Debug, ValueEnum)]
enum CodexSandboxArg {
    ReadOnly,
    WorkspaceWrite,
    DangerFullAccess,
}

impl CodexSandboxArg {
    fn as_str(self) -> &'static str {
        match self {
            Self::ReadOnly => "read-only",
            Self::WorkspaceWrite => "workspace-write",
            Self::DangerFullAccess => "danger-full-access",
        }
    }
}

#[derive(Copy, Clone, Debug, ValueEnum)]
enum CodexApprovalPolicyArg {
    Untrusted,
    OnFailure,
    OnRequest,
    Never,
}

impl CodexApprovalPolicyArg {
    fn as_str(self) -> &'static str {
        match self {
            Self::Untrusted => "untrusted",
            Self::OnFailure => "on-failure",
            Self::OnRequest => "on-request",
            Self::Never => "never",
        }
    }
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
    let agents_arg = cli.agents.unwrap_or(AgentsArg::All);
    state.agents = match agents_arg {
        AgentsArg::Local => load_local_agent_lane(),
        AgentsArg::Codex => load_only_codex_agents(),
        AgentsArg::Claude => load_only_claude_agents(),
        AgentsArg::All => load_all_available_agents(),
    };
    state.agents.codex_cli_available = codex_cli_available();
    state.agents.claude_cli_available = claude_cli_available();
    state.agents.gemini_cli_available = gemini_cli_available();
    if matches!(agents_arg, AgentsArg::All | AgentsArg::Claude) && state.agents.claude_cli_available
    {
        let (models, error) = probe_claude_models();
        state.agents.claude_models = models;
        state.agents.claude_models_error = error;
        // Populate Claude model metadata (context windows, effort levels).
        populate_claude_model_metadata(&mut state.agents);
    } else {
        state.agents.claude_models.clear();
        state.agents.claude_models_error = None;
    }
    if matches!(agents_arg, AgentsArg::All) && state.agents.gemini_cli_available {
        let (models, error) = probe_gemini_models();
        state.agents.gemini_models = models;
        state.agents.gemini_models_error = error;
    } else {
        state.agents.gemini_models.clear();
        state.agents.gemini_models_error = None;
    }
    sync_backend_model_lanes(&mut state.agents, agents_arg);
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
        rule_warnings.extend(rule_config.warnings);
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
    let codex_config = nit_tui::codex_runner::CodexRunnerConfig {
        sandbox: cli.codex_sandbox.map(|v| v.as_str().to_string()),
        approval_policy: Some(cli.codex_approval_policy.as_str().to_string()),
        max_parallel_turns: cli.codex_max_parallel_turns as usize,
    };
    let claude_config = nit_tui::claude_runner::ClaudeRunnerConfig {
        max_parallel_turns: cli.codex_max_parallel_turns as usize,
        permission_mode: None,
    };
    run(state, theme, log_rx, codex_runtime, codex_config, claude_config)?;
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
self_play = true
save_data = true
seed = 12345
noise = 0.0

[payoff]
R = -1
S = -3
T = 0
P = -2

[history]
enabled = true
include_cycle_metadata = false

[engine]
mode = "interactive"
parallelism = "auto"
progress_interval_ms = 80
fast_eval = true
score_aggregation = "mean" # Code-02 semantics: per-round average score; TotalPayoff sums matchup means

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
    event_path: Option<PathBuf>,
    history_path: Option<PathBuf>,
) -> anyhow::Result<(
    nit_games::output::TournamentResults,
    nit_games::RuntimeAcceleratorStats,
    Option<String>,
    Option<String>,
)> {
    let config = kernel.config();
    let parallelism = Parallelism::from_config(&config.engine.parallelism);
    let event_log_enabled = event_path.is_some();
    let history_log_enabled = history_path.is_some();

    let (results, runtime, event_log, history_log) = if matches!(parallelism, Parallelism::Off) {
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
        let (results, runtime) = kernel.run_with_runtime(KernelRunMode::Sequential {
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
        (results, runtime, event_log, history_log)
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
                    writer.write(&event)?;
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
                    writer.write(&record)?;
                }
                writer.finish()
            });
            history_sender = Some(tx);
            history_handle = Some(handle);
        }

        let (results, runtime) = kernel.run_with_runtime(KernelRunMode::Parallel {
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
        (results, runtime, event_log, history_log)
    };

    Ok((results, runtime, event_log, history_log))
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

    if !config.save_data {
        anyhow::bail!("`save_data = false` is not supported for `games sweep`.");
    }

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
    config =
        try_select_halting_turing_machine_strategies(config).map_err(|err| anyhow::anyhow!(err))?;
    accelerator_run_preflight(
        &config,
        config.save_data && config.event_log.enabled,
        config.save_data && config.history.enabled,
        false,
    )
    .map_err(|err| anyhow::anyhow!(err))?;

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

    let layout = config
        .save_data
        .then(|| RunLayout::for_base(&out_dir, &timestamp, seed, &run_id));
    if let Some(layout) = layout.as_ref() {
        fs::create_dir_all(&layout.run_dir)
            .with_context(|| format!("failed to create {}", layout.run_dir.display()))?;
    }

    let summary_path = layout.as_ref().map(|layout| layout.summary_path.clone());
    let event_path = layout.as_ref().map(|layout| layout.events_path.clone());
    let history_path = layout.as_ref().map(|layout| layout.history_path.clone());

    if verbose {
        eprintln!("Games config: {}", config_path.display());
        match summary_path.as_ref() {
            Some(path) => eprintln!("Games summary: {}", path.display()),
            None => eprintln!("Games summary: disabled (`save_data = false`)"),
        }
    }

    let kernel = TournamentKernel::new(config.clone());
    let event_log_enabled = config.save_data && config.event_log.enabled;
    let history_log_enabled = config.save_data && config.history.enabled;
    let effective_config = kernel.config().clone();
    let (results, runtime, event_log, history_log) = execute_tournament(
        &kernel,
        if event_log_enabled {
            event_path.clone()
        } else {
            None
        },
        if history_log_enabled {
            history_path.clone()
        } else {
            None
        },
    )?;

    if let Some(layout) = layout.as_ref() {
        if let Err(err) = fs::write(&layout.config_path, &config_text) {
            eprintln!("Warning: failed to write config snapshot: {err}");
        }
    }

    if let Some(definitions_path) = layout
        .as_ref()
        .map(|layout| layout.definitions_path.clone())
    {
        if let Err(err) = nit_utils::fs::write_atomic(&definitions_path, |writer| {
            serde_json::to_writer_pretty(writer, kernel.definitions())
                .map_err(std::io::Error::other)
        }) {
            eprintln!("Warning: failed to write definitions: {err}");
        }
    }

    if let Some(results_path) = layout.as_ref().map(|layout| layout.results_path.clone()) {
        if let Err(err) = nit_utils::fs::write_atomic(&results_path, |writer| {
            serde_json::to_writer_pretty(writer, &results).map_err(std::io::Error::other)
        }) {
            eprintln!("Warning: failed to write results: {err}");
        }
    }

    let summary = RunSummary {
        schema_version: RUN_SUMMARY_SCHEMA_VERSION,
        timestamp,
        run_id,
        seed,
        config_text: config_text.clone(),
        config: effective_config.clone(),
        paths: RunPaths {
            summary: summary_path.as_ref().map(|path| path.display().to_string()),
            events: event_log.clone(),
            history: history_log.clone(),
            definitions: layout
                .as_ref()
                .map(|layout| layout.definitions_path.display().to_string()),
            results: layout
                .as_ref()
                .map(|layout| layout.results_path.display().to_string()),
            config: layout
                .as_ref()
                .map(|layout| layout.config_path.display().to_string()),
            analysis_dir: layout
                .as_ref()
                .map(|layout| layout.analysis_dir.display().to_string()),
        },
        strategies: kernel.definitions().to_vec(),
        results,
        event_log,
        history_log,
        runtime,
        run_dir: layout
            .as_ref()
            .map(|layout| layout.run_dir.display().to_string()),
    };

    if let Some(summary_path) = summary_path.as_ref() {
        write_summary(summary_path, &summary).with_context(|| {
            let summary_path_display = summary_path.display().to_string();
            format!("failed to write {summary_path_display}")
        })?;
    }

    if verbose {
        if let Some(path) = summary.paths.events.as_ref() {
            eprintln!("Events: {path}");
        }
        if let Some(path) = summary.paths.history.as_ref() {
            eprintln!("History: {path}");
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

#[allow(clippy::too_many_arguments)]
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
            .ok_or_else(|| anyhow::anyhow!("unknown payoff preset '{name}'"))?,
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
    let score_aggregation = config.engine.score_aggregation;
    let adjusted_scores = config.engine.complexity_cost.enabled;
    let mut scores_by_strategy: HashMap<String, Vec<f64>> = HashMap::new();
    let mut top_counts: HashMap<String, u32> = HashMap::new();
    let mut cell_id = 0usize;
    let top_k = 3usize;

    let collect_results = |results: &nit_games::output::TournamentResults,
                           scores: &mut HashMap<String, Vec<f64>>,
                           top_counts: &mut HashMap<String, u32>| {
        let mut top_entries = Vec::new();
        for entry in results.ranking.iter().take(top_k) {
            top_entries.push(SweepTopEntry {
                id: entry.id.clone(),
                score: entry.score(score_aggregation, adjusted_scores),
            });
        }
        let top_id = top_entries
            .first()
            .map(|entry| entry.id.clone())
            .unwrap_or_else(|| "none".into());
        *top_counts.entry(top_id.clone()).or_insert(0) += 1;

        for strategy in &results.ranking {
            scores
                .entry(strategy.id.clone())
                .or_default()
                .push(strategy.score(score_aggregation, adjusted_scores));
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
                                cell_config =
                                    try_select_halting_turing_machine_strategies(cell_config)
                                        .map_err(|err| anyhow::anyhow!(err))?;
                                accelerator_run_preflight(
                                    &cell_config,
                                    cell_config.save_data && cell_config.event_log.enabled,
                                    cell_config.save_data && cell_config.history.enabled,
                                    false,
                                )
                                .map_err(|err| anyhow::anyhow!(err))?;

                                let config_text_cell = toml::to_string(&cell_config)
                                    .unwrap_or_else(|_| config_text.clone());
                                let run_id = run_id_from_seed_config(cell_seed, &config_text_cell);
                                let noise_label = format!("{noise:.4}").replace('.', "_");
                                let cell_dir = cells_root.join(format!(
                                    "{cell_id:04}__r{rounds}__n{noise_label}__rep{reps}__R{r}__S{s}__T{t}__P{p}"
                                ));
                                let cell_dir_display = cell_dir.display().to_string();
                                fs::create_dir_all(&cell_dir).with_context(|| {
                                    format!("failed to create {cell_dir_display}")
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
                                                &mut scores_by_strategy,
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
                                let effective_cell_config = kernel.config().clone();
                                let (results, runtime, event_log, history_log) =
                                    execute_tournament(
                                        &kernel,
                                        cell_config
                                            .event_log
                                            .enabled
                                            .then_some(events_path.clone()),
                                        cell_config.history.enabled.then_some(history_path.clone()),
                                    )?;

                                if let Err(err) =
                                    nit_utils::fs::write_atomic(&definitions_path, |writer| {
                                        serde_json::to_writer_pretty(writer, kernel.definitions())
                                            .map_err(std::io::Error::other)
                                    })
                                {
                                    eprintln!("Warning: failed to write definitions: {err}");
                                }
                                if let Err(err) =
                                    nit_utils::fs::write_atomic(&results_path, |writer| {
                                        serde_json::to_writer_pretty(writer, &results)
                                            .map_err(std::io::Error::other)
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
                                    config: effective_cell_config.clone(),
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
                                    runtime,
                                    run_dir: Some(cell_dir.display().to_string()),
                                };

                                write_summary(&summary_path, &summary).with_context(|| {
                                    format!("failed to write {}", summary_path.display())
                                })?;

                                let (top_id, top_entries) = collect_results(
                                    &results,
                                    &mut scores_by_strategy,
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
    for (id, scores) in scores_by_strategy {
        let count = scores.len() as f64;
        let mean = scores.iter().sum::<f64>() / count.max(1.0);
        let var = scores
            .iter()
            .map(|v| {
                let diff = *v - mean;
                diff * diff
            })
            .sum::<f64>()
            / count.max(1.0);
        let top_count = top_counts.get(&id).copied().unwrap_or(0);
        strategies.push(SweepStrategyAggregate {
            id,
            mean_score: mean,
            std_score: var.sqrt(),
            top1_count: top_count,
        });
    }
    strategies.sort_by(|a, b| b.mean_score.partial_cmp(&a.mean_score).unwrap());

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
        aggregate: SweepAggregate {
            score_aggregation,
            adjusted_scores,
            strategies,
        },
    };

    let summary_path = sweep_root.join("sweep_summary.json");
    nit_utils::fs::write_atomic(&summary_path, |writer| {
        serde_json::to_writer_pretty(writer, &summary).map_err(std::io::Error::other)
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
        .ok_or_else(|| anyhow::anyhow!("strategy '{id}' not found"))?;
    let intro = introspect_strategy(&spec);
    let output = match format {
        OutputFormat::Json => serde_json::to_string(&intro)?,
        OutputFormat::Pretty => format_strategy_introspection(&intro).join("\n"),
    };

    if let Some(out_path) = out {
        if let Some(parent) = out_path.parent() {
            if !parent.as_os_str().is_empty() {
                let parent_display = parent.display().to_string();
                fs::create_dir_all(parent)
                    .with_context(|| format!("failed to create directory {parent_display}"))?;
            }
        }
        let out_path_display = out_path.display().to_string();
        fs::write(&out_path, output)
            .with_context(|| format!("failed to write {out_path_display}"))?;
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
        let run_path_display = run_path.display().to_string();
        let run_text = core_io::load_to_string(&run_path)
            .with_context(|| format!("failed to read {run_path_display}"))?;
        let summary: RunSummary = serde_json::from_str(&run_text)
            .with_context(|| format!("failed to parse {run_path_display}"))?;
        summary
            .config
            .strategies
            .iter()
            .find(|spec| spec.id == strategy_id)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("strategy '{strategy_id}' not found"))?
    } else {
        let config_path = config_path.unwrap_or_else(|| PathBuf::from("games.toml"));
        let config_path_display = config_path.display().to_string();
        let config_text = core_io::load_to_string(&config_path)
            .with_context(|| format!("failed to read {config_path_display}"))?;
        let config = GamesConfig::from_toml_with_root(&config_text, config_path.parent())
            .map_err(|err| anyhow::anyhow!(err))?;
        config
            .strategies
            .iter()
            .find(|spec| spec.id == strategy_id)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("strategy '{strategy_id}' not found"))?
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
            serde_json::to_writer_pretty(writer, &graph).map_err(io::Error::other)
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
        let start_id = dot_id(start);
        dot.push_str(&format!("  start -> {start_id};\n"));
    }
    for node in &graph.nodes {
        let label = node.label.replace('"', "\\\"");
        let node_id = dot_id(&node.id);
        dot.push_str(&format!("  {node_id} [label=\"{label}\"];\n"));
    }
    for edge in &graph.edges {
        let label = edge.label.replace('"', "\\\"");
        let mut attrs = vec![format!("label=\"{label}\"")];
        if let Some(color) = &edge.color {
            attrs.push(format!("color=\"{color}\""));
            attrs.push(format!("fontcolor=\"{color}\""));
        }
        let from = dot_id(&edge.from);
        let to = dot_id(&edge.to);
        let attrs_joined = attrs.join(", ");
        dot.push_str(&format!("  {from} -> {to} [{attrs_joined}];\n"));
    }
    dot.push_str("}\n");
    dot
}

fn dot_id(raw: &str) -> String {
    let escaped = raw.replace('"', "\\\"");
    format!("\"{escaped}\"")
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
            "invalid input_mode '{raw}': expected opponent_last_action, self_last_action, or joint_last_action"
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
    score_aggregation: ScoreAggregation,
    adjusted_scores: bool,
    strategies: Vec<SweepStrategyAggregate>,
}

#[derive(Serialize)]
struct SweepStrategyAggregate {
    id: String,
    mean_score: f64,
    std_score: f64,
    top1_count: u32,
}

#[derive(Serialize)]
struct SweepTopEntry {
    id: String,
    score: f64,
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
    let filename = format!("{hash:016x}.md");
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
                        let _ = writeln!(file, "{msg}");
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
                        let _ = writeln!(file, "{line}");
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
    let filename = format!("{hash:016x}.log");
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

fn load_only_codex_agents() -> nit_core::AgentsState {
    load_agents_from_codex_models_cache().unwrap_or_else(|err| {
        let mut agents = nit_core::AgentsState::default();
        agents.alerts.push(nit_core::AgentAlert {
            severity: nit_core::AgentAlertSeverity::Warn,
            source: "codex".into(),
            message: format!("Failed to load Codex models: {err}"),
            at: "t+0".into(),
        });
        agents
    })
}

fn load_only_claude_agents() -> nit_core::AgentsState {
    let mut agents = nit_core::AgentsState::default();
    if !claude_cli_available() {
        agents.alerts.push(nit_core::AgentAlert {
            severity: nit_core::AgentAlertSeverity::Warn,
            source: "claude".into(),
            message: "Claude CLI not found in PATH.".into(),
            at: "t+0".into(),
        });
        return agents;
    }
    agents.agents.push(nit_core::AgentLane {
        id: "claude".into(),
        role: "Claude".into(),
        lane: "Claude".into(),
        kind: nit_core::AgentLaneKind::Claude,
        status: nit_core::AgentStatus::Idle,
        heartbeat_age_secs: 0,
        queue_len: 0,
        current_mission: None,
        last_message: "Claude backend detected.".into(),
    });
    agents.selected_agent = Some("claude".into());
    agents.roster_selected = 0;
    agents
}

fn load_local_agent_lane() -> nit_core::AgentsState {
    let mut agents = nit_core::AgentsState::default();
    agents.agents.push(nit_core::AgentLane {
        id: "local".into(),
        role: "Local".into(),
        lane: "Local".into(),
        kind: nit_core::AgentLaneKind::Mock,
        status: nit_core::AgentStatus::Idle,
        heartbeat_age_secs: 0,
        queue_len: 0,
        current_mission: None,
        last_message: "Built-in local lane.".into(),
    });
    agents.selected_agent = Some("local".into());
    agents.roster_selected = 0;
    agents
}

fn load_all_available_agents() -> nit_core::AgentsState {
    let mut agents = nit_core::AgentsState::default();
    agents.agents.extend(load_local_agent_lane().agents);

    if claude_cli_available() {
        agents.agents.push(nit_core::AgentLane {
            id: "claude".into(),
            role: "Claude".into(),
            lane: "Claude".into(),
            kind: nit_core::AgentLaneKind::Claude,
            status: nit_core::AgentStatus::Idle,
            heartbeat_age_secs: 0,
            queue_len: 0,
            current_mission: None,
            last_message: "Claude backend detected.".into(),
        });
    }

    if codex_cli_available() {
        match load_agents_from_codex_models_cache() {
            Ok(mut codex_agents) => {
                agents
                    .codex_effective_context_window_tokens
                    .extend(codex_agents.codex_effective_context_window_tokens.drain());
                agents
                    .codex_default_reasoning_effort
                    .extend(codex_agents.codex_default_reasoning_effort.drain());
                agents
                    .codex_supported_reasoning_efforts
                    .extend(codex_agents.codex_supported_reasoning_efforts.drain());
                agents
                    .codex_selected_reasoning_effort
                    .extend(codex_agents.codex_selected_reasoning_effort.drain());
                agents.agents.append(&mut codex_agents.agents);
                agents.mcp = codex_agents.mcp;
            }
            Err(err) => {
                agents.alerts.push(nit_core::AgentAlert {
                    severity: nit_core::AgentAlertSeverity::Warn,
                    source: "codex".into(),
                    message: format!("Failed to load Codex models: {err}"),
                    at: "t+0".into(),
                });
            }
        }
    }

    agents.selected_agent = agents.agents.first().map(|a| a.id.clone());
    agents.roster_selected = 0;
    agents
}

fn codex_cli_available() -> bool {
    is_executable_in_path("codex")
}

fn claude_cli_available() -> bool {
    is_executable_in_path("claude")
}

fn gemini_cli_available() -> bool {
    is_executable_in_path("gemini")
}

fn probe_claude_models() -> (Vec<String>, Option<String>) {
    let (models, error) = probe_models_from_cli(
        "claude",
        &[
            &["models", "--json"],
            &["models"],
            &["list-models"],
            &["--list-models"],
        ],
    );
    let models = select_current_claude_models(models);
    if !models.is_empty() {
        return (models, None);
    }

    if let Some(models) = probe_claude_models_from_install() {
        let models = select_current_claude_models(models);
        return (models, None);
    }

    (models, error)
}

fn probe_gemini_models() -> (Vec<String>, Option<String>) {
    let (models, error) = probe_models_from_cli(
        "gemini",
        &[
            &["models", "--json"],
            &["models"],
            &["list-models"],
            &["--list-models"],
            &["--models"],
        ],
    );
    let models = select_current_gemini_models(models);
    if !models.is_empty() {
        return (models, None);
    }

    if let Some(models) = probe_gemini_models_from_install() {
        let models = select_current_gemini_models(models);
        return (models, None);
    }

    (models, error)
}

fn probe_models_from_cli(bin: &str, attempts: &[&[&str]]) -> (Vec<String>, Option<String>) {
    let timeout = Duration::from_millis(1500);
    let mut last_err: Option<String> = None;

    for args in attempts {
        match run_command_capture_timeout(bin, args, timeout) {
            Ok((status, stdout, stderr)) => {
                if !status.success() {
                    let err = String::from_utf8_lossy(&stderr).trim().to_string();
                    last_err = Some(if err.is_empty() {
                        format!("{bin} {} exited with {status}", args.join(" "))
                    } else {
                        err
                    });
                    continue;
                }

                let models = parse_model_list_from_output(&stdout);
                if !models.is_empty() {
                    return (models, None);
                }

                let err = String::from_utf8_lossy(&stderr).trim().to_string();
                last_err = Some(if err.is_empty() {
                    format!("{bin} {} returned no models", args.join(" "))
                } else {
                    err
                });
            }
            Err(err) => {
                last_err = Some(err.to_string());
            }
        }
    }

    (Vec::new(), last_err)
}

fn sync_backend_model_lanes(agents: &mut nit_core::AgentsState, agents_arg: AgentsArg) {
    let wants_claude = matches!(agents_arg, AgentsArg::All | AgentsArg::Claude);
    let wants_gemini = matches!(agents_arg, AgentsArg::All);
    let replace_claude = wants_claude && !agents.claude_models.is_empty();
    let replace_gemini = wants_gemini && !agents.gemini_models.is_empty();

    if !replace_claude && !replace_gemini {
        return;
    }

    let selected_agent = agents.selected_agent.clone();
    let mut updated: Vec<nit_core::AgentLane> = Vec::with_capacity(
        agents.agents.len()
            + agents
                .claude_models
                .len()
                .saturating_add(agents.gemini_models.len()),
    );

    for lane in agents.agents.drain(..) {
        if replace_claude && matches!(lane.kind, nit_core::AgentLaneKind::Claude) {
            continue;
        }
        if replace_gemini && matches!(lane.kind, nit_core::AgentLaneKind::Gemini) {
            continue;
        }
        updated.push(lane);
    }

    if replace_claude {
        for model in agents.claude_models.iter() {
            updated.push(nit_core::AgentLane {
                id: model.clone(),
                role: model.clone(),
                lane: "Claude".into(),
                kind: nit_core::AgentLaneKind::Claude,
                status: nit_core::AgentStatus::Idle,
                heartbeat_age_secs: 0,
                queue_len: 0,
                current_mission: None,
                last_message: String::new(),
            });
        }
    }

    if replace_gemini {
        for model in agents.gemini_models.iter() {
            updated.push(nit_core::AgentLane {
                id: model.clone(),
                role: model.clone(),
                lane: "Gemini".into(),
                kind: nit_core::AgentLaneKind::Gemini,
                status: nit_core::AgentStatus::Idle,
                heartbeat_age_secs: 0,
                queue_len: 0,
                current_mission: None,
                last_message: String::new(),
            });
        }
    }

    agents.agents = updated;

    if let Some(selected) = selected_agent {
        if let Some(idx) = agents.agents.iter().position(|lane| lane.id == selected) {
            agents.selected_agent = Some(selected);
            agents.roster_selected = idx;
            return;
        }
    }

    agents.selected_agent = agents.agents.first().map(|lane| lane.id.clone());
    agents.roster_selected = 0;
}

/// Populate Claude model metadata (context windows, effort levels) for all probed models.
fn populate_claude_model_metadata(agents: &mut nit_core::AgentsState) {
    for model in agents.claude_models.iter() {
        // Determine context window based on model name.
        // Models with "[1m]" suffix have 1M context; others default to 200k.
        let context_window: u32 = if model.contains("[1m]") || model.contains("1m") {
            1_000_000
        } else {
            200_000
        };
        agents
            .claude_effective_context_window_tokens
            .insert(model.clone(), context_window);

        // Determine supported effort levels. "max" is only available on Opus models.
        let is_opus = model.to_lowercase().contains("opus");
        let supported = if is_opus {
            vec![
                "low".to_string(),
                "medium".to_string(),
                "high".to_string(),
                "max".to_string(),
            ]
        } else {
            vec![
                "low".to_string(),
                "medium".to_string(),
                "high".to_string(),
            ]
        };
        agents
            .claude_supported_efforts
            .insert(model.clone(), supported);

        // Default effort: "high" for all Claude models.
        agents
            .claude_default_effort
            .insert(model.clone(), "high".to_string());
        // Initialize selected effort to default.
        agents
            .claude_selected_effort
            .insert(model.clone(), "high".to_string());
    }
}

fn run_command_capture_timeout(
    bin: &str,
    args: &[&str],
    timeout: Duration,
) -> io::Result<(std::process::ExitStatus, Vec<u8>, Vec<u8>)> {
    let executable = find_executable_in_path(bin).unwrap_or_else(|| PathBuf::from(bin));
    let mut command = ProcessCommand::new(&executable);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(path_override) = preferred_path_for_executable(&executable) {
        command.env("PATH", path_override);
    }
    let mut child = command.spawn()?;

    let start = Instant::now();
    loop {
        if let Some(status) = child.try_wait()? {
            let mut stdout = Vec::new();
            let mut stderr = Vec::new();
            if let Some(mut out) = child.stdout.take() {
                let _ = out.read_to_end(&mut stdout);
            }
            if let Some(mut err) = child.stderr.take() {
                let _ = err.read_to_end(&mut stderr);
            }
            return Ok((status, stdout, stderr));
        }

        if start.elapsed() >= timeout {
            let _ = child.kill();
            let _ = child.wait();
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                format!("{bin} {} timed out after {timeout:?}", args.join(" ")),
            ));
        }
        thread::sleep(Duration::from_millis(25));
    }
}

fn probe_gemini_models_from_install() -> Option<Vec<String>> {
    let executable = find_executable_in_path("gemini")?;
    let resolved = fs::canonicalize(executable).ok()?;
    let package_root = resolved.parent()?.parent()?;
    let models_js = package_root
        .join("node_modules")
        .join("@google")
        .join("gemini-cli-core")
        .join("dist")
        .join("src")
        .join("config")
        .join("models.js");
    let source = fs::read_to_string(models_js).ok()?;
    let models = parse_gemini_models_from_source(&source);
    if models.is_empty() {
        None
    } else {
        Some(models)
    }
}

fn probe_claude_models_from_install() -> Option<Vec<String>> {
    let executable = find_executable_in_path("claude")?;
    let bytes = fs::read(executable).ok()?;
    let models = parse_claude_models_from_binary(&bytes);
    if models.is_empty() {
        None
    } else {
        Some(models)
    }
}

fn parse_claude_models_from_binary(bytes: &[u8]) -> Vec<String> {
    let ascii_runs = extract_ascii_runs(bytes);

    let mut models = Vec::new();
    for pair in ascii_runs.windows(2) {
        let Some(candidate) = normalize_claude_model_token(&pair[0]) else {
            continue;
        };
        if looks_like_claude_model_label(&pair[1]) {
            models.push(candidate.to_string());
        }
    }
    models.sort();
    models.dedup();
    models
}

fn extract_ascii_runs(bytes: &[u8]) -> Vec<String> {
    let mut ascii_runs = Vec::new();
    let mut start = None;

    for (idx, &byte) in bytes.iter().enumerate() {
        if byte.is_ascii_graphic() || byte == b' ' {
            if start.is_none() {
                start = Some(idx);
            }
            continue;
        }
        if let Some(run_start) = start.take() {
            if idx.saturating_sub(run_start) >= 8 {
                ascii_runs.push(String::from_utf8_lossy(&bytes[run_start..idx]).into_owned());
            }
        }
    }
    if let Some(run_start) = start {
        if bytes.len().saturating_sub(run_start) >= 8 {
            ascii_runs.push(String::from_utf8_lossy(&bytes[run_start..]).into_owned());
        }
    }

    ascii_runs
}

fn parse_gemini_models_from_source(source: &str) -> Vec<String> {
    let mut named_values = HashMap::new();
    for line in source.lines() {
        let trimmed = line.trim();
        let Some(rest) = trimmed.strip_prefix("export const ") else {
            continue;
        };
        let Some((name, value)) = rest.split_once('=') else {
            continue;
        };
        let name = name.trim();
        let value = value.trim().trim_end_matches(';').trim();
        if let Some(value) = parse_single_quoted_literal(value) {
            named_values.insert(name.to_string(), value.to_string());
        }
    }

    let marker = "export const VALID_GEMINI_MODELS = new Set([";
    let Some(start) = source.find(marker) else {
        return Vec::new();
    };
    let remainder = &source[start + marker.len()..];
    let Some(end) = remainder.find("]);") else {
        return Vec::new();
    };
    let mut models = Vec::new();
    for entry in remainder[..end].split(',') {
        let entry = entry.trim();
        if entry.is_empty() {
            continue;
        }
        if let Some(value) = parse_single_quoted_literal(entry) {
            models.push(value.to_string());
            continue;
        }
        if let Some(value) = named_values.get(entry) {
            models.push(value.clone());
        }
    }
    models.sort();
    models.dedup();
    models
}

fn select_current_claude_models(models: Vec<String>) -> Vec<String> {
    let mut original = models;
    original.sort();
    original.dedup();

    let mut latest_by_family: HashMap<&'static str, (Vec<u32>, String)> = HashMap::new();
    for model in original.iter() {
        let Some((family, version)) = parse_claude_family_and_version(model) else {
            continue;
        };
        match latest_by_family.entry(family) {
            std::collections::hash_map::Entry::Vacant(entry) => {
                entry.insert((version, model.clone()));
            }
            std::collections::hash_map::Entry::Occupied(mut entry) => {
                let (current_version, current_model) = entry.get();
                if version > *current_version
                    || (version == *current_version
                        && prefer_shorter_model_name(model, current_model))
                {
                    entry.insert((version, model.clone()));
                }
            }
        }
    }

    if latest_by_family.is_empty() {
        return original;
    }

    let mut current: Vec<String> = latest_by_family
        .into_values()
        .map(|(_version, model)| model)
        .collect();
    current.sort();
    current
}

fn select_current_gemini_models(models: Vec<String>) -> Vec<String> {
    let mut original = models;
    original.sort();
    original.dedup();

    let mut latest_by_family: HashMap<&'static str, (bool, Vec<u32>, String)> = HashMap::new();
    for model in original.iter() {
        let Some((family, preview, version)) = parse_gemini_family_preview_and_version(model)
        else {
            continue;
        };
        match latest_by_family.entry(family) {
            std::collections::hash_map::Entry::Vacant(entry) => {
                entry.insert((preview, version, model.clone()));
            }
            std::collections::hash_map::Entry::Occupied(mut entry) => {
                let (current_preview, current_version, current_model) = entry.get();
                let better = (*current_preview && !preview)
                    || (*current_preview == preview
                        && (version > *current_version
                            || (version == *current_version
                                && prefer_shorter_model_name(model, current_model))));
                if better {
                    entry.insert((preview, version, model.clone()));
                }
            }
        }
    }

    if latest_by_family.is_empty() {
        return original;
    }

    let mut current: Vec<String> = latest_by_family
        .into_values()
        .map(|(_preview, _version, model)| model)
        .collect();
    current.sort();
    current
}

fn normalize_claude_model_token(raw: &str) -> Option<&str> {
    let candidate = raw.trim().strip_suffix("[1m]").unwrap_or(raw.trim());
    if is_probable_claude_model(candidate) {
        Some(candidate)
    } else {
        None
    }
}

fn is_probable_claude_model(candidate: &str) -> bool {
    let candidate = candidate.to_ascii_lowercase();
    if !candidate.starts_with("claude-")
        || candidate.ends_with('-')
        || candidate.contains("--")
        || candidate.contains("..")
        || candidate.ends_with("-latest")
        || candidate.contains("-latest-")
        || candidate.contains("-v1")
        || candidate.contains("-v2")
        || candidate.contains("-v3")
    {
        return false;
    }

    if !(candidate.contains("-haiku")
        || candidate.contains("-sonnet")
        || candidate.contains("-opus"))
    {
        return false;
    }

    ![
        "api",
        "sdk",
        "cli",
        "code",
        "plugin",
        "desktop",
        "chrome",
        "agent",
        "guide",
        "github",
        "review",
        "marketplace",
        "settings",
        "context",
        "swarm",
        "folder",
        "hidden",
        "http",
        "staging",
    ]
    .iter()
    .any(|needle| candidate.contains(needle))
}

fn looks_like_claude_model_label(label: &str) -> bool {
    let label = label.trim();
    !label.is_empty()
        && !label.starts_with("claude-")
        && (label.contains("Haiku")
            || label.contains("Sonnet")
            || label.contains("Opus")
            || label.contains("Claude "))
}

fn parse_claude_family_and_version(model: &str) -> Option<(&'static str, Vec<u32>)> {
    let candidate = normalize_claude_model_token(model)?;
    let parts: Vec<&str> = candidate.split('-').collect();
    if parts.first().copied() != Some("claude") || parts.len() < 3 {
        return None;
    }

    for family in ["haiku", "sonnet", "opus"] {
        if parts.get(1).copied() == Some(family) {
            return parse_small_numeric_parts(&parts[2..]).map(|version| (family, version));
        }
        if parts.last().copied() == Some(family) {
            return parse_small_numeric_parts(&parts[1..parts.len().saturating_sub(1)])
                .map(|version| (family, version));
        }
    }

    None
}

fn parse_gemini_family_preview_and_version(model: &str) -> Option<(&'static str, bool, Vec<u32>)> {
    let candidate = model.trim().to_ascii_lowercase();
    let rest = candidate.strip_prefix("gemini-")?;
    let (version, suffix) = rest.split_once('-')?;
    let version = parse_dot_numeric_parts(version)?;
    if suffix.contains("customtools") || suffix.contains("embedding") {
        return None;
    }

    let family = if suffix.contains("flash-lite") {
        "flash-lite"
    } else if suffix.contains("flash") {
        "flash"
    } else if suffix.contains("pro") {
        "pro"
    } else {
        return None;
    };

    Some((family, suffix.contains("preview"), version))
}

fn parse_small_numeric_parts(parts: &[&str]) -> Option<Vec<u32>> {
    if parts.is_empty() {
        return None;
    }
    let mut out = Vec::with_capacity(parts.len());
    for part in parts {
        if part.is_empty() || part.len() > 2 || !part.chars().all(|ch| ch.is_ascii_digit()) {
            return None;
        }
        let value = part.parse::<u32>().ok()?;
        if value > 99 {
            return None;
        }
        out.push(value);
    }
    Some(out)
}

fn parse_dot_numeric_parts(raw: &str) -> Option<Vec<u32>> {
    if raw.is_empty() {
        return None;
    }
    let mut out = Vec::new();
    for part in raw.split('.') {
        if part.is_empty() || !part.chars().all(|ch| ch.is_ascii_digit()) {
            return None;
        }
        out.push(part.parse::<u32>().ok()?);
    }
    Some(out)
}

fn prefer_shorter_model_name(candidate: &str, current: &str) -> bool {
    candidate.len() < current.len() || (candidate.len() == current.len() && candidate < current)
}

fn parse_single_quoted_literal(value: &str) -> Option<&str> {
    let value = value.trim();
    let value = value.strip_prefix('\'')?;
    let value = value.strip_suffix('\'')?;
    if value.is_empty() {
        None
    } else {
        Some(value)
    }
}

fn parse_model_list_from_output(stdout: &[u8]) -> Vec<String> {
    let raw = String::from_utf8_lossy(stdout);
    let raw = raw.trim();
    if raw.is_empty() {
        return Vec::new();
    }

    if let Ok(value) = serde_json::from_str::<serde_json::Value>(raw) {
        let mut out = Vec::new();
        extract_models_from_json(&value, &mut out);
        out.sort();
        out.dedup();
        return out;
    }

    let mut out = Vec::new();
    for line in raw.lines() {
        let mut line = line.trim();
        if line.is_empty() {
            continue;
        }
        line = line
            .trim_start_matches('-')
            .trim_start_matches('*')
            .trim_start_matches('•')
            .trim();
        if line.is_empty() {
            continue;
        }
        let Some(candidate) = line.split_whitespace().next() else {
            continue;
        };
        if candidate.ends_with(':') {
            continue;
        }
        if candidate.eq_ignore_ascii_case("models") || candidate.eq_ignore_ascii_case("model") {
            continue;
        }
        if candidate.len() < 3 {
            continue;
        }
        out.push(candidate.to_string());
    }
    out.sort();
    out.dedup();
    out
}

fn extract_models_from_json(value: &serde_json::Value, out: &mut Vec<String>) {
    match value {
        serde_json::Value::String(s) => {
            let s = s.trim();
            if !s.is_empty() {
                out.push(s.to_string());
            }
        }
        serde_json::Value::Array(arr) => {
            for v in arr {
                extract_models_from_json(v, out);
            }
        }
        serde_json::Value::Object(map) => {
            for key in ["id", "name", "model", "slug"] {
                if let Some(serde_json::Value::String(s)) = map.get(key) {
                    let s = s.trim();
                    if !s.is_empty() {
                        out.push(s.to_string());
                        return;
                    }
                }
            }
            for key in ["models", "data"] {
                if let Some(v) = map.get(key) {
                    extract_models_from_json(v, out);
                }
            }
        }
        _ => {}
    }
}

fn is_executable_in_path(bin: &str) -> bool {
    find_executable_in_path(bin).is_some()
}

fn find_executable_in_path(bin: &str) -> Option<PathBuf> {
    for dir in executable_search_dirs() {
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
                return Some(candidate);
            }
            for ext in exts.iter() {
                let candidate = dir.join(format!("{bin}.{ext}"));
                if candidate.is_file() {
                    return Some(candidate);
                }
            }
        }
        #[cfg(not(windows))]
        {
            let candidate = dir.join(bin);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

fn executable_search_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(path) = std::env::var_os("PATH") {
        dirs.extend(std::env::split_paths(&path));
    }
    if let Some(home) = std::env::var_os("HOME") {
        let home = PathBuf::from(home);
        dirs.push(home.join(".local/bin"));
        dirs.push(home.join("bin"));
    }

    #[cfg(target_os = "macos")]
    {
        dirs.push(PathBuf::from("/opt/homebrew/bin"));
        dirs.push(PathBuf::from("/opt/homebrew/sbin"));
    }

    dirs.push(PathBuf::from("/usr/local/bin"));
    dirs.push(PathBuf::from("/usr/local/sbin"));

    let mut unique = Vec::new();
    for dir in dirs {
        if dir.as_os_str().is_empty() || unique.iter().any(|existing| existing == &dir) {
            continue;
        }
        unique.push(dir);
    }
    unique
}

fn preferred_path_for_executable(executable: &Path) -> Option<OsString> {
    let mut paths = Vec::<PathBuf>::new();
    if let Some(dir) = executable.parent() {
        paths.push(dir.to_path_buf());
    }
    paths.extend(executable_search_dirs());
    let mut deduped = Vec::new();
    for path in paths {
        if deduped.iter().any(|existing| existing == &path) {
            continue;
        }
        deduped.push(path);
    }
    std::env::join_paths(deduped).ok()
}

#[cfg(test)]
mod tests {
    use super::{
        parse_claude_models_from_binary, parse_gemini_models_from_source,
        select_current_claude_models, select_current_gemini_models, sync_backend_model_lanes,
        AgentsArg,
    };

    #[test]
    fn parses_gemini_models_from_backend_source() {
        let source = r#"
            export const PREVIEW_GEMINI_MODEL = 'gemini-3-pro-preview';
            export const DEFAULT_GEMINI_MODEL = 'gemini-2.5-pro';
            export const DEFAULT_GEMINI_FLASH_MODEL = 'gemini-2.5-flash';
            export const VALID_GEMINI_MODELS = new Set([
                PREVIEW_GEMINI_MODEL,
                DEFAULT_GEMINI_MODEL,
                DEFAULT_GEMINI_FLASH_MODEL,
            ]);
        "#;

        assert_eq!(
            parse_gemini_models_from_source(source),
            vec![
                "gemini-2.5-flash".to_string(),
                "gemini-2.5-pro".to_string(),
                "gemini-3-pro-preview".to_string(),
            ]
        );
    }

    #[test]
    fn keeps_only_current_gemini_models_by_family() {
        let models = vec![
            "gemini-3-pro-preview".to_string(),
            "gemini-3.1-pro-preview-customtools".to_string(),
            "gemini-2.5-pro".to_string(),
            "gemini-3-flash-preview".to_string(),
            "gemini-2.5-flash".to_string(),
            "gemini-2.5-flash-lite".to_string(),
        ];

        assert_eq!(
            select_current_gemini_models(models),
            vec![
                "gemini-2.5-flash".to_string(),
                "gemini-2.5-flash-lite".to_string(),
                "gemini-2.5-pro".to_string(),
            ]
        );
    }

    #[test]
    fn parses_claude_models_from_backend_binary_strings() {
        let binary = br#"
            foundry
            claude-opus-4-6[1m]
            Opus 4.6 (with 1M context)
            claude-opus-4-6
            Opus 4.6
            claude-opus-4-5
            Opus 4.5
            claude-sonnet-4-6[1m]
            Sonnet 4.6 (with 1M context)
            claude-sonnet-4-6
            Sonnet 4.6
            claude-sonnet-4
            Sonnet 4
            claude-3-7-sonnet
            Claude 3.7 Sonnet
            claude-haiku-4-5
            Haiku 4.5
            haiku45
            sonnet46
            claude-3-5-haiku-20241022
            claude-sonnet-4-20250514
            claude-sonnet-4-latest
            claude-sonnet-4-v2
            claude-code
            claude-plugin-directory
        "#;

        assert_eq!(
            parse_claude_models_from_binary(binary),
            vec![
                "claude-3-7-sonnet".to_string(),
                "claude-haiku-4-5".to_string(),
                "claude-opus-4-5".to_string(),
                "claude-opus-4-6".to_string(),
                "claude-sonnet-4".to_string(),
                "claude-sonnet-4-6".to_string(),
            ]
        );
    }

    #[test]
    fn keeps_only_current_claude_models_by_family() {
        let models = vec![
            "claude-3-5-haiku".to_string(),
            "claude-haiku-4-5".to_string(),
            "claude-3-7-sonnet".to_string(),
            "claude-sonnet-4".to_string(),
            "claude-sonnet-4-6".to_string(),
            "claude-opus-4-5".to_string(),
            "claude-opus-4-6".to_string(),
        ];

        assert_eq!(
            select_current_claude_models(models),
            vec![
                "claude-haiku-4-5".to_string(),
                "claude-opus-4-6".to_string(),
                "claude-sonnet-4-6".to_string(),
            ]
        );
    }

    #[test]
    fn sync_backend_model_lanes_replaces_placeholder_backend_rows() {
        let mut agents = nit_core::AgentsState::default();
        agents.agents.push(nit_core::AgentLane {
            id: "local".into(),
            role: "Local".into(),
            lane: "Local".into(),
            kind: nit_core::AgentLaneKind::Mock,
            status: nit_core::AgentStatus::Idle,
            heartbeat_age_secs: 0,
            queue_len: 0,
            current_mission: None,
            last_message: String::new(),
        });
        agents.agents.push(nit_core::AgentLane {
            id: "claude".into(),
            role: "Claude".into(),
            lane: "Claude".into(),
            kind: nit_core::AgentLaneKind::Claude,
            status: nit_core::AgentStatus::Idle,
            heartbeat_age_secs: 0,
            queue_len: 0,
            current_mission: None,
            last_message: String::new(),
        });
        agents.claude_models = vec!["claude-sonnet-4-6".into(), "claude-opus-4-6".into()];

        sync_backend_model_lanes(&mut agents, AgentsArg::All);

        assert!(agents.agents.iter().any(|lane| lane.id == "local"));
        assert!(agents
            .agents
            .iter()
            .any(|lane| lane.id == "claude-sonnet-4-6"));
        assert!(agents
            .agents
            .iter()
            .any(|lane| lane.id == "claude-opus-4-6"));
        assert!(!agents.agents.iter().any(|lane| lane.id == "claude"));
    }
}
