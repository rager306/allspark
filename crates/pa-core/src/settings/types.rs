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
