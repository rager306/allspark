//! The supervisor's provider-admission plane (ADR `ratelimit-admission`,
//! M1+M2): the ONE cross-session AIMD registry every worker draws a slot
//! from, and the wire arm that serves the workers' requests.
//!
//! The registry lives here, not in a worker: one daemon hosts one process
//! per session, so a per-process semaphore bounds nothing (the ADR's
//! "глобальный семафор в pa-core per-session — бесполезен при N
//! процессах"). A worker's leases are attributed to its token, so a worker
//! that dies mid-stream is reaped by [`Supervisor::reap_provider_admission`]
//! instead of shrinking the pool forever.
//!
//! Fail-open is the contract: an unauthenticated ask, a malformed action,
//! or a lease that never existed answers a refusal the worker treats as
//! "proceed unadmitted" — the admission plane throttles, it never gates.

use std::sync::Arc;
use std::time::Duration;

use serde_json::json;

use crate::agent_engine::provider_admission::{
    ProviderAdmission, ProviderKey, StreamOutcome, ADMISSION_COMMAND,
};
use crate::protocol::{response_failure, response_success, DaemonResponse};
use crate::supervisor::Supervisor;
use pa_types::daemon::DaemonCommand;

impl Supervisor {
    /// The shared admission registry (built on first use from the live
    /// `[agents] maxConcurrentPerProvider` setting).
    pub(crate) fn provider_admission(&self) -> Arc<ProviderAdmission> {
        let mut slot = self
            .provider_admission
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(registry) = slot.as_ref() {
            return Arc::clone(registry);
        }
        let registry = ProviderAdmission::new(self.provider_concurrency_cap());
        *slot = Some(Arc::clone(&registry));
        registry
    }

    /// The live ceiling: the settings value, read once at registry build
    /// (the AIMD limit recovers toward it, never past it).
    fn provider_concurrency_cap(&self) -> usize {
        let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("/"));
        pa_core::settings::SettingsManager::create(cwd.as_path(), &self.options.agent_dir)
            .get_max_concurrent_per_provider()
    }

    /// Release every slot a worker still holds: the worker-exit reap, so a
    /// session that dies mid-stream never leaks a permit.
    pub(crate) fn reap_provider_admission(&self, worker_token: &str) {
        let registry = self
            .provider_admission
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        let Some(registry) = registry else {
            return;
        };
        let released = registry.release_holder(worker_token);
        if released > 0 {
            self.log_line(&format!(
                "provider admission: released {released} lease(s) of a stopped worker"
            ));
        }
    }

    /// Serve one `provider_admission` request: authenticate the worker
    /// token, then run the action against the shared registry.
    pub(crate) async fn handle_provider_admission(
        self: &Arc<Self>,
        command_id: &str,
        type_name: &str,
        command: &DaemonCommand,
    ) -> DaemonResponse {
        let DaemonCommand::ProviderAdmission {
            worker_token,
            action,
            provider,
            model,
            spawn_id,
            lease_id,
            outcome,
            ..
        } = command
        else {
            return response_failure(
                Some(command_id),
                type_name,
                "invalid admission command",
                None,
            );
        };
        if worker_token.is_empty() {
            return response_failure(
                Some(command_id),
                type_name,
                "Worker authentication failed",
                None,
            );
        }
        let registry = self.provider_admission();
        match action.as_str() {
            "acquire_stream" | "spawn_gate" => {
                let Some(provider) = provider.clone() else {
                    return response_failure(
                        Some(command_id),
                        type_name,
                        "provider admission requires a provider",
                        None,
                    );
                };
                let key = ProviderKey::new(provider, model.clone().unwrap_or_default());
                let admitted = if action == "acquire_stream" {
                    registry
                        .acquire_stream(&key, worker_token)
                        .await
                        .map(|lease| (lease, Duration::ZERO))
                } else {
                    let spawn_id = spawn_id.clone().unwrap_or_else(|| worker_token.clone());
                    registry.spawn_gate(&key, &spawn_id).await
                };
                let Some((lease, stagger)) = admitted else {
                    // The wait budget expired: the worker proceeds
                    // unadmitted (fail-open), which the wire says plainly.
                    return response_success(
                        Some(command_id),
                        type_name,
                        Some(json!({ "admitted": false })),
                    );
                };
                let snapshot = registry.snapshot(&key);
                self.log_line(&format!(
                    "{ADMISSION_COMMAND}: {action} {} by {worker_token} limit={} in_flight={} queued={} cap={} stagger_ms={}",
                    key.selector(),
                    snapshot.limit,
                    snapshot.in_flight,
                    snapshot.queued,
                    snapshot.cap,
                    stagger.as_millis()
                ));
                response_success(
                    Some(command_id),
                    type_name,
                    Some(json!({
                        "admitted": true,
                        "leaseId": lease,
                        "staggerMs": u64::try_from(stagger.as_millis()).unwrap_or(u64::MAX),
                        "limit": snapshot.limit,
                        "inFlight": snapshot.in_flight,
                        "queued": snapshot.queued,
                        "cap": snapshot.cap,
                    })),
                )
            }
            "settle_stream" | "release_stream" => {
                let Some(lease_id) = lease_id else {
                    return response_failure(
                        Some(command_id),
                        type_name,
                        "provider admission requires a leaseId",
                        None,
                    );
                };
                let settled = outcome
                    .as_deref()
                    .and_then(StreamOutcome::from_wire)
                    .unwrap_or(StreamOutcome::Settled);
                if action == "settle_stream" {
                    registry.settle(*lease_id, settled);
                } else {
                    registry.release(*lease_id);
                }
                self.log_line(&format!(
                    "{ADMISSION_COMMAND}: {action} lease {lease_id} by {worker_token} ({}) queued={}",
                    settled.as_wire(),
                    registry.queued_total()
                ));
                response_success(
                    Some(command_id),
                    type_name,
                    Some(json!({ "released": true })),
                )
            }
            other => response_failure(
                Some(command_id),
                type_name,
                &format!("unknown provider admission action: {other}"),
                None,
            ),
        }
    }
}
