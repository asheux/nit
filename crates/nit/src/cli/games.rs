use std::path::PathBuf;

use clap::{Args, Subcommand, ValueEnum};

#[derive(Subcommand, Debug)]
pub(crate) enum GamesCommand {
    /// Headless batch runner for games
    #[command(alias = "tournament")]
    Run(RunArgs),
    /// Sweep runner for games (parameter grids)
    Sweep(SweepArgs),
    /// Enumerate strategies (FSMs)
    Enumerate {
        #[command(subcommand)]
        kind: EnumerateCommand,
    },
    /// Inspect a strategy definition
    Inspect(InspectArgs),
    /// Export strategy graph (DOT or JSON)
    Graph(GraphArgs),
}

#[derive(Args, Debug)]
pub(crate) struct RunArgs {
    /// Config path (defaults to games.toml)
    #[arg(long)]
    pub config: Option<PathBuf>,
    /// NDJSON strategy list to append
    #[arg(long)]
    pub strategies: Option<PathBuf>,
    /// Output directory (defaults to ./output)
    #[arg(long)]
    pub out: Option<PathBuf>,
    /// Override seed
    #[arg(long)]
    pub seed: Option<u64>,
    #[command(flatten)]
    pub output: CommonOutputArgs,
}

/// Shared stdout/stderr presentation flags reused by Run and Sweep.
#[derive(Args, Debug)]
pub(crate) struct CommonOutputArgs {
    /// Output format for stdout summary
    #[arg(long, value_enum, default_value_t = OutputFormat::Pretty)]
    pub format: OutputFormat,
    /// Suppress stdout summary
    #[arg(long)]
    pub quiet: bool,
    /// Verbose logging to stderr
    #[arg(long)]
    pub verbose: bool,
}

#[derive(Args, Debug)]
pub(crate) struct SweepArgs {
    /// Config path (defaults to games.toml)
    #[arg(long)]
    pub config: Option<PathBuf>,
    /// NDJSON strategy list to append
    #[arg(long)]
    pub strategies: Option<PathBuf>,
    /// Output directory root (defaults to config directory)
    #[arg(long)]
    pub out: Option<PathBuf>,
    /// Override base seed
    #[arg(long)]
    pub seed: Option<u64>,
    /// Rounds grid (comma-separated)
    #[arg(long, value_delimiter = ',')]
    pub rounds: Vec<u32>,
    /// Noise grid (comma-separated)
    #[arg(long, value_delimiter = ',')]
    pub noise: Vec<f32>,
    /// Repetitions grid (comma-separated)
    #[arg(long, value_delimiter = ',')]
    pub repetitions: Vec<u32>,
    /// Payoff preset (pd, stag_hunt, snowdrift, chicken)
    #[arg(long)]
    pub payoff_preset: Option<String>,
    /// Payoff R grid (comma-separated)
    #[arg(long, value_delimiter = ',')]
    pub payoff_r: Vec<i32>,
    /// Payoff S grid (comma-separated)
    #[arg(long, value_delimiter = ',')]
    pub payoff_s: Vec<i32>,
    /// Payoff T grid (comma-separated)
    #[arg(long, value_delimiter = ',')]
    pub payoff_t: Vec<i32>,
    /// Payoff P grid (comma-separated)
    #[arg(long, value_delimiter = ',')]
    pub payoff_p: Vec<i32>,
    /// Force rerun even if cell output exists
    #[arg(long)]
    pub force: bool,
    #[command(flatten)]
    pub output: CommonOutputArgs,
}

#[derive(Args, Debug)]
pub(crate) struct InspectArgs {
    /// Config path (defaults to games.toml)
    #[arg(long)]
    pub config: Option<PathBuf>,
    /// Strategy id to inspect
    #[arg(long)]
    pub id: String,
    /// Output format (json or pretty text)
    #[arg(long, value_enum, default_value_t = OutputFormat::Pretty)]
    pub format: OutputFormat,
    /// Optional output path (defaults to stdout)
    #[arg(long)]
    pub out: Option<PathBuf>,
}

#[derive(Args, Debug)]
pub(crate) struct GraphArgs {
    /// Config path (defaults to games.toml)
    #[arg(long)]
    pub config: Option<PathBuf>,
    /// Run summary path (optional, overrides --config)
    #[arg(long)]
    pub run: Option<PathBuf>,
    /// Strategy id to graph (alias: --fsm)
    #[arg(long, alias = "fsm")]
    pub id: String,
    /// Output path (.dot/.gv or .json)
    #[arg(long)]
    pub out: PathBuf,
}

#[derive(Subcommand, Debug)]
pub(crate) enum EnumerateCommand {
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
pub(crate) enum OutputFormat {
    Json,
    Pretty,
}
