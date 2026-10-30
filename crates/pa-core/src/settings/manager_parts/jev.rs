//! The `[jev]` typed-judgment settings.

use crate::settings::manager::SettingsManager;

impl SettingsManager {
    /// FORK-ONLY: the typed-decision (judgment) layer's gate. Unset and
    /// unrecognized spellings read as OFF (a paid third-party endpoint;
    /// unlike `lsp`/`graph`, nothing wires by default).
    #[must_use]
    pub fn get_jev_enabled(&self) -> crate::settings::types::JevEnabledSetting {
        self.merged
            .jev
            .as_ref()
            .and_then(|jev| jev.enabled.as_deref())
            .and_then(crate::settings::types::JevEnabledSetting::parse)
            .unwrap_or_default()
    }

    /// The pinned judge model (`jev.model`); unset reads the official
    /// SDK's `TYPESAFE_DEFAULT_MODEL` env, else the ADR's pinned default.
    #[must_use]
    pub fn get_jev_model(&self) -> String {
        if let Some(model) = self
            .merged
            .jev
            .as_ref()
            .and_then(|jev| jev.model.as_deref())
            .map(str::trim)
            .filter(|model| !model.is_empty())
        {
            return model.to_string();
        }
        std::env::var("TYPESAFE_DEFAULT_MODEL")
            .ok()
            .map(|model| model.trim().to_string())
            .filter(|model| !model.is_empty())
            .unwrap_or_else(|| crate::jev::DEFAULT_JEV_MODEL.to_string())
    }

    /// The judge fallback chain (`jev.fallbackChain`), trimmed and emptied
    /// of blanks.
    #[must_use]
    pub fn get_jev_fallback_chain(&self) -> Vec<String> {
        self.merged
            .jev
            .as_ref()
            .and_then(|jev| jev.fallback_chain.clone())
            .unwrap_or_default()
            .into_iter()
            .map(|id| id.trim().to_string())
            .filter(|id| !id.is_empty())
            .collect()
    }

    /// The prompted-fallback mode (`jev.fallback`, `"off" | "llm"`).
    ///
    /// Documented and parsed with no reader (review #23): the sibling `fallback_chain` is a
    /// list of MODELS, this is about whether the judge may be re-asked with a prompt. Closed
    /// by default — an unrecognized or absent value reads as `off`, matching the field's own
    /// contract ("Phase 1 always treats it as off"), so a typo cannot enable an uncalibrated
    /// path.
    #[must_use]
    pub fn get_jev_fallback(&self) -> bool {
        self.merged
            .jev
            .as_ref()
            .and_then(|jev| jev.fallback.as_deref())
            .is_some_and(|mode| mode.trim().eq_ignore_ascii_case("llm"))
    }

    /// The judgment per-attempt timeout (`jev.timeoutMs`, default 10s).
    #[must_use]
    pub fn get_jev_timeout_ms(&self) -> u64 {
        self.merged
            .jev
            .as_ref()
            .and_then(|jev| jev.timeout_ms)
            .unwrap_or(pa_jev::DEFAULT_TIMEOUT_MS)
    }

    /// FORK-ONLY: the auto-thinking consumer's gate (`jev.autoThinking`).
    /// Unset and unrecognized spellings read as OFF - the classifier is
    /// advisory and never on by default.
    #[must_use]
    pub fn get_jev_auto_thinking(&self) -> crate::settings::types::JevAutoThinkingSetting {
        self.merged
            .jev
            .as_ref()
            .and_then(|jev| jev.auto_thinking.as_deref())
            .map(crate::settings::types::JevAutoThinkingSetting::parse)
            .unwrap_or_default()
    }

    /// FORK-ONLY: the graph re-rank consumer's gate (`jev.graphRerank`),
    /// OFF when unset.
    #[must_use]
    pub fn get_jev_graph_rerank(&self) -> crate::settings::types::JevGraphRerankSetting {
        self.merged
            .jev
            .as_ref()
            .and_then(|jev| jev.graph_rerank.as_deref())
            .map(crate::settings::types::JevGraphRerankSetting::parse)
            .unwrap_or_default()
    }

    /// FORK-ONLY: the tool-selection arbiter's gate (`jev.toolArbiter`),
    /// `auto` when unset (armed behind a native judge; the deterministic
    /// table needs no judge at all).
    #[must_use]
    pub fn get_jev_tool_arbiter(&self) -> crate::settings::types::JevToolArbiterSetting {
        self.merged
            .jev
            .as_ref()
            .and_then(|jev| jev.tool_arbiter.as_deref())
            .map(crate::settings::types::JevToolArbiterSetting::parse)
            .unwrap_or_default()
    }

    /// The judgment answer cache gate (`jev.cache.enabled`, default on).
    #[must_use]
    pub fn get_jev_cache_enabled(&self) -> bool {
        self.merged
            .jev
            .as_ref()
            .and_then(|jev| jev.cache.as_ref())
            .and_then(|cache| cache.enabled)
            .unwrap_or(true)
    }
}
