//! Settings types. All fields optional; JSON values of the wrong type load
//! as `None` (the TS access-time typechecks), never a load error.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MermaidRenderingMode {
    Off,
    Final,
    Streaming,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum UpdateChannel {
    Stable,
    Nightly,
}

/// Thinking levels in wire form ("off" | "minimal" | "low" | "medium" | "high" | "xhigh" | "max").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ThinkingLevelSetting {
    Off,
    Minimal,
    Low,
    Medium,
    High,
    Xhigh,
    Max,
}

impl ThinkingLevelSetting {
    /// The reverse of [`ThinkingLevelSetting::model_level`]: a live switch (daemon
    /// `set_thinking_level`) persists a model-vocabulary level as the settings default.
    #[must_use]
    pub fn from_model_level(level: pa_types::ai::ModelThinkingLevel) -> ThinkingLevelSetting {
        match level {
            pa_types::ai::ModelThinkingLevel::Off => ThinkingLevelSetting::Off,
            pa_types::ai::ModelThinkingLevel::Minimal => ThinkingLevelSetting::Minimal,
            pa_types::ai::ModelThinkingLevel::Low => ThinkingLevelSetting::Low,
            pa_types::ai::ModelThinkingLevel::Medium => ThinkingLevelSetting::Medium,
            pa_types::ai::ModelThinkingLevel::High => ThinkingLevelSetting::High,
            pa_types::ai::ModelThinkingLevel::Xhigh => ThinkingLevelSetting::Xhigh,
            pa_types::ai::ModelThinkingLevel::Max => ThinkingLevelSetting::Max,
        }
    }

    /// The same level in the shared model vocabulary: the settings default
    /// feeds session thinking-level resolution.
    #[must_use]
    pub fn model_level(self) -> pa_types::ai::ModelThinkingLevel {
        match self {
            ThinkingLevelSetting::Off => pa_types::ai::ModelThinkingLevel::Off,
            ThinkingLevelSetting::Minimal => pa_types::ai::ModelThinkingLevel::Minimal,
            ThinkingLevelSetting::Low => pa_types::ai::ModelThinkingLevel::Low,
            ThinkingLevelSetting::Medium => pa_types::ai::ModelThinkingLevel::Medium,
            ThinkingLevelSetting::High => pa_types::ai::ModelThinkingLevel::High,
            ThinkingLevelSetting::Xhigh => pa_types::ai::ModelThinkingLevel::Xhigh,
            ThinkingLevelSetting::Max => pa_types::ai::ModelThinkingLevel::Max,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum QueueModeSetting {
    All,
    #[serde(alias = "oneAtATime")]
    OneAtATime,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TransportSetting {
    Auto,
    Sse,
    WebSocket,
    #[serde(rename = "websocket-cached")]
    WebSocketCached,
}

fn deserialize_threshold_percent<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<f64>, D::Error> {
    let value = serde_json::Value::deserialize(deserializer)?;
    Ok(value
        .as_f64()
        .filter(|percent| pa_types::compaction::is_valid_threshold_percent(*percent)))
}

/// `number | "unlimited"` autonomous limit.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum AutonomousLimitSetting {
    Number(f64),
    Unlimited(String),
}

/// FORK-ONLY: the fork's rotating debug-log overlay (`debugLog`).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DebugLogSettings {
    pub enabled: Option<bool>,
    pub level: Option<String>,
    pub max_files: Option<u32>,
    pub max_bytes: Option<u64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompactionSettings {
    pub enabled: Option<bool>,
    /// Percentage ceiling for automatic compaction; invalid values behave as unset.
    #[serde(default, deserialize_with = "deserialize_threshold_percent")]
    pub threshold_percent: Option<f64>,
    pub reserve_tokens: Option<u64>,
    pub keep_recent_tokens: Option<u64>,
    pub agent_callable: Option<bool>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BranchSummarySettings {
    pub reserve_tokens: Option<u64>,
    pub skip_prompt: Option<bool>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AutoRefineSettings {
    pub enabled: Option<bool>,
    pub turn_interval: Option<u64>,
    pub compact: Option<bool>,
    pub cooldown_ms: Option<u64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderWaitSettings {
    pub enabled: Option<bool>,
    pub base_delay_ms: Option<u64>,
    pub max_delay_ms: Option<u64>,
    pub max_attempts: Option<u64>,
    pub max_wait_ms: Option<u64>,
    /// Park sessions for provider-reported resets beyond the bounded wait (default true).
    pub pause_until_reset: Option<bool>,
    /// Maximum single park duration (default 24h, clamped to 7d).
    pub max_pause_ms: Option<u64>,
    /// Maximum parks per quota episode (default 8).
    pub max_parks: Option<u64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderRetrySettings {
    pub timeout_ms: Option<u64>,
    pub max_retry_delay_ms: Option<u64>,
    pub wait_for_usage: Option<ProviderWaitSettings>,
}

/// Provider-failover settings (`retry.failover`): when another configured
/// provider serves the same model, a provider that exhausts its quick
/// retries hands the turn to the next one instead of failing it.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RetryFailoverSettings {
    pub enabled: Option<bool>,
    /// Retries per provider before switching (default 5).
    pub max_retries: Option<u64>,
    /// First backoff delay, doubling each retry (default 1000ms).
    pub base_delay_ms: Option<u64>,
    /// Backoff ceiling per retry (default 30000ms).
    pub max_delay_ms: Option<u64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RetrySettings {
    pub enabled: Option<bool>,
    pub max_retries: Option<u64>,
    pub base_delay_ms: Option<u64>,
    pub failover: Option<RetryFailoverSettings>,
    pub provider: Option<ProviderRetrySettings>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalSettings {
    pub show_images: Option<bool>,
    pub clear_on_shrink: Option<bool>,
    pub show_terminal_progress: Option<bool>,
    pub fullscreen_mouse: Option<bool>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageSettings {
    pub auto_resize: Option<bool>,
    pub block_images: Option<bool>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThinkingBudgetsSettings {
    pub minimal: Option<u64>,
    pub low: Option<u64>,
    pub medium: Option<u64>,
    pub high: Option<u64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AutonomousSettings {
    pub max_continuations: Option<AutonomousLimitSetting>,
    pub max_turns: Option<AutonomousLimitSetting>,
    pub max_tokens: Option<AutonomousLimitSetting>,
    pub timeout_ms: Option<AutonomousLimitSetting>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MarkdownSettings {
    pub code_block_indent: Option<String>,
    pub mermaid: Option<MermaidRenderingMode>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BundledSkillsSettings {
    pub websearch: Option<bool>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WarningSettings {
    pub anthropic_extra_usage: Option<bool>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentTracesSettings {
    pub enabled: Option<bool>,
}

/// The native `lsp` tool's opt-in gate and tuning (fork-only). `enabled`
/// unset reads as `true` (the tool is on by default); `servers` reserves a
/// per-server override map and `idle_timeout_ms` a client-idle bound.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LspSettings {
    pub enabled: Option<bool>,
    /// Per-server overrides, keyed by server name (reserved; the built-in
    /// server table applies until a key is set).
    pub servers: Option<serde_json::Map<String, serde_json::Value>>,
    /// Idle timeout for a live language-server client, in milliseconds.
    pub idle_timeout_ms: Option<u64>,
}

/// The native `graph` tool's opt-in gate and store location (fork-only).
/// `enabled` unset reads as `true`; `db_path` defaults to
/// `<agentDir>/graph.db`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GraphSettings {
    pub enabled: Option<bool>,
    pub db_path: Option<String>,
    /// Build (and refresh) the graph in the background at session start.
    ///
    /// Unset reads as `auto`: the first `search`/`trace`/`architecture` query
    /// builds the graph on demand (see `GraphTool::ensure_indexed`) and a
    /// background task refreshes it while the session works. `"watch"` also
    /// keeps a filesystem watcher on the tree, so edits re-index incrementally
    /// instead of waiting for the next query. `"off"` disables both, leaving
    /// the explicit `index` action as the only build path.
    pub auto: Option<GraphAuto>,
}

/// When the graph builds and refreshes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GraphAuto {
    /// Build in the background at session start; refresh on query when stale.
    #[default]
    Auto,
    /// `auto`, plus a filesystem watcher that re-indexes changed files live.
    Watch,
    /// Never build on its own; the explicit `index` action is the only path.
    Off,
}

/// The native `edit` tool's opt-in gate and mode policy (fork-only).
/// `enabled` unset reads as `true` (the tool is on by default);
/// `defaultMode` picks the mode a call omits `mode` for; `allowModes`
/// restricts the requestable set (all five when unset).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EditHybridSettings {
    pub enabled: Option<bool>,
    pub default_mode: Option<String>,
    pub allow_modes: Option<Vec<String>>,
}

/// [FORK-ONLY] The model router's gate and mode (`[routing]`): whether the
/// per-turn chooser runs at all, and how far it may act. The router owns
/// no state of its own; it orders the provider ladder the failover driver
/// already walks, so `off` is exactly the pre-router behaviour.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RoutingSettings {
    /// `"off" | "dry-run" | "auto" | "on"`; unset or unrecognized reads as
    /// `off` (fail-closed on a typo, never fail-open).
    pub mode: Option<String>,
}

/// The resolved `routing.mode` gate.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ModelRoutingLevel {
    /// The router never runs: the pinned model serves, and failover keeps
    /// catalog order (the default).
    #[default]
    Off,
    /// The router decides and records the decision, and applies nothing:
    /// the level that produces the data justifying the rest.
    DryRun,
    /// The router may reorder the provider ladder, but only when a
    /// challenger beats the incumbent by the hysteresis margin.
    Auto,
    /// The router always orders by score (still behind every hard filter).
    On,
}

impl ModelRoutingLevel {
    /// Parse the settings string; unrecognized spellings read as `off`
    /// (fail-closed on a typo, never fail-open).
    #[must_use]
    pub fn parse(value: &str) -> ModelRoutingLevel {
        match value.trim().to_ascii_lowercase().as_str() {
            "dry-run" | "dry_run" | "dryrun" => ModelRoutingLevel::DryRun,
            "auto" => ModelRoutingLevel::Auto,
            "on" => ModelRoutingLevel::On,
            _ => ModelRoutingLevel::Off,
        }
    }

    /// Every level, in the menu's cycle order (`off` first, the default).
    #[must_use]
    pub fn all() -> [ModelRoutingLevel; 4] {
        [
            ModelRoutingLevel::Off,
            ModelRoutingLevel::DryRun,
            ModelRoutingLevel::Auto,
            ModelRoutingLevel::On,
        ]
    }

    /// The settings/wire spelling.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::DryRun => "dry-run",
            Self::Auto => "auto",
            Self::On => "on",
        }
    }

    /// Whether the router runs at all (every level but `off`).
    #[must_use]
    pub fn is_enabled(self) -> bool {
        !matches!(self, Self::Off)
    }

    /// Whether a decision may reorder the provider ladder (`auto` and `on`;
    /// `dry-run` records and applies nothing).
    #[must_use]
    pub fn applies(self) -> bool {
        matches!(self, Self::Auto | Self::On)
    }
}

/// `factory` (the agent factory's opt-in gate): `factory.enabled` is
/// unset/false by default, and the `/factory on` client command persists
/// it. Both the daemon's `factory_activity` lane advertisement and the
/// kernel's factory gate read the same shared settings key.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FactorySettings {
    pub enabled: Option<bool>,
}

/// FORK-ONLY: the typed-decision (judgment) layer's gate and tuning
/// (`[jev]`). Unlike `lsp`/`graph`/`edit` (default on), `enabled` defaults
/// OFF - the layer bills a paid third-party endpoint. `auto` keeps
/// judge-heavy surfaces off until a native (calibrated) judge resolves.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JevSettings {
    /// `"off" | "auto" | "on"`; unset or unrecognized reads as `off`.
    pub enabled: Option<String>,
    /// The pinned judge model (`provider/model-id`); the official SDK's
    /// `TYPESAFE_DEFAULT_MODEL` env applies when unset.
    pub model: Option<String>,
    /// Fallback candidates in attempt order.
    pub fallback_chain: Option<Vec<String>>,
    /// `"off" | "llm"`: prompted fallback. Phase 1 always treats it as
    /// `off` (better a disabled feature than uncalibrated thresholds).
    pub fallback: Option<String>,
    /// Per-attempt timeout in milliseconds (default 10000).
    pub timeout_ms: Option<u64>,
    pub cache: Option<JevCacheSettings>,
    /// FORK-ONLY Phase 2: the auto-thinking difficulty classifier
    /// (`off` when unset - advisory, and only ever `auto`).
    pub auto_thinking: Option<String>,
    /// FORK-ONLY Phase 2: the graph search re-rank (`off` when unset).
    pub graph_rerank: Option<String>,
    /// FORK-ONLY Phase 2: the tool-selection arbiter (`auto` when unset; the
    /// deterministic table always applies, the judgment layer needs a native
    /// judge).
    pub tool_arbiter: Option<String>,
}

/// `[agents]`: the subagent admission surface. `maxConcurrentPerProvider`
/// is the AIMD ceiling per resolved `(provider, model)` pair — the start
/// limit is the compiled default, and a successful stream raises the
/// limit up to this cap. Unset (or `0`) reads the compiled default.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentsSettings {
    pub max_concurrent_per_provider: Option<u64>,
}

/// `[jev.cache]`: the judgment answer cache.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JevCacheSettings {
    /// Unset reads as `true` (repeated questions cost nothing).
    pub enabled: Option<bool>,
}

/// The resolved `jev.enabled` gate.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum JevEnabledSetting {
    /// The layer is off: no tool, no consumers, no spend (the default).
    #[default]
    Off,
    /// Judge-heavy surfaces run only while a native judge resolves.
    Auto,
    /// The layer runs whenever a judge chain resolves.
    On,
}

impl JevEnabledSetting {
    /// Parse the settings string; `None` for unrecognized spellings.
    #[must_use]
    pub fn parse(value: &str) -> Option<JevEnabledSetting> {
        match value.trim().to_ascii_lowercase().as_str() {
            "off" => Some(JevEnabledSetting::Off),
            "auto" => Some(JevEnabledSetting::Auto),
            "on" => Some(JevEnabledSetting::On),
            _ => None,
        }
    }
}

/// The resolved `jev.autoThinking` gate. Both spellings that mean "not off"
/// are the same behaviour (`auto`): a consumer is never forced on by a
/// settings typo, and `on` reads as `auto` so no value can bypass the
/// native-judge requirement. The default is `auto` since the live
/// promotion runs (see CHANGELOG-FORK fork.007).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum JevAutoThinkingSetting {
    /// The consumer never runs.
    Off,
    /// Run only behind a native (calibrated) judge (the default since
    /// the live promotion run: +50.0 points on the frozen test split).
    #[default]
    Auto,
}

impl JevAutoThinkingSetting {
    /// Parse the settings string; unrecognized spellings read as `off`
    /// (fail-closed on a typo, never fail-open).
    #[must_use]
    pub fn parse(value: &str) -> JevAutoThinkingSetting {
        match value.trim().to_ascii_lowercase().as_str() {
            "auto" | "on" => JevAutoThinkingSetting::Auto,
            _ => JevAutoThinkingSetting::Off,
        }
    }
}

/// The resolved `jev.graphRerank` gate (same shape as
/// [`JevAutoThinkingSetting`]: `auto` or off).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum JevGraphRerankSetting {
    Off,
    /// Run only behind a native (calibrated) judge (the default since
    /// the live promotion run: +8.3 points on the frozen test split).
    #[default]
    Auto,
}

impl JevGraphRerankSetting {
    /// Parse the settings string; unrecognized spellings read as `off`.
    #[must_use]
    pub fn parse(value: &str) -> JevGraphRerankSetting {
        match value.trim().to_ascii_lowercase().as_str() {
            "auto" | "on" => JevGraphRerankSetting::Auto,
            _ => JevGraphRerankSetting::Off,
        }
    }
}

/// The resolved `jev.toolArbiter` gate (same shape as
/// [`JevGraphRerankSetting`]: `auto` or off). The arbiter is advisory by
/// construction - it appends a note and never blocks or rewrites a call - so
/// `auto` (the default, matching the other Phase-2 consumers) only arms the
/// judgment layer behind a native judge.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum JevToolArbiterSetting {
    /// The arbiter emits nothing at all (the pre-jev bash result).
    Off,
    /// The deterministic table always applies; the judgment layer runs only
    /// behind a native (calibrated) judge (the default).
    #[default]
    Auto,
}

impl JevToolArbiterSetting {
    /// Parse the settings string; unrecognized spellings read as `off`
    /// (fail-closed on a typo, never fail-open).
    #[must_use]
    pub fn parse(value: &str) -> JevToolArbiterSetting {
        match value.trim().to_ascii_lowercase().as_str() {
            "auto" | "on" => JevToolArbiterSetting::Auto,
            _ => JevToolArbiterSetting::Off,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TelemetrySettings {
    pub enabled: Option<bool>,
    pub notice_shown: Option<bool>,
    /// Local JSONL mirror at `<agentDir>/telemetry.jsonl` (default on:
    /// user-observable transparency).
    pub local_mirror: Option<bool>,
}

/// User-declared MCP server (settings `mcpServers` entry).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum McpServerConfig {
    Http {
        url: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        headers: Option<serde_json::Map<String, serde_json::Value>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        bearer_token_env_var: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        oauth: Option<bool>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        enabled: Option<bool>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        enabled_tools: Option<Vec<String>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        disabled_tools: Option<Vec<String>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        startup_timeout_ms: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        call_timeout_ms: Option<u64>,
    },
    Stdio {
        command: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        args: Option<Vec<String>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cwd: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        env: Option<serde_json::Map<String, serde_json::Value>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        enabled: Option<bool>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        enabled_tools: Option<Vec<String>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        disabled_tools: Option<Vec<String>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        startup_timeout_ms: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        call_timeout_ms: Option<u64>,
    },
}

/// Phase-7 tool-surface profile (fork-only): the active context, the modes
/// layered on top of it, and a project-local delta. Resolved into a
/// [`crate::tool_profile::ToolsetLayer`] pipeline at session creation
/// (`config -> context -> base modes -> dynamic modes -> single-project ->
/// project`). Unset reads as the full default surface.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ToolProfileSettings {
    /// Context preset name (`coding`, `query`, `minimal`, `debug`).
    pub context: Option<String>,
    /// Mode names applied after the context (any of the nine Serena modes).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub modes: Vec<String>,
    /// Project-local tool exclusions (the last layer).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub excluded_tools: Vec<String>,
    /// Project-local tool inclusions (an inclusion beats any exclusion).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub included_tools: Vec<String>,
}

/// The settings document: every known field optional; unknown keys preserved.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    pub onboarding_shown: Option<bool>,
    pub onboarding_completed: Option<bool>,
    pub default_provider: Option<String>,
    pub default_model: Option<String>,
    pub subagent_default_model: Option<String>,
    pub update_channel: Option<UpdateChannel>,
    pub recent_models: Option<Vec<String>>,
    pub auxiliary_model: Option<String>,
    pub default_thinking_level: Option<ThinkingLevelSetting>,
    pub default_service_tier: Option<String>,
    pub rlm_max_depth: Option<u64>,
    /// `number | "off" | "none"`; raw JSON because the TS getter validates at access time.
    pub idle_eviction_minutes: Option<serde_json::Value>,
    /// Session archiving age rule: sessions untouched for this many days are
    /// moved to the daemon's archive directory. `number | "off" | "none"`; raw
    /// JSON validated at access time (same grammar as `idleEvictionMinutes`).
    pub session_archive_max_age_days: Option<serde_json::Value>,
    /// Session archiving count rule: keep at most this many unarchived sessions (newest by mtime
    /// win); `0 | "off" | "none"` disables. Raw JSON validated at access time.
    pub session_archive_max_sessions: Option<serde_json::Value>,
    pub transport: Option<TransportSetting>,
    pub steering_mode: Option<QueueModeSetting>,
    pub follow_up_mode: Option<QueueModeSetting>,
    pub theme: Option<String>,
    pub compaction: Option<CompactionSettings>,
    /// FORK-ONLY: the rotating debug-log overlay (see `pa_daemon::debug_log`).
    pub debug_log: Option<DebugLogSettings>,
    pub auto_refine: Option<AutoRefineSettings>,
    /// Master switch for harness creation and context loading; unset preserves enabled behavior.
    pub harness_refinements: Option<bool>,
    pub agent_traces: Option<AgentTracesSettings>,
    /// Native `lsp` tool settings (fork-only; `enabled` defaults on).
    pub lsp: Option<LspSettings>,
    /// Native `graph` code-graph tool settings (fork-only; `enabled` defaults on).
    pub graph: Option<GraphSettings>,
    /// Native `edit` tool settings (fork-only; `enabled` defaults on).
    pub edit_hybrid: Option<EditHybridSettings>,
    /// Tool-surface profile (fork-only; Phase 7 config isolation).
    pub tool_profile: Option<ToolProfileSettings>,
    /// Typed-decision (judgment) layer settings (fork-only; `enabled`
    /// defaults off - see [`JevSettings`]).
    pub jev: Option<JevSettings>,
    pub factory: Option<FactorySettings>,
    /// [FORK-ONLY] The model router's gate and mode (see
    /// [`RoutingSettings`]); `mode` unset reads as `off`.
    pub routing: Option<RoutingSettings>,
    /// `[agents]`: subagent admission (the per-provider AIMD ceiling).
    pub agents: Option<AgentsSettings>,
    pub telemetry: Option<TelemetrySettings>,
    pub branch_summary: Option<BranchSummarySettings>,
    pub retry: Option<RetrySettings>,
    pub provider_backup_model: Option<String>,
    /// Model ("provider/model-id" or a bare model id) that serves turns attaching images when the
    /// session model does not accept image input. Default: none - image turns on a text-only model
    /// fail with a configuration hint instead of silently dropping the images.
    pub image_model: Option<String>,
    pub autonomous: Option<AutonomousSettings>,
    /// Custom shell that runs every kernel `bash()` and user-bash command as
    /// `<shell> -c <command>`: an absolute path to an existing executable,
    /// validated once at session start (a bad value fails session creation
    /// naming this setting). Project `.prime/agent/settings.json` overrides
    /// the global value (TS parity), so opening an untrusted repo lets it
    /// choose the binary that wraps every agent shell command - review
    /// project settings before running the agent there.
    pub shell_path: Option<String>,
    pub quiet_startup: Option<bool>,
    pub shell_command_prefix: Option<String>,
    pub npm_command: Option<Vec<String>>,
    pub mcp_servers: Option<serde_json::Map<String, serde_json::Value>>,
    /// Extra local MCP service-catalog files (~-relative allowed), merged after the
    /// compiled built-ins; first source wins per id, and no bundled id can be shadowed.
    pub mcp_catalog_sources: Option<Vec<String>>,
    pub packages: Option<Vec<serde_json::Value>>,
    pub skills: Option<Vec<String>>,
    pub prompts: Option<Vec<String>>,
    pub themes: Option<Vec<String>>,
    pub enable_skill_commands: Option<bool>,
    pub bundled_skills: Option<BundledSkillsSettings>,
    pub enable_builtin_skills: Option<bool>,
    pub terminal: Option<TerminalSettings>,
    pub images: Option<ImageSettings>,
    pub enabled_models: Option<Vec<String>>,
    /// Rust-only daemon-level model allowlist: a model outside fails loudly instead
    /// of resolving, with no fallback. `None` is unrestricted.
    pub allowed_models: Option<Vec<String>>,
    pub tree_filter_mode: Option<String>,
    /// `chatDetail`: the conversation-detail level Ctrl+O cycles and persists
    /// ("overview"/"details"/"all"); `None` reads as the `overview` startup level.
    pub chat_detail: Option<String>,
    pub thinking_budgets: Option<ThinkingBudgetsSettings>,
    pub editor_padding_x: Option<u64>,
    pub autocomplete_max_visible: Option<u64>,
    pub show_hardware_cursor: Option<bool>,
    pub markdown: Option<MarkdownSettings>,
    pub warnings: Option<WarningSettings>,
    pub session_dir: Option<String>,
    /// Log per-request provider timing phases to the diagnostic log
    /// (unset means OFF, exactly the TS default).
    pub request_timing: Option<bool>,
    /// Unknown keys survive load/save round-trips (forward compatibility).
    #[serde(flatten, skip_serializing_if = "serde_json::Map::is_empty")]
    pub extra: serde_json::Map<String, serde_json::Value>,
}
