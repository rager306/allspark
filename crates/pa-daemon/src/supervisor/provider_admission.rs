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

use pa_core::session_engine::provider_retry::RateLimitReason;

use crate::agent_engine::provider_admission::{
    ProviderAdmission, ProviderKey, StreamAdmission, StreamOutcome, ADMISSION_COMMAND,
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
            rest,
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
        // The read-only snapshot (M5) needs no worker identity: counters
        // only, never prompt/session content, on the user-owned socket.
        // Every mutating action still authenticates by the worker token.
        let snapshot_only = action == "snapshot";
        if worker_token.is_empty() && !snapshot_only {
            return response_failure(
                Some(command_id),
                type_name,
                "Worker authentication failed",
                None,
            );
        }
        let registry = self.provider_admission();
        if snapshot_only {
            let rows: Vec<serde_json::Value> = registry
                .provider_rows()
                .into_iter()
                .map(|row| {
                    json!({
                        "provider": row.provider,
                        "model": row.model,
                        "currentLimit": row.limit,
                        "inFlight": row.in_flight,
                        "ceiling": row.cap,
                        "streak429": row.consecutive_429,
                        "queueDepth": row.queued,
                        "breakerState": row.breaker.state.as_wire(),
                        "budgetFailures": row.breaker.budget_failures,
                        "budgetSamples": row.breaker.budget_samples,
                    })
                })
                .collect();
            return response_success(
                Some(command_id),
                type_name,
                Some(json!({
                    "providers": rows,
                    "queuedTotal": registry.queued_total(),
                })),
            );
        }
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
                    match registry.acquire_stream(&key, worker_token).await {
                        StreamAdmission::Admitted(lease) => Some((lease, Duration::ZERO)),
                        // The provider's breaker is OPEN (M4): the wire
                        // answers a plain refusal the worker reads as
                        // fail-fast-to-failover, never as fail-open.
                        StreamAdmission::Refused => {
                            self.log_line(&format!(
                                "{ADMISSION_COMMAND}: acquire_stream {} by {worker_token} refused (circuit open)",
                                key.selector()
                            ));
                            return response_success(
                                Some(command_id),
                                type_name,
                                Some(json!({ "admitted": false, "refused": true })),
                            );
                        }
                        StreamAdmission::Unadmitted => None,
                    }
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
                    "{ADMISSION_COMMAND}: {action} {} by {worker_token} limit={} in_flight={} queued={} cap={} breaker={} stagger_ms={}",
                    key.selector(),
                    snapshot.limit,
                    snapshot.in_flight,
                    snapshot.queued,
                    snapshot.cap,
                    snapshot.breaker.state.as_wire(),
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
                let settled = match outcome.as_deref().and_then(StreamOutcome::from_wire) {
                    // The typed reason rides the wire beside the outcome
                    // (the worker's raw-JSON settle sends it as a plain
                    // field, which the command's catch-all holds).
                    Some(StreamOutcome::RateLimited(default_reason)) => {
                        let reason = rest
                            .get("reason")
                            .and_then(serde_json::Value::as_str)
                            .and_then(RateLimitReason::from_wire)
                            .unwrap_or(default_reason);
                        StreamOutcome::RateLimited(reason)
                    }
                    other => other.unwrap_or(StreamOutcome::Settled),
                };
                if action == "settle_stream" {
                    registry.settle(*lease_id, settled);
                } else {
                    registry.release(*lease_id);
                }
                let reason_wire = match settled {
                    StreamOutcome::RateLimited(reason) => reason.as_wire(),
                    StreamOutcome::Settled => "settled",
                };
                self.log_line(&format!(
                    "{ADMISSION_COMMAND}: {action} lease {lease_id} by {worker_token} ({reason_wire}) queued={}",
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::supervisor::SupervisorOptions;
    use pa_types::daemon::DaemonCommand;
    use serde_json::Map;

    /// The supervisor plus the temp dir that keeps its paths alive for
    /// the test's duration (bind both; the registry reads settings from
    /// `agent_dir` on first use).
    fn supervisor() -> (Arc<Supervisor>, tempfile::TempDir) {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let agent_dir = dir.path().join("agent");
        std::fs::create_dir_all(&agent_dir).expect("agent dir");
        let supervisor = Arc::new(
            Supervisor::new(SupervisorOptions {
                socket_path: dir.path().join("daemon.sock"),
                agent_dir,
            })
            .expect("supervisor"),
        );
        (supervisor, dir)
    }

    fn command(action: &str, fields: &[(&str, serde_json::Value)]) -> DaemonCommand {
        let mut rest = Map::new();
        for (key, value) in fields {
            rest.insert((*key).to_string(), value.clone());
        }
        DaemonCommand::ProviderAdmission {
            id: None,
            worker_token: "worker-a".to_string(),
            action: action.to_string(),
            provider: Some("zai".to_string()),
            model: Some("glm-5.3".to_string()),
            spawn_id: None,
            lease_id: None,
            outcome: None,
            rest,
        }
    }

    async fn acquire(supervisor: &Arc<Supervisor>) -> u64 {
        let response = supervisor
            .handle_provider_admission("c", "provider_admission", &command("acquire_stream", &[]))
            .await;
        assert!(response.success, "acquire must succeed: {response:?}");
        response.data.expect("acquire data")["leaseId"]
            .as_u64()
            .expect("lease id")
    }

    /// M3 on the wire: a settle's typed reason rides the command's plain
    /// fields, and an account-local reason drops the AIMD limit to the
    /// floor (6 -> 1) instead of halving it.
    #[tokio::test]
    async fn a_settle_reason_rides_the_wire_and_drops_the_limit_aggressively() {
        let (supervisor, _dir) = supervisor();
        let lease = acquire(&supervisor).await;
        let mut settle = command("settle_stream", &[]);
        if let DaemonCommand::ProviderAdmission {
            lease_id, outcome, ..
        } = &mut settle
        {
            *lease_id = Some(lease);
            *outcome = Some("rate_limited".to_string());
        }
        supervisor
            .handle_provider_admission("c", "provider_admission", &settle)
            .await;
        let key = ProviderKey::new("zai", "glm-5.3");
        let snapshot = supervisor.provider_admission().snapshot(&key);
        assert_eq!(snapshot.limit, 3, "a plain 429 halves: 6 -> 3");

        let lease = acquire(&supervisor).await;
        let mut quota_settle = command(
            "settle_stream",
            &[("reason", serde_json::json!("quota_exhausted"))],
        );
        if let DaemonCommand::ProviderAdmission {
            lease_id, outcome, ..
        } = &mut quota_settle
        {
            *lease_id = Some(lease);
            *outcome = Some("rate_limited".to_string());
        }
        supervisor
            .handle_provider_admission("c", "provider_admission", &quota_settle)
            .await;
        let snapshot = supervisor.provider_admission().snapshot(&key);
        assert_eq!(snapshot.limit, 1, "an account-local reason hits the floor");
        assert_eq!(snapshot.breaker.budget_failures, 2);
    }

    /// M4 on the wire: a spent retry budget makes the supervisor answer
    /// acquires with `refused: true` — the fail-fast the worker reads
    /// as fail-over, never as fail-open.
    #[tokio::test]
    async fn a_spent_budget_answers_acquires_with_a_wire_refusal() {
        let (supervisor, _dir) = supervisor();
        for _ in 0..crate::agent_engine::provider_admission::RETRY_BUDGET_MIN_SAMPLES {
            let lease = acquire(&supervisor).await;
            let mut settle = command("settle_stream", &[]);
            if let DaemonCommand::ProviderAdmission {
                lease_id, outcome, ..
            } = &mut settle
            {
                *lease_id = Some(lease);
                *outcome = Some("rate_limited".to_string());
            }
            supervisor
                .handle_provider_admission("c", "provider_admission", &settle)
                .await;
        }
        let response = supervisor
            .handle_provider_admission("c", "provider_admission", &command("acquire_stream", &[]))
            .await;
        assert!(response.success);
        let data = response.data.expect("acquire data");
        assert_eq!(data["admitted"], serde_json::json!(false));
        assert_eq!(data["refused"], serde_json::json!(true));
    }

    /// M5 on the wire: the read-only `snapshot` action answers every
    /// per-provider observability field and needs no worker token (the
    /// daemon state's compact section; counters only, never content).
    #[tokio::test]
    async fn the_snapshot_action_serves_every_field_without_a_token() {
        let (supervisor, _dir) = supervisor();
        let lease = acquire(&supervisor).await;
        let mut settle = command("settle_stream", &[]);
        if let DaemonCommand::ProviderAdmission {
            lease_id, outcome, ..
        } = &mut settle
        {
            *lease_id = Some(lease);
            *outcome = Some("rate_limited".to_string());
        }
        supervisor
            .handle_provider_admission("c", "provider_admission", &settle)
            .await;

        let mut snapshot = command("snapshot", &[]);
        if let DaemonCommand::ProviderAdmission { worker_token, .. } = &mut snapshot {
            *worker_token = String::new();
        }
        let response = supervisor
            .handle_provider_admission("c", "provider_admission", &snapshot)
            .await;
        assert!(
            response.success,
            "the snapshot needs no token: {response:?}"
        );
        let data = response.data.expect("snapshot data");
        assert_eq!(data["queuedTotal"], serde_json::json!(0));
        let rows = data["providers"].as_array().expect("rows");
        let row = &rows[0];
        assert_eq!(row["provider"], serde_json::json!("zai"));
        assert_eq!(row["model"], serde_json::json!("glm-5.3"));
        assert_eq!(row["currentLimit"], serde_json::json!(3));
        assert_eq!(row["inFlight"], serde_json::json!(0));
        assert_eq!(row["ceiling"], serde_json::json!(6));
        assert_eq!(row["streak429"], serde_json::json!(1));
        assert_eq!(row["queueDepth"], serde_json::json!(0));
        assert_eq!(row["breakerState"], serde_json::json!("closed"));
        assert_eq!(row["budgetFailures"], serde_json::json!(1));
        assert_eq!(row["budgetSamples"], serde_json::json!(1));
    }
}
