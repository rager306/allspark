//! S18: serialized tool-call execution.
//!
//! Serena's `TaskExecutor` funnels the agent's (possibly batched) tool calls
//! through a single queue so a batch never races on a shared resource (a
//! language-server client, an edit buffer). This port keeps the same contract:
//! [`TaskExecutor::submit`] runs one call at a time, in submission order, with
//! an optional timeout.

use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Mutex;

/// A submission failure.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TaskError {
    /// The call exceeded the executor's timeout.
    #[error("task timed out after {0:?}")]
    Timeout(Duration),
    /// The call's future was cancelled (the queue was dropped).
    #[error("task cancelled")]
    Cancelled,
}

/// S18: a FIFO serializer for tool calls.
///
/// Cloning shares the same queue, so every clone serializes against the same
/// lock. The optional timeout bounds a single call.
#[derive(Debug, Clone)]
pub struct TaskExecutor {
    gate: Arc<Mutex<()>>,
    timeout: Option<Duration>,
}

impl Default for TaskExecutor {
    fn default() -> Self {
        Self::new(None)
    }
}

impl TaskExecutor {
    /// A serializer with an optional per-call timeout.
    #[must_use]
    pub fn new(timeout: Option<Duration>) -> Self {
        Self {
            gate: Arc::new(Mutex::new(())),
            timeout,
        }
    }

    /// The per-call timeout, if any.
    #[must_use]
    pub fn timeout(&self) -> Option<Duration> {
        self.timeout
    }

    /// Submit a call; it runs once the calls submitted before it have
    /// finished.
    ///
    /// # Errors
    ///
    /// Returns [`TaskError::Timeout`] when the call outlives the configured
    /// timeout.
    pub async fn submit<F, T>(&self, call: F) -> Result<T, TaskError>
    where
        F: Future<Output = T>,
    {
        let _guard = self.gate.lock().await;
        match self.timeout {
            Some(limit) => tokio::time::timeout(limit, call)
                .await
                .map_err(|_| TaskError::Timeout(limit)),
            None => Ok(call.await),
        }
    }

    /// Submit a call that may itself fail; the timeout wraps the whole
    /// `Result`.
    ///
    /// # Errors
    ///
    /// Propagates the call's own error, or [`TaskError::Timeout`].
    pub async fn submit_result<F, T, E>(&self, call: F) -> Result<T, SubmitError<E>>
    where
        F: Future<Output = Result<T, E>>,
    {
        match self.submit(call).await {
            Ok(Ok(value)) => Ok(value),
            Ok(Err(error)) => Err(SubmitError::Call(error)),
            Err(timeout) => Err(SubmitError::Executor(timeout)),
        }
    }
}

/// The error of [`TaskExecutor::submit_result`]: either the call's own error
/// or an executor error.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SubmitError<E> {
    /// The call returned an error.
    #[error(transparent)]
    Call(E),
    /// The executor refused the call.
    #[error(transparent)]
    Executor(TaskError),
}
