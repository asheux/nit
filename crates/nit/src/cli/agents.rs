use clap::ValueEnum;

#[derive(Copy, Clone, Debug, ValueEnum)]
pub(crate) enum AgentsArg {
    #[value(alias = "mock")]
    Local,
    Codex,
    Claude,
    All,
}
