//! Typed builders for the catalog's lifecycle events (onboarding, startup,
//! installation stages, admission transitions) and the run vocabulary
//! enums. Every field is typed (numbers are `u64`, enums are Rust
//! enums, nullable durations are `Option<u64>`), so a firing
//! site cannot emit a stringly-typed or out-of-vocabulary value. Optional
//! fields are `Option` and simply omit the property when `None`.
//!
//! The builders only carry event-specific properties; the client merges the
//! base properties (version, platform, execution mode) under them, and the
//! worker normalizes every batch through [`crate::catalog::sanitize`]
//! before any sink sees it.

use serde_json::Value;

use crate::properties::Properties;
use crate::TelemetryClient;

/// A fresh prompt or a loop continuation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunTrigger {
    Prompt,
    Continuation,
    Unknown,
}

impl RunTrigger {
    /// The wire vocabulary value.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Prompt => "prompt",
            Self::Continuation => "continuation",
            Self::Unknown => "unknown",
        }
    }
}

/// The fixed tool-category vocabulary: `from_tool_name` maps a concrete tool
/// name onto it, `unknown` for anything unrecognized.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ToolCategory {
    Read,
    Write,
    Edit,
    Bash,
    Grep,
    Find,
    Ls,
    Ipython,
    Mcp,
    /// FORK-ONLY: the code-graph tool. Without its own arm every graph call
    /// folded into `custom` beside lsp and jev, so the run summary could not
    /// say which of the three cost the time.
    Graph,
    /// FORK-ONLY: the language-server tool.
    Lsp,
    /// FORK-ONLY: the typed-judgment tool.
    Jev,
    Custom,
    Unknown,
}

impl ToolCategory {
    /// Map a concrete tool name onto the fixed category vocabulary.
    #[must_use]
    pub fn from_tool_name(tool_name: &str) -> Self {
        let normalized = tool_name.to_ascii_lowercase();
        let core = normalized.split([':', '_']).next().unwrap_or_default();
        match core {
            "read" => Self::Read,
            "write" => Self::Write,
            "edit" => Self::Edit,
            "bash" => Self::Bash,
            "grep" => Self::Grep,
            "find" => Self::Find,
            "ls" => Self::Ls,
            "python" | "ipython" => Self::Ipython,
            "mcp" => Self::Mcp,
            // FORK-ONLY tools: each gets its own category so the run summary
            // reports its own call/error/duration/max_duration rows instead of
            // pooling them into `custom_tool_*`.
            "graph" => Self::Graph,
            "lsp" => Self::Lsp,
            "jev" => Self::Jev,
            "" => Self::Unknown,
            _ => Self::Custom,
        }
    }

    /// The category's wire name.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Write => "write",
            Self::Edit => "edit",
            Self::Bash => "bash",
            Self::Grep => "grep",
            Self::Find => "find",
            Self::Ls => "ls",
            Self::Ipython => "ipython",
            Self::Mcp => "mcp",
            Self::Graph => "graph",
            Self::Lsp => "lsp",
            Self::Jev => "jev",
            Self::Custom => "custom",
            Self::Unknown => "unknown",
        }
    }
}

/// `onboarding stage`: one onboarding journey stage, the flow's real
/// stages only.
#[derive(Debug, Clone)]
pub struct OnboardingStage {
    pub onboarding_id: String,
    pub stage: &'static str,
    pub outcome: &'static str,
    pub duration_ms: Option<u64>,
    pub auth_category: Option<&'static str>,
    pub entry_reason: Option<&'static str>,
    pub timing_scope: Option<&'static str>,
}

impl OnboardingStage {
    pub fn track(&self, client: &TelemetryClient) {
        let mut properties = Properties::new();
        properties.set("onboarding_id", Value::String(self.onboarding_id.clone()));
        properties.set("stage", Value::from(self.stage));
        properties.set("outcome", Value::from(self.outcome));
        if let Some(duration) = self.duration_ms {
            properties.set("duration_ms", Value::from(duration));
        }
        if let Some(category) = self.auth_category {
            properties.set("auth_category", Value::from(category));
        }
        if let Some(reason) = self.entry_reason {
            properties.set("entry_reason", Value::from(reason));
        }
        if let Some(scope) = self.timing_scope {
            properties.set("timing_scope", Value::from(scope));
        }
        client.track("onboarding stage", properties);
    }
}

/// `agent startup stage`: one startup phase's timing.
#[derive(Debug, Clone)]
pub struct AgentStartupStage {
    pub stage: &'static str,
    pub outcome: &'static str,
    pub duration_ms: Option<u64>,
    pub startup_kind: Option<&'static str>,
    pub timing_scope: Option<&'static str>,
}

impl AgentStartupStage {
    pub fn track(&self, client: &TelemetryClient) {
        let Some(duration_ms) = self.duration_ms else {
            return;
        };
        let mut properties = Properties::new();
        properties.set("stage", Value::from(self.stage));
        properties.set("outcome", Value::from(self.outcome));
        properties.set("duration_ms", Value::from(duration_ms));
        if let Some(kind) = self.startup_kind {
            properties.set("startup_kind", Value::from(kind));
        }
        if let Some(scope) = self.timing_scope {
            properties.set("timing_scope", Value::from(scope));
        }
        client.track("agent startup stage", properties);
    }
}

/// `agent installation stage`: one installer/updater stage outcome.
#[derive(Debug, Clone)]
pub struct AgentInstallationStage {
    pub installation_attempt_id: String,
    pub installation_action: &'static str,
    pub installation_source: &'static str,
    pub stage: &'static str,
    pub outcome: &'static str,
    pub reason: Option<&'static str>,
    pub from_version: Option<String>,
    pub target_version: Option<String>,
    pub observed_version: Option<String>,
    pub duration_ms: Option<u64>,
    pub exit_code: Option<u64>,
    pub session_restore_total: Option<u64>,
    pub session_restore_failed: Option<u64>,
}

impl AgentInstallationStage {
    pub fn track(&self, client: &TelemetryClient) {
        let mut properties = Properties::new();
        properties.set(
            "installation_attempt_id",
            Value::String(self.installation_attempt_id.clone()),
        );
        properties.set("installation_action", Value::from(self.installation_action));
        properties.set("installation_source", Value::from(self.installation_source));
        properties.set("stage", Value::from(self.stage));
        properties.set("outcome", Value::from(self.outcome));
        if let Some(reason) = self.reason {
            properties.set("reason", Value::from(reason));
        }
        if let Some(version) = &self.from_version {
            properties.set("from_version", Value::String(version.clone()));
        }
        if let Some(version) = &self.target_version {
            properties.set("target_version", Value::String(version.clone()));
        }
        if let Some(version) = &self.observed_version {
            properties.set("observed_version", Value::String(version.clone()));
        }
        if let Some(duration) = self.duration_ms {
            properties.set("duration_ms", Value::from(duration));
        }
        if let Some(code) = self.exit_code {
            properties.set("exit_code", Value::from(code));
        }
        if let Some(total) = self.session_restore_total {
            properties.set("session_restore_total", Value::from(total));
        }
        if let Some(failed) = self.session_restore_failed {
            properties.set("session_restore_failed", Value::from(failed));
        }
        client.track("agent installation stage", properties);
    }
}

/// `update_homebrew_refusal`: an installer update rejected because Homebrew
/// owns the executable. Only the fixed formula/cask layout kind is recorded.
#[derive(Debug, Clone, Copy)]
pub struct UpdateHomebrewRefusal {
    /// `formula` or `cask`; never the executable path.
    pub kind: &'static str,
}

impl UpdateHomebrewRefusal {
    /// Record this refusal on the shared lifecycle telemetry client.
    pub fn track(&self, client: &TelemetryClient) {
        let mut properties = Properties::new();
        properties.set("kind", Value::from(self.kind));
        client.track("update_homebrew_refusal", properties);
    }
}

/// `admission_breaker_transition`: one provider-admission circuit
/// breaker state flip. The states come from
/// [`crate::catalog::BREAKER_STATES`] (the breaker's wire spelling);
/// `failure_count` is the retry-budget window's rate-limited settle
/// count at the decision moment.
#[derive(Debug, Clone)]
pub struct AdmissionBreakerTransition {
    pub provider: String,
    pub from_state: &'static str,
    pub to_state: &'static str,
    pub failure_count: u64,
}

impl AdmissionBreakerTransition {
    pub fn track(&self, client: &TelemetryClient) {
        let mut properties = Properties::new();
        properties.set("provider", Value::String(self.provider.clone()));
        properties.set("from_state", Value::from(self.from_state));
        properties.set("to_state", Value::from(self.to_state));
        properties.set("failure_count", Value::from(self.failure_count));
        client.track("admission_breaker_transition", properties);
    }
}

/// `admission_snapshot`: the admission registry's observability rollup —
/// counter aggregates only, one event per served snapshot ask.
#[derive(Debug, Clone, Copy)]
pub struct AdmissionSnapshot {
    pub providers_count: u64,
    pub total_in_flight: u64,
    pub total_queued: u64,
    pub any_open_breakers: bool,
}

impl AdmissionSnapshot {
    pub fn track(&self, client: &TelemetryClient) {
        let mut properties = Properties::new();
        properties.set("providers_count", Value::from(self.providers_count));
        properties.set("total_in_flight", Value::from(self.total_in_flight));
        properties.set("total_queued", Value::from(self.total_queued));
        properties.set("any_open_breakers", Value::from(self.any_open_breakers));
        client.track("admission_snapshot", properties);
    }
}

/// One AIMD limit step (`admission_promoted` on the way up,
/// `admission_demoted` on the way down). The reason comes from
/// [`crate::catalog::ADMISSION_PROMOTE_REASONS`] /
/// [`crate::catalog::ADMISSION_DEMOTE_REASONS`].
#[derive(Debug, Clone)]
pub struct AdmissionLimitStep {
    pub provider: String,
    pub new_limit: u64,
    pub old_limit: u64,
    pub reason: &'static str,
}

impl AdmissionLimitStep {
    /// Record the step as a limit increase (`admission_promoted`).
    pub fn track_promoted(&self, client: &TelemetryClient) {
        self.track_named(client, "admission_promoted");
    }

    /// Record the step as a limit decrease (`admission_demoted`).
    pub fn track_demoted(&self, client: &TelemetryClient) {
        self.track_named(client, "admission_demoted");
    }

    fn track_named(&self, client: &TelemetryClient, name: &str) {
        let mut properties = Properties::new();
        properties.set("provider", Value::String(self.provider.clone()));
        properties.set("new_limit", Value::from(self.new_limit));
        properties.set("old_limit", Value::from(self.old_limit));
        properties.set("reason", Value::from(self.reason));
        client.track(name, properties);
    }
}

/// `router_decision`: one model-router decision at a turn boundary.
///
/// Counters, categories, and the closed reason vocabulary only. The chosen
/// and incumbent models arrive as CATEGORIES (the `model refused` precedent),
/// so no selector, provider key, or prompt ever rides the event: the router's
/// value is the decision shape, not the deployment's model list.
#[derive(Debug, Clone, Copy)]
pub struct RouterDecision<'a> {
    /// The `routing.mode` that produced the decision
    /// ([`crate::catalog::ROUTING_LEVELS`]).
    pub level: &'static str,
    /// The outcome ([`crate::catalog::ROUTING_REASONS`]).
    pub reason: &'static str,
    /// The model category (model family) of the serving model
    /// ([`crate::catalog::MODEL_CATEGORIES`]).
    pub incumbent_model_category: &'a str,
    /// The model category of the candidate the router would serve.
    pub chosen_model_category: &'a str,
    pub candidate_count: u64,
    pub rejected_count: u64,
    /// The chosen row's score in thousandths of a point.
    pub chosen_score_milli: Option<u64>,
    /// The runner-up's score in thousandths of a point.
    pub runner_up_score_milli: Option<u64>,
    /// Whether this decision moved the serving target off the incumbent.
    pub switched: bool,
    /// The incumbent's dwell (turns since the last switch).
    pub dwell_turns: u64,
}

impl RouterDecision<'_> {
    pub fn track(&self, client: &TelemetryClient) {
        let mut properties = Properties::new();
        properties.set("level", Value::from(self.level));
        properties.set("reason", Value::from(self.reason));
        properties.set(
            "incumbent_model_category",
            Value::from(self.incumbent_model_category),
        );
        properties.set(
            "chosen_model_category",
            Value::from(self.chosen_model_category),
        );
        properties.set("candidate_count", Value::from(self.candidate_count));
        properties.set("rejected_count", Value::from(self.rejected_count));
        if let Some(score) = self.chosen_score_milli {
            properties.set("chosen_score_milli", Value::from(score));
        }
        if let Some(score) = self.runner_up_score_milli {
            properties.set("runner_up_score_milli", Value::from(score));
        }
        properties.set("switched", Value::from(self.switched));
        properties.set("dwell_turns", Value::from(self.dwell_turns));
        client.track("router_decision", properties);
    }
}

/// `routing_setting_changed`: the operator moved `routing.mode`.
#[derive(Debug, Clone, Copy)]
pub struct RoutingSettingChanged {
    /// The level now in force ([`crate::catalog::ROUTING_LEVELS`]).
    pub level: &'static str,
}

impl RoutingSettingChanged {
    pub fn track(&self, client: &TelemetryClient) {
        let mut properties = Properties::new();
        properties.set("level", Value::from(self.level));
        client.track("routing_setting_changed", properties);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::{RUN_TRIGGERS, TOOL_CATEGORIES};

    #[test]
    fn tool_category_mapping() {
        assert_eq!(ToolCategory::from_tool_name("bash"), ToolCategory::Bash);
        assert_eq!(ToolCategory::from_tool_name("Edit"), ToolCategory::Edit);
        assert_eq!(
            ToolCategory::from_tool_name("python_repl"),
            ToolCategory::Ipython
        );
        assert_eq!(
            ToolCategory::from_tool_name("mcp__github__create_issue"),
            ToolCategory::Mcp
        );
        assert_eq!(
            ToolCategory::from_tool_name("web-search"),
            ToolCategory::Custom
        );
        assert_eq!(ToolCategory::from_tool_name(""), ToolCategory::Unknown);
    }

    /// FORK-ONLY: the three native fork tools each own a category. Before this,
    /// `graph`, `lsp` and `jev` all fell into `Custom`, so `finalize_run_locked`
    /// folded their call/error/duration rows into one `custom_tool_*` aggregate
    /// and the run summary could not say which cost the time.
    #[test]
    fn the_fork_tools_have_their_own_categories() {
        assert_eq!(ToolCategory::from_tool_name("graph"), ToolCategory::Graph);
        assert_eq!(ToolCategory::from_tool_name("lsp"), ToolCategory::Lsp);
        assert_eq!(ToolCategory::from_tool_name("jev"), ToolCategory::Jev);
        // The wire names the catalog rules are keyed by.
        assert_eq!(ToolCategory::Graph.as_str(), "graph");
        assert_eq!(ToolCategory::Lsp.as_str(), "lsp");
        assert_eq!(ToolCategory::Jev.as_str(), "jev");
        // The custom bucket keeps meaning "a tool we do not know by name".
        assert_eq!(
            ToolCategory::from_tool_name("web-search"),
            ToolCategory::Custom
        );
    }

    #[test]
    fn vocabulary_consts_stay_in_the_catalog() {
        for trigger in ["prompt", "continuation", "unknown"] {
            assert!(RUN_TRIGGERS.contains(&trigger));
        }
        for category in [
            ToolCategory::Read,
            ToolCategory::Write,
            ToolCategory::Edit,
            ToolCategory::Bash,
            ToolCategory::Grep,
            ToolCategory::Find,
            ToolCategory::Ls,
            ToolCategory::Ipython,
            ToolCategory::Mcp,
            ToolCategory::Custom,
            ToolCategory::Unknown,
        ] {
            assert!(TOOL_CATEGORIES.contains(&category.as_str()));
        }
    }

    #[test]
    fn router_vocabularies_stay_in_the_catalog() {
        for level in ["off", "dry-run", "auto", "on"] {
            assert!(crate::catalog::ROUTING_LEVELS.contains(&level));
        }
        for reason in [
            "incumbent_retained",
            "challenger_won",
            "incumbent_failed",
            "no_incumbent",
            "all_rejected",
            "disabled",
        ] {
            assert!(crate::catalog::ROUTING_REASONS.contains(&reason));
        }
        let rule = crate::catalog::lookup("router_decision").expect("catalogued");
        for key in [
            "level",
            "reason",
            "incumbent_model_category",
            "chosen_model_category",
            "candidate_count",
            "rejected_count",
            "chosen_score_milli",
            "runner_up_score_milli",
            "switched",
            "dwell_turns",
        ] {
            assert!(
                rule.properties.iter().any(|(known, _)| *known == key),
                "{key} joins router_decision"
            );
        }
    }
}
