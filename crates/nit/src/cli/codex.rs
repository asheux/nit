use clap::ValueEnum;

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
    pub(crate) const fn as_str(self) -> &'static str {
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
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Untrusted => "untrusted",
            Self::OnFailure => "on-failure",
            Self::OnRequest => "on-request",
            Self::Never => "never",
        }
    }
}
