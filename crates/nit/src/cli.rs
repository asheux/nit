use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};
use nit_core::LabId;

#[derive(Parser, Debug)]
#[command(
    name = "nit",
    version,
    about = "Neural Interface Terminal",
    subcommand_precedence_over_arg = true
)]
pub(crate) struct Cli {
    /// File or directory to open
    pub path: Option<PathBuf>,

    /// Start in the specified lab (gol or games)
    #[arg(long, value_enum, default_value_t = LabArg::Gol)]
    pub lab: LabArg,

    /// Agent station backend selection (defaults to all available backends)
    #[arg(long, value_enum)]
    pub agents: Option<AgentsArg>,

    /// Codex automation runtime (exec spawns per-turn; mcp keeps a persistent server)
    #[arg(long, value_enum, default_value_t = CodexRuntimeArg::Mcp)]
    pub codex_runtime: CodexRuntimeArg,

    /// Codex sandbox mode (forwarded to Codex runs; default is Codex's own config)
    #[arg(long, value_enum)]
    pub codex_sandbox: Option<CodexSandboxArg>,

    /// Codex approval policy — defaults to `never` because nit drives Codex non-interactively.
    #[arg(long, value_enum, default_value_t = CodexApprovalPolicyArg::Never)]
    pub codex_approval_policy: CodexApprovalPolicyArg,

    /// Max concurrent Codex turns (MCP: in-flight calls; Exec: child processes)
    #[arg(
        long,
        alias = "codex-parallel",
        default_value_t = 2u8,
        value_parser = clap::value_parser!(u8).range(1..=16)
    )]
    pub codex_max_parallel_turns: u8,

    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Subcommand, Debug)]
#[allow(clippy::large_enum_variant)]
pub(crate) enum Command {
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
pub(crate) enum GamesCommand {
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

#[derive(Copy, Clone, Debug, ValueEnum)]
pub(crate) enum AgentsArg {
    #[value(alias = "mock")]
    Local,
    Codex,
    Claude,
    All,
}

#[derive(Copy, Clone, Debug, ValueEnum)]
pub(crate) enum CodexRuntimeArg {
    Exec,
    Mcp,
}

impl From<CodexRuntimeArg> for nit_tui::codex_runner::CodexRuntimeMode {
    fn from(value: CodexRuntimeArg) -> Self {
        match value {
            CodexRuntimeArg::Exec => Self::Exec,
            CodexRuntimeArg::Mcp => Self::Mcp,
        }
    }
}

#[derive(Copy, Clone, Debug, ValueEnum)]
pub(crate) enum CodexSandboxArg {
    ReadOnly,
    WorkspaceWrite,
    DangerFullAccess,
}

impl CodexSandboxArg {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::ReadOnly => "read-only",
            Self::WorkspaceWrite => "workspace-write",
            Self::DangerFullAccess => "danger-full-access",
        }
    }
}

#[derive(Copy, Clone, Debug, ValueEnum)]
pub(crate) enum CodexApprovalPolicyArg {
    Untrusted,
    OnFailure,
    OnRequest,
    Never,
}

impl CodexApprovalPolicyArg {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Untrusted => "untrusted",
            Self::OnFailure => "on-failure",
            Self::OnRequest => "on-request",
            Self::Never => "never",
        }
    }
}

#[derive(Copy, Clone, Debug, ValueEnum)]
pub(crate) enum LabArg {
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

/// Fuse `--lab <value>` into `--lab=<value>` so clap's subcommand_precedence_over_arg
/// doesn't swallow the lab value as a subcommand when it matches a known name.
pub(crate) fn normalize_lab_args<I>(args: I) -> Vec<String>
where
    I: IntoIterator<Item = String>,
{
    let mut iter = args.into_iter();
    let mut out = Vec::new();
    if let Some(bin) = iter.next() {
        out.push(bin);
    }

    while let Some(arg) = iter.next() {
        if arg != "--lab" {
            out.push(arg);
            continue;
        }
        // Fuse `--lab gol` → `--lab=gol` for recognized lab names.
        match iter.next() {
            Some(value) if is_lab_name(&value) => out.push(format!("--lab={value}")),
            Some(value) => {
                out.push(arg);
                out.push(value);
            }
            None => out.push(arg),
        }
    }
    out
}

fn is_lab_name(value: &str) -> bool {
    matches!(value.to_ascii_lowercase().as_str(), "gol" | "games")
}
