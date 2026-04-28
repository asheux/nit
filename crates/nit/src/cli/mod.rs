use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};

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
    /// Multipane mode: a grid of independent chat panes, one cwd each.
    Multipane(MultipaneArgs),
}

#[derive(Args, Debug)]
pub(crate) struct MultipaneArgs {
    /// Backend model id (specific lane like `claude-haiku-4-5`) or family
    /// alias (`codex` / `claude` / `gemini` / `local`). Optional — when
    /// omitted, every pane opens in roster mode showing every available
    /// backend; the operator picks per pane via Up/Down + Enter.
    #[arg(long)]
    pub backend: Option<String>,

    /// Number of panes to open. Clamped to [1, 32]. Grid is roughly
    /// square: ceil(sqrt(N)) columns × ceil(N / cols) rows.
    #[arg(long, default_value_t = 8u8, value_parser = clap::value_parser!(u8).range(1..=32))]
    pub panes: u8,

    /// Starting directory for every pane. Defaults to the current
    /// working directory.
    #[arg(long)]
    pub cwd: Option<PathBuf>,
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

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    fn parse(args: &[&str]) -> Result<Cli, clap::Error> {
        Cli::try_parse_from(args.iter().copied())
    }

    #[test]
    fn multipane_args_parses_with_backend() {
        let cli = parse(&[
            "nit",
            "multipane",
            "--backend",
            "claude-haiku-4-5",
            "--panes",
            "4",
            "--cwd",
            "/tmp",
        ])
        .expect("parses");
        match cli.command {
            Some(Command::Multipane(args)) => {
                assert_eq!(args.backend.as_deref(), Some("claude-haiku-4-5"));
                assert_eq!(args.panes, 4);
                assert_eq!(args.cwd, Some(PathBuf::from("/tmp")));
            }
            other => panic!("expected Multipane, got {other:?}"),
        }
    }

    #[test]
    fn multipane_defaults_eight_panes_and_no_cwd() {
        let cli = parse(&["nit", "multipane", "--backend", "gpt-5"]).expect("parses");
        match cli.command {
            Some(Command::Multipane(args)) => {
                assert_eq!(args.panes, 8);
                assert_eq!(args.cwd, None);
            }
            other => panic!("expected Multipane, got {other:?}"),
        }
    }

    #[test]
    fn multipane_no_backend_now_accepted() {
        let cli = parse(&["nit", "multipane"]).expect("parses without --backend");
        match cli.command {
            Some(Command::Multipane(args)) => {
                assert!(args.backend.is_none());
                assert_eq!(args.panes, 8);
                assert_eq!(args.cwd, None);
            }
            other => panic!("expected Multipane, got {other:?}"),
        }
    }

    #[test]
    fn multipane_with_backend_family() {
        let cli = parse(&["nit", "multipane", "--backend", "claude"]).expect("parses");
        match cli.command {
            Some(Command::Multipane(args)) => {
                assert_eq!(args.backend.as_deref(), Some("claude"));
                assert_eq!(args.panes, 8);
            }
            other => panic!("expected Multipane, got {other:?}"),
        }
    }

    #[test]
    fn multipane_panes_zero_rejected() {
        let err = parse(&["nit", "multipane", "--backend", "x", "--panes", "0"])
            .expect_err("--panes 0 must be rejected");
        assert!(err.to_string().contains("0"));
    }

    #[test]
    fn multipane_panes_thirtythree_rejected() {
        let err = parse(&["nit", "multipane", "--backend", "x", "--panes", "33"])
            .expect_err("--panes 33 must be rejected");
        assert!(err.to_string().contains("33"));
    }

    #[test]
    fn multipane_panes_at_bounds_accepted() {
        for p in [1u8, 32u8] {
            let cli = parse(&[
                "nit",
                "multipane",
                "--backend",
                "x",
                "--panes",
                &p.to_string(),
            ])
            .expect("parses at bound");
            match cli.command {
                Some(Command::Multipane(args)) => assert_eq!(args.panes, p),
                _ => panic!("expected Multipane"),
            }
        }
    }
}
