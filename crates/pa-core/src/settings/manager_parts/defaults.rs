//! The model/provider defaults and the recent-model list.

use anyhow::Result;

use crate::settings::manager::SettingsManager;
use crate::settings::types::{SubagentModelChoice, SubagentModelRung};

impl SettingsManager {
    /// # Errors
    ///
    /// Returns an error when the global settings file cannot be written.
    pub fn set_default_provider(&mut self, provider: String) -> Result<()> {
        self.global.default_provider = Some(provider);
        self.save_global(&["defaultProvider"])
    }

    /// # Errors
    ///
    /// Returns an error when the global settings file cannot be written.
    pub fn set_default_model(&mut self, model: String) -> Result<()> {
        self.global.default_model = Some(model);
        self.save_global(&["defaultModel"])
    }

    /// # Errors
    ///
    /// Returns an error when the global settings file cannot be written.
    pub fn set_default_model_and_provider(&mut self, provider: &str, model: &str) -> Result<()> {
        self.global.default_provider = Some(provider.to_string());
        self.global.default_model = Some(model.to_string());
        self.record_model_use(provider, model);
        self.save_global(&["defaultProvider", "defaultModel", "recentModels"])
    }

    #[must_use]
    pub fn get_default_provider(&self) -> Option<&str> {
        self.merged.default_provider.as_deref()
    }

    #[must_use]
    pub fn get_default_model(&self) -> Option<&str> {
        self.merged.default_model.as_deref()
    }

    /// Model for `rlm.spawn` without a pinned model; unset inherits parent.
    ///
    /// The legacy `default` rung of the `subagentModel` ladder: superseded
    /// by the ladder, still honored last (see
    /// [`Self::get_subagent_model_for_effort`]).
    #[must_use]
    pub fn get_subagent_default_model(&self) -> Option<String> {
        self.merged
            .subagent_default_model
            .as_ref()
            .map(|m| m.trim().to_string())
            .filter(|m| !m.is_empty())
    }

    /// Model (+ optional pinned thinking) for `rlm.spawn` without a pinned
    /// model, resolved by the `subagentModel` effort ladder: the spawn's
    /// thinking level picks its rung (`normal` and `medium` name the same
    /// rung; `off`, `minimal`, unknown levels, and no level take the
    /// `default` rung), then the `default` rung, then the legacy
    /// `subagentDefaultModel` string. `None` inherits the parent's model.
    /// Whitespace or empty rung references behave as unset and fall
    /// through.
    #[must_use]
    pub fn get_subagent_model_for_effort(
        &self,
        effort: Option<&str>,
    ) -> Option<SubagentModelChoice> {
        let ladder = self.merged.subagent_model.as_ref();
        let rung_choice = |rung: Option<&SubagentModelRung>| -> Option<SubagentModelChoice> {
            match rung? {
                SubagentModelRung::Reference(model) => Some(SubagentModelChoice {
                    model: model.trim().to_string(),
                    thinking: None,
                }),
                SubagentModelRung::Pinned { model, thinking } => Some(SubagentModelChoice {
                    model: model.trim().to_string(),
                    thinking: thinking
                        .as_deref()
                        .map(str::trim)
                        .filter(|level| !level.is_empty())
                        .map(str::to_string),
                }),
            }
        };
        let effort_rung = ladder.and_then(|ladder| {
            match effort.map(str::trim).map(str::to_lowercase).as_deref() {
                Some("low") => rung_choice(ladder.low.as_ref()),
                Some("normal" | "medium") => rung_choice(ladder.normal.as_ref()),
                Some("high") => rung_choice(ladder.high.as_ref()),
                Some("xhigh") => rung_choice(ladder.xhigh.as_ref()),
                Some("max") => rung_choice(ladder.max.as_ref()),
                _ => None,
            }
        });
        effort_rung
            .or_else(|| ladder.and_then(|ladder| rung_choice(ladder.default.as_ref())))
            .or_else(|| {
                self.get_subagent_default_model()
                    .map(|model| SubagentModelChoice {
                        model,
                        thinking: None,
                    })
            })
            .filter(|choice| !choice.model.is_empty())
    }

    #[must_use]
    pub fn get_auxiliary_model(&self) -> Option<&str> {
        self.merged.auxiliary_model.as_deref()
    }

    /// The "provider/model-id" (or bare id) reference serving image turns on models
    /// without image input; malformed values behave as unset and the refusal names the setting.
    /// The legacy `providerBackupModel` string is the fallback when `imageModel`
    /// is unset (the same reference under its older name).
    #[must_use]
    pub fn get_image_model(&self) -> Option<String> {
        let reference_of = |value: Option<&String>| -> Option<String> {
            value
                .map(|m| m.trim().to_string())
                .filter(|m| !m.is_empty())
        };
        reference_of(self.merged.image_model.as_ref())
            .or_else(|| reference_of(self.merged.provider_backup_model.as_ref()))
    }

    /// The daemon-level model allowlist (settings `allowedModels`), enforced at every
    /// daemon model resolution — a model outside fails loudly, never a fallback.
    /// Rust-only guardrail; `None` is unrestricted; global scope only, so a project
    /// cannot weaken a box-level pin; a list that trims to empty behaves as unset.
    #[must_use]
    pub fn get_allowed_models(&self) -> Option<Vec<String>> {
        let patterns = self.global.allowed_models.as_ref()?;
        let patterns: Vec<String> = patterns
            .iter()
            .map(|pattern| pattern.trim().to_string())
            .filter(|pattern| !pattern.is_empty())
            .collect();
        (!patterns.is_empty()).then_some(patterns)
    }

    /// The persisted default a fresh session starts from; the stored
    /// string is the same vocabulary `get_default_service_tier` parses.
    ///
    /// # Errors
    ///
    /// Returns an error when the global settings file cannot be written.
    pub fn set_default_service_tier(&mut self, tier: pa_types::ai::ServiceTier) -> Result<()> {
        use pa_types::ai::ServiceTier;
        let name = match tier {
            ServiceTier::Auto => "auto",
            ServiceTier::Default => "default",
            ServiceTier::Flex => "flex",
            ServiceTier::Scale => "scale",
            ServiceTier::Priority => "priority",
        };
        self.global.default_service_tier = Some(name.to_string());
        self.save_global(&["defaultServiceTier"])
    }

    /// Service tier a fresh session records as its preference.
    /// An unrecognized setting value falls back to the same `default`.
    #[must_use]
    pub fn get_default_service_tier(&self) -> pa_types::ai::ServiceTier {
        use pa_types::ai::ServiceTier;
        self.merged
            .default_service_tier
            .as_deref()
            .and_then(|value| match value.trim().to_lowercase().as_str() {
                "auto" => Some(ServiceTier::Auto),
                "flex" => Some(ServiceTier::Flex),
                "scale" => Some(ServiceTier::Scale),
                "priority" => Some(ServiceTier::Priority),
                "default" => Some(ServiceTier::Default),
                _ => None,
            })
            .unwrap_or(ServiceTier::Default)
    }

    #[must_use]
    pub fn get_recent_models(&self) -> Vec<String> {
        self.merged.recent_models.clone().unwrap_or_default()
    }
}
