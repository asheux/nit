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
