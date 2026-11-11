//! Onboarding flags and the package/resource arrays.

use anyhow::Result;

use crate::settings::manager::{settings_document, strings, SettingsError, SettingsManager};
use crate::settings::merge::deep_merge;
use crate::settings::storage::SettingsScope;

impl SettingsManager {
    /// # Errors
    ///
    /// Returns an error when the global settings file cannot be written.
    pub fn set_onboarding_shown(&mut self, shown: bool) -> Result<()> {
        self.global.onboarding_shown = Some(shown);
        self.save_global(&["onboardingShown"])
    }

    /// The shown flag with the legacy completed flag as fallback.
    #[must_use]
    pub fn get_onboarding_shown(&self) -> bool {
        self.merged
            .onboarding_shown
            .or(self.merged.onboarding_completed)
            .unwrap_or(false)
    }

    pub fn set_packages(&mut self, packages: Vec<serde_json::Value>) {
        self.global.packages = Some(packages.clone());
        self.persist_scope_field(
            SettingsScope::Global,
            "packages",
            &serde_json::Value::Array(packages),
        );
        self.merged = deep_merge(&self.global, &self.project);
    }

    /// Project package entries remain available for an explicit removal, but never
    /// participate in session resource resolution or update checks (upstream #3907).
    #[must_use]
    pub(crate) fn project_packages_for_removal(&self) -> Vec<serde_json::Value> {
        self.project_raw
            .as_ref()
            .and_then(|value| value.get("packages"))
            .and_then(serde_json::Value::as_array)
            .cloned()
            .unwrap_or_default()
    }

    pub fn set_project_packages(&mut self, packages: Vec<serde_json::Value>) {
        // The document keeps the entries so a later `package remove` can name them; the
        // EFFECTIVE settings never gain them.
        let value = serde_json::Value::Array(packages);
        self.persist_scope_field(SettingsScope::Project, "packages", &value);
        if let serde_json::Value::Object(project_raw) = self
            .project_raw
            .get_or_insert_with(|| serde_json::json!({}))
        {
            project_raw.insert("packages".into(), value);
        }
    }

    /// Replace one resource-path array (`skills`/`prompts`/`themes`) in the global settings file.
    pub fn set_global_resource_array(&mut self, field: &str, values: Vec<String>) {
        let array: Vec<serde_json::Value> =
            values.into_iter().map(serde_json::Value::String).collect();
        match field {
            "skills" => self.global.skills = Some(strings(&array)),
            "prompts" => self.global.prompts = Some(strings(&array)),
            "themes" => self.global.themes = Some(strings(&array)),
            _ => return,
        }
        self.persist_scope_field(
            SettingsScope::Global,
            field,
            &serde_json::Value::Array(array),
        );
        self.merged = deep_merge(&self.global, &self.project);
    }

    /// Replace one resource-path array in the project settings file.
    pub fn set_project_resource_array(&mut self, field: &str, values: Vec<String>) {
        let array: Vec<serde_json::Value> =
            values.into_iter().map(serde_json::Value::String).collect();
        match field {
            "skills" => self.project.skills = Some(strings(&array)),
            "prompts" => self.project.prompts = Some(strings(&array)),
            "themes" => self.project.themes = Some(strings(&array)),
            _ => return,
        }
        self.persist_scope_field(
            SettingsScope::Project,
            field,
            &serde_json::Value::Array(array),
        );
        self.merged = deep_merge(&self.global, &self.project);
    }

    /// Write one field into a scope's file, merging with the current on-disk document
    /// so concurrently-added fields survive; failures are recorded as warnings, never thrown.
    fn persist_scope_field(
        &mut self,
        scope: SettingsScope,
        field: &str,
        value: &serde_json::Value,
    ) {
        let load_error = match scope {
            SettingsScope::Global => self.global_load_error.clone(),
            SettingsScope::Project => self.project_load_error.clone(),
        };
        if let Some(message) = load_error {
            let label = match scope {
                SettingsScope::Global => "Global",
                SettingsScope::Project => "Project",
            };
            self.errors.push(SettingsError {
                scope,
                message: format!(
                    "{label} settings not saved: settings file failed to load: {message}"
                ),
            });
            return;
        }
        let mut failure = None;
        let result =
            self.storage.with_lock(
                scope,
                &mut |current| match settings_document(current.as_deref()).and_then(|mut map| {
                    map.insert(field.to_string(), value.clone());
                    serde_json::to_string_pretty(&serde_json::Value::Object(map))
                        .map_err(Into::into)
                }) {
                    Ok(content) => Some(content),
                    Err(error) => {
                        failure = Some(error);
                        None
                    }
                },
            );
        let result = result.and_then(|()| failure.map_or(Ok(()), Err));
        if let Err(error) = result {
            self.errors.push(SettingsError {
                scope,
                message: error.to_string(),
            });
        }
    }

    // -- getters with TS semantics ------------------------------------------
}
