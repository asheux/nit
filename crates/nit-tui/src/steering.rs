//! Acknowledgements for additional input addressed to a specific running turn.

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SteerOutcome {
    /// The provider accepted the input on the named, still-active turn.
    Accepted,
    /// The session acknowledged the message. Its own result will follow.
    Received,
    Completed { message: String, failed: bool },
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
    let body = provider_prompt.strip_suffix(&hint).unwrap_or(provider_prompt);
    let hygiene = crate::swarm::code_hygiene_preamble("").unwrap_or_default();
    let body = body.strip_prefix(&hygiene).unwrap_or(body);
    body == operator_prompt
        || body.strip_prefix("\n[genome context]\n")
            .and_then(|body| body.split_once("[/genome context]\n\n"))
            .is_some_and(|(_, text)| text == operator_prompt)
}
