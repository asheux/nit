use std::path::PathBuf;

use clap::{Parser, Subcommand};

mod agents;
mod codex;
mod games;
mod labs;

pub(crate) use agents::AgentsArg;
pub(crate) use codex::{CodexApprovalPolicyArg, CodexRuntimeArg, CodexSandboxArg};
pub(crate) use games::{
    EnumerateCommand, GamesCommand, GraphArgs, InspectArgs, OutputFormat, RunArgs, SweepArgs,
};
pub(crate) use labs::LabArg;

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
    value.eq_ignore_ascii_case("gol") || value.eq_ignore_ascii_case("games")
}
