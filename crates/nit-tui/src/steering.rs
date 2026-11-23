//! Acknowledgements for additional input addressed to a specific running turn.

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SteerOutcome {
    /// The provider accepted the input on the named, still-active turn.
    Accepted,
    /// The session acknowledged the message. Its own result will follow.
    Received,
    Completed {
        message: String,
        failed: bool,
    },
    /// The expected turn ended before the provider accepted the input.
    Stale,
    /// The provider rejected the input or nothing was written.
    Unavailable(String),
    /// Input may have been accepted. Automatic replay would risk duplication.
    Unknown(String),
}

/// Match the operator's message after accounting for Nit-supplied context.
pub(crate) fn matches_chat_prompt(provider_prompt: &str, operator_prompt: &str) -> bool {
    if operator_prompt.is_empty() {
        return false;
    }
    if provider_prompt == operator_prompt {
        return true;
    }
    let hint = format!("\n{}", nit_core::skills::reference_hint());
    let body = provider_prompt
        .strip_suffix(&hint)
        .unwrap_or(provider_prompt);
    let hygiene = crate::swarm::code_hygiene_preamble("").unwrap_or_default();
    let body = body.strip_prefix(&hygiene).unwrap_or(body);
    body == operator_prompt
        || body
            .strip_prefix("\n[genome context]\n")
            .and_then(|body| body.split_once("[/genome context]\n\n"))
            .is_some_and(|(_, text)| text == operator_prompt)
}

use crate::{
    claude_runner::{ClaudeLiveTurn, ClaudeRunner},
    codex_runner::{CodexLiveTurn, CodexRunner},
};
use std::{
    path::Path,
    sync::mpsc::{self, Receiver},
};

pub(crate) enum LiveTurn {
    Codex(CodexLiveTurn),
    Claude(ClaudeLiveTurn),
}

pub(crate) fn live_turn(
    codex: Option<&CodexRunner>,
    claude: Option<&ClaudeRunner>,
    agent: &str,
    mission: Option<&str>,
    cwd: &Path,
) -> Option<LiveTurn> {
    codex
        .and_then(|r| r.live_turn(agent, mission, cwd))
        .map(LiveTurn::Codex)
        .or_else(|| {
            claude
                .and_then(|r| r.live_turn(agent, mission, cwd))
                .map(LiveTurn::Claude)
        })
}

impl LiveTurn {
    pub(crate) fn matches_task_turn(&self, nonce: &str) -> bool {
        matches!(self, Self::Codex(turn) if turn.matches_task_turn(nonce))
    }

    pub(crate) fn matches_chat_prompt(&self, text: &str) -> bool {
        match self {
            Self::Codex(turn) => turn.matches_chat_prompt(text),
            Self::Claude(turn) => turn.matches_chat_prompt(text),
        }
    }

    pub(crate) fn send(
        self,
        codex: Option<&CodexRunner>,
        claude: Option<&ClaudeRunner>,
        id: String,
        text: String,
    ) -> Receiver<SteerOutcome> {
        match (self, codex, claude) {
            (Self::Codex(turn), Some(runner), _) => runner.steer(turn, id, text),
            (Self::Claude(turn), _, Some(runner)) => runner.steer(turn, id, text),
            _ => {
                let (tx, rx) = mpsc::channel();
                let _ = tx.send(SteerOutcome::Unavailable(
                    "Runner is no longer connected".into(),
                ));
                rx
            }
        }
    }
}

pub(crate) fn message_id() -> Option<String> {
    let mut b = [0u8; 16];
    getrandom::getrandom(&mut b).ok()?;
    b[6] = (b[6] & 15) | 64;
    b[8] = (b[8] & 63) | 128;
    let n = u128::from_be_bytes(b);
    Some(format!(
        "{:08x}-{:04x}-{:04x}-{:04x}-{:012x}",
        n >> 96,
        (n >> 80) & 0xffff,
        (n >> 64) & 0xffff,
        (n >> 48) & 0xffff,
        n & 0xffffffffffff
    ))
}
