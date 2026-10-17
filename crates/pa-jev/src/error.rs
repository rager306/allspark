//! The judgment client's typed errors. The chain layer reads two facts off
//! these variants: account rejection (401/402/403 - the candidate cools
//! down) and abort-or-timeout (the failure rethrows without fallback).

/// One judgment attempt's failure.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum JevError {
    /// The judgment was cancelled: a sibling chunk failed, or the caller
    /// dropped the request future.
    #[error("judgment aborted")]
    Aborted,
    /// Every attempt timed out. A timeout is never a candidate-quality
    /// signal (the endpoint may just be slow), so the chain rethrows it
    /// instead of spending the next candidate.
    #[error("judgment timed out after {attempts} attempts")]
    Timeout { attempts: u32 },
    /// Transport failure (no HTTP response).
    #[error("judgment connection failed: {message}")]
    Connection { message: String },
    /// A non-transient HTTP failure (400/401/402/403/404/...): do not retry.
    #[error("judgment API error ({status}): {body}")]
    Api { status: u16, body: String },
    /// A transient HTTP failure (408/429/5xx) that survived every retry.
    #[error("judgment API kept failing after {attempts} attempts (last status {status}): {body}")]
    RetriesExhausted {
        status: u16,
        attempts: u32,
        body: String,
    },
    /// The response decoded but violated a postcondition (an unanswerable
    /// payload, a missing or mistyped answer).
    #[error("malformed judgment response: {detail}")]
    Malformed { detail: String },
    /// A split judgment failed after some chunks had already been billed;
    /// `partial_usage` is what the survivors spent (the omp `onPartialUsage`
    /// contract, carried in the error so no callback plumbing is needed).
    #[error("chunked judgment failed after partial usage of {partial_cost_usd:.6} USD")]
    ChunkFailed {
        partial_cost_usd: f64,
        #[source]
        source: Box<JevError>,
    },
}

impl JevError {
    /// The HTTP status, when this failure carries one.
    #[must_use]
    pub fn status(&self) -> Option<u16> {
        match self {
            JevError::Api { status, .. } | JevError::RetriesExhausted { status, .. } => {
                Some(*status)
            }
            JevError::ChunkFailed { source, .. } => source.status(),
            _ => None,
        }
    }

    /// Whether the account rejected the request outright (401/402/403): the
    /// candidate cools down instead of retrying this judgment.
    #[must_use]
    pub fn is_account_rejection(&self) -> bool {
        matches!(self.status(), Some(401..=403))
    }

    /// Whether this failure must rethrow without spending the next
    /// candidate (an abort or a timeout).
    #[must_use]
    pub fn is_abort_or_timeout(&self) -> bool {
        matches!(self, JevError::Aborted | JevError::Timeout { .. })
    }
}
