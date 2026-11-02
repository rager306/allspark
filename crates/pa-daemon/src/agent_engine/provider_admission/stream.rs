//! The stream wrapper: one admission slot owned for exactly one LLM stream's
//! lifetime.
//!
//! The narrow bracket is the whole point (oh-my-pi #3749). A slot held for a
//! worker's lifetime deadlocks spawn trees, so this wrapper takes the slot before
//! the provider call and gives it back when that stream settles, errors, aborts,
//! or is dropped — never later.

use std::sync::Arc;

use pa_agent::stream::{LlmContext, ModelStream, StreamFn, StreamRequestOptions};
use pa_agent::types::{AssistantMessage, Model as AgentModel};

use super::breaker::breaker_refusal_message;
use super::client::{AdmissionClient, KeyResolver, StreamLease};
use super::{outcome_for_message, StreamAdmission};

/// Wrap a stream function so every LLM stream runs inside an admission
/// slot: acquired before the provider call, released when the stream
/// settles, errors, aborts, or is dropped.
#[must_use]
pub fn admit_stream_fn(inner: StreamFn, key_of: KeyResolver, client: AdmissionClient) -> StreamFn {
    if !client.is_enabled() {
        return inner;
    }
    Arc::new(
        move |requested: AgentModel, context: LlmContext, options: StreamRequestOptions| {
            let inner = Arc::clone(&inner);
            let client = client.clone();
            let key_of = Arc::clone(&key_of);
            Box::pin(async move {
                // The key resolves at call time, from the same slot the
                // inner stream reads its target from.
                let key = key_of();
                let lease = match client.acquire_stream(&key).await {
                    StreamAdmission::Admitted(id) => Some(StreamLease {
                        id,
                        client: client.clone(),
                        resolved: false,
                    }),
                    StreamAdmission::Unadmitted => None,
                    // The provider's breaker is open (M4): fail fast with
                    // a rate-limit-shaped refusal the ladder's quota lane
                    // settles at once, so the failover chain re-routes
                    // instead of hammering the provider.
                    StreamAdmission::Refused => {
                        let (handle, consumer) = pa_agent::stream::event_stream();
                        handle.end(Some(breaker_refusal_message(&key)));
                        return Ok(Box::new(consumer) as Box<dyn ModelStream>);
                    }
                };
                let stream = inner(requested, context, options).await?;
                Ok(Box::new(AdmittedStream {
                    inner: stream,
                    lease,
                    client,
                }) as Box<dyn ModelStream>)
            })
        },
    )
}

/// A [`ModelStream`] that owns one admission slot for exactly its own
/// lifetime (the narrow bracket: never the worker's).
struct AdmittedStream {
    inner: Box<dyn ModelStream>,
    lease: Option<StreamLease>,
    client: AdmissionClient,
}

impl ModelStream for AdmittedStream {
    fn next_event(
        &mut self,
    ) -> pa_agent::BoxFut<'_, Option<pa_agent::stream::AssistantMessageEvent>> {
        self.inner.next_event()
    }

    fn result(&mut self) -> pa_agent::BoxFut<'_, anyhow::Result<AssistantMessage>> {
        Box::pin(async move {
            let outcome = self.inner.result().await;
            match &outcome {
                // A provider 429 halves the limit; any other settle adds a
                // slot.
                Ok(message) => {
                    if let Some(lease) = self.lease.take() {
                        lease.settle(outcome_for_message(message)).await;
                    }
                }
                // A stream error is not a rate-limit signal: the slot
                // returns without moving the limit.
                Err(_) => {
                    if let Some(lease) = self.lease.take() {
                        lease.release().await;
                    }
                }
            }
            outcome
        })
    }

    fn close(&mut self) {
        self.inner.close();
        if let Some(lease) = self.lease.take() {
            self.client.release_detached(lease.id());
        }
    }
}
