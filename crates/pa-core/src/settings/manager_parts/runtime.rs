//! Runtime policy: retries, compaction, transport, concurrency, tracing.

use anyhow::Result;

use crate::settings::manager::{
    IdleEviction, SettingsManager, DEFAULT_IDLE_EVICTION_MINUTES,
    DEFAULT_PROVIDER_CONCURRENCY_LIMIT,
};
use crate::settings::types::TransportSetting;

impl SettingsManager {
    /// The auto-retry toggle (`retry.enabled`).
    ///
    /// # Errors
    ///
    /// Returns an error when the global settings file cannot be written.
    pub fn set_retry_enabled(&mut self, enabled: bool) -> Result<()> {
        self.global
            .retry
            .get_or_insert_with(Default::default)
            .enabled = Some(enabled);
        self.save_global(&["retry.enabled"])
    }

    /// The auto-compaction toggle (`compaction.enabled`): a daemon restart re-seeds
    /// the connection state's flag from the persisted setting.
    ///
    /// # Errors
    ///
    /// Returns an error when the global settings file cannot be written.
    pub fn set_compaction_enabled(&mut self, enabled: bool) -> Result<()> {
        self.global
            .compaction
            .get_or_insert_with(Default::default)
            .enabled = Some(enabled);
        self.save_global(&["compaction.enabled"])
    }

    /// # Errors
    ///
    /// Returns an error when the global settings file cannot be written.
    pub fn set_transport(&mut self, transport: TransportSetting) -> Result<()> {
        self.global.transport = Some(transport);
        self.save_global(&["transport"])
    }

    /// # Errors
    ///
    /// Returns an error when the global settings file cannot be written.
    pub fn set_rlm_max_depth(&mut self, depth: u64) -> Result<()> {
        self.global.rlm_max_depth = Some(depth);
        self.save_global(&["rlmMaxDepth"])
    }

    /// The auto-compaction toggle, on until the user opts out.
    #[must_use]
    pub fn get_compaction_enabled(&self) -> bool {
        self.merged
            .compaction
            .as_ref()
            .and_then(|compaction| compaction.enabled)
            .unwrap_or(true)
    }

    /// `agentTraces.enabled`: unset means OFF — trace sharing is opt-in, exactly the TS default.
    #[must_use]
    pub fn get_agent_traces_enabled(&self) -> bool {
        self.merged
            .agent_traces
            .as_ref()
            .and_then(|traces| traces.enabled)
            .unwrap_or(false)
    }

    /// Whether a trace-sharing choice was ever written: the `agentTraces.enabled` key
    /// present in the merged settings. Only a fresh home (no choice written) is asked, once.
    #[must_use]
    pub fn agent_traces_choice_written(&self) -> bool {
        self.merged
            .agent_traces
            .as_ref()
            .and_then(|traces| traces.enabled)
            .is_some()
    }

    /// # Errors
    ///
    /// Returns an error when the global settings file cannot be written.
    pub fn set_agent_traces_enabled(&mut self, enabled: bool) -> Result<()> {
        let traces = self
            .global
            .agent_traces
            .get_or_insert_with(Default::default);
        traces.enabled = Some(enabled);
        self.save_global(&["agentTraces.enabled"])
    }

    /// The shared provider retry policy from settings.
    #[must_use]
    pub fn get_provider_retry_policy(
        &self,
    ) -> crate::session_engine::provider_retry::ProviderRetryPolicy {
        crate::session_engine::provider_retry::ProviderRetryPolicy {
            enabled: self
                .merged
                .retry
                .as_ref()
                .and_then(|retry| retry.enabled)
                .unwrap_or(true),
            max_retries: self
                .merged
                .retry
                .as_ref()
                .and_then(|retry| retry.max_retries)
                .map_or(
                    crate::session_engine::provider_retry::DEFAULT_PROVIDER_RETRY_POLICY
                        .max_retries,
                    |retries| retries.min(u64::from(u32::MAX)) as u32,
                ),
            base_delay_ms: self
                .merged
                .retry
                .as_ref()
                .and_then(|retry| retry.base_delay_ms)
                .unwrap_or(
                    crate::session_engine::provider_retry::DEFAULT_PROVIDER_RETRY_POLICY
                        .base_delay_ms,
                ),
            max_retry_delay_ms: self
                .merged
                .retry
                .as_ref()
                .and_then(|retry| retry.provider.as_ref())
                .and_then(|provider| provider.max_retry_delay_ms)
                .unwrap_or(
                    crate::session_engine::provider_retry::DEFAULT_PROVIDER_RETRY_POLICY
                        .max_retry_delay_ms,
                ),
            max_delay_ms: crate::session_engine::provider_retry::UNBOUNDED_BACKOFF_MS,
            max_wait_ms: self
                .merged
                .retry
                .as_ref()
                .and_then(|retry| retry.provider.as_ref())
                .and_then(|provider| provider.wait_for_usage.as_ref())
                .and_then(|wait| wait.max_wait_ms)
                .unwrap_or(crate::session_engine::provider_retry::UNBOUNDED_BACKOFF_MS),
        }
    }

    /// The provider-failover policy from settings (`retry.failover`).
    #[must_use]
    pub fn get_provider_failover_policy(
        &self,
    ) -> crate::session_engine::provider_failover::ProviderFailoverPolicy {
        let defaults = crate::session_engine::provider_failover::DEFAULT_PROVIDER_FAILOVER_POLICY;
        let failover = self
            .merged
            .retry
            .as_ref()
            .and_then(|retry| retry.failover.as_ref());
        crate::session_engine::provider_failover::ProviderFailoverPolicy {
            enabled: failover
                .and_then(|failover| failover.enabled)
                .unwrap_or(defaults.enabled),
            max_retries: failover
                .and_then(|failover| failover.max_retries)
                .map_or(defaults.max_retries, |retries| {
                    retries.min(u64::from(u32::MAX)) as u32
                }),
            base_delay_ms: failover
                .and_then(|failover| failover.base_delay_ms)
                .unwrap_or(defaults.base_delay_ms),
            max_delay_ms: failover
                .and_then(|failover| failover.max_delay_ms)
                .unwrap_or(defaults.max_delay_ms),
        }
    }

    /// The quota-park policy from settings (`retry.provider.waitForUsage`): the
    /// park toggle, per-park ceiling (clamped to one week), and per-episode budget.
    #[must_use]
    pub fn get_provider_park_policy(
        &self,
    ) -> crate::session_engine::provider_park::ProviderParkPolicy {
        let defaults = crate::session_engine::provider_park::DEFAULT_PROVIDER_PARK_POLICY;
        let wait = self
            .merged
            .retry
            .as_ref()
            .and_then(|retry| retry.provider.as_ref())
            .and_then(|provider| provider.wait_for_usage.as_ref());
        crate::session_engine::provider_park::ProviderParkPolicy {
            pause_until_reset: wait
                .and_then(|wait| wait.pause_until_reset)
                .unwrap_or(defaults.pause_until_reset),
            max_pause_ms: wait
                .and_then(|wait| wait.max_pause_ms)
                .unwrap_or(defaults.max_pause_ms),
            max_parks: wait
                .and_then(|wait| wait.max_parks)
                .map_or(defaults.max_parks, |parks| {
                    parks.min(u64::from(u32::MAX)) as u32
                }),
        }
    }

    /// `[agents] maxConcurrentPerProvider`: the per-`(provider, model)`
    /// ceiling for concurrent provider streams and admitted spawns. The
    /// AIMD semaphore starts at [`DEFAULT_PROVIDER_CONCURRENCY_LIMIT`]
    /// (clamped to this ceiling) and recovers up to it after a 429
    /// backoff. Unset, non-numeric, or `0` reads the compiled default.
    #[must_use]
    pub fn get_max_concurrent_per_provider(&self) -> usize {
        self.merged
            .agents
            .as_ref()
            .and_then(|agents| agents.max_concurrent_per_provider)
            .and_then(|value| usize::try_from(value).ok())
            .filter(|value| *value > 0)
            .unwrap_or(DEFAULT_PROVIDER_CONCURRENCY_LIMIT)
    }

    #[must_use]
    pub fn get_rlm_max_depth(&self) -> Option<u64> {
        self.global.rlm_max_depth
    }

    /// `number | "off" | "none"` -> finite minutes or Off; malformed falls
    /// back to the default (90).
    #[must_use]
    pub fn get_idle_eviction(&self) -> IdleEviction {
        match &self.global.idle_eviction_minutes {
            Some(serde_json::Value::String(text)) if text == "off" || text == "none" => {
                IdleEviction::Off
            }
            Some(serde_json::Value::Number(number)) => {
                if let Some(minutes) = number.as_u64() {
                    if minutes > 0 {
                        return IdleEviction::Minutes(minutes);
                    }
                }
                IdleEviction::Minutes(DEFAULT_IDLE_EVICTION_MINUTES)
            }
            _ => IdleEviction::Minutes(DEFAULT_IDLE_EVICTION_MINUTES),
        }
    }

    #[must_use]
    pub fn get_transport(&self) -> TransportSetting {
        self.merged.transport.unwrap_or(TransportSetting::Auto)
    }

    /// `requestTiming`: unset means OFF — the per-request timing timeline
    /// is opt-in, exactly the TS default.
    #[must_use]
    pub fn get_request_timing(&self) -> bool {
        self.merged.request_timing.unwrap_or(false)
    }

    /// The per-cell watchdog budget for user `python_repl` cells, in
    /// milliseconds (`0` disables; the default is 60s). Daemonless
    /// sessions never arm the watchdog.
    #[must_use]
    pub fn get_python_cell_timeout_ms(&self) -> u64 {
        self.merged
            .python_cell_timeout_ms
            .unwrap_or(crate::settings::types::DEFAULT_PYTHON_CELL_TIMEOUT_MS)
    }
}
