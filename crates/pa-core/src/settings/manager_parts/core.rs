//! Loading, saving and raw scope access: the manager's own state.
use std::collections::BTreeSet;
use std::sync::Arc;

use anyhow::Result;

use crate::settings::manager::{
    load_scope, settings_document, Settings, SettingsError, SettingsManager, SettingsStorage,
    RECENT_MODELS_LIMIT,
};
use crate::settings::merge::deep_merge;
use crate::settings::storage::SettingsScope;

impl SettingsManager {
    pub fn from_storage(storage: Arc<dyn SettingsStorage>) -> Self {
        let mut errors = Vec::new();
        let (global, global_raw, global_load_error) =
            load_scope(storage.as_ref(), SettingsScope::Global, &mut errors);
        let (project, _, project_load_error) =
            load_scope(storage.as_ref(), SettingsScope::Project, &mut errors);
        let merged = deep_merge(&global, &project);
        Self {
            storage,
            global,
            pending_global_fields: BTreeSet::new(),
            project,
            merged,
            runtime_overrides: Settings::default(),
            global_raw,
            errors,
            global_load_error,
            project_load_error,
        }
    }

    pub fn create(
        cwd: impl AsRef<std::path::Path>,
        agent_dir: impl AsRef<std::path::Path>,
    ) -> Self {
        let cwd = std::path::PathBuf::from(cwd.as_ref());
        let agent_dir = std::path::PathBuf::from(agent_dir.as_ref());
        let storage: Arc<dyn SettingsStorage> = Arc::new(
            crate::settings::storage::FileSettingsStorage::new(cwd, agent_dir),
        );
        Self::from_storage(storage)
    }

    /// A new manager over the same settings store (a fresh read; the
    /// runtime overrides carry over), for long-lived readers such as the
    /// telemetry switch.
    #[must_use]
    pub fn reopen(&self) -> Self {
        let mut fresh = Self::from_storage(Arc::clone(&self.storage));
        fresh.apply_overrides(&self.runtime_overrides);
        fresh
    }

    /// In-memory manager (tests, embedded hosts).
    #[must_use]
    pub fn in_memory(initial: &Settings) -> Self {
        let storage: Arc<dyn SettingsStorage> =
            Arc::new(crate::settings::storage::InMemorySettingsStorage::default());
        let content = serde_json::to_string_pretty(&initial).unwrap_or_default();
        storage
            .with_lock(SettingsScope::Global, &mut |current| {
                let _ = current;
                Some(content.clone())
            })
            .ok();
        Self::from_storage(storage)
    }

    #[must_use]
    pub fn settings(&self) -> &Settings {
        &self.merged
    }

    #[must_use]
    pub fn global_settings(&self) -> &Settings {
        &self.global
    }

    /// The raw global document (post-migration, pre-lenient-load value);
    /// `None` when the scope has no document or it failed to parse.
    #[must_use]
    pub fn global_raw(&self) -> Option<&serde_json::Value> {
        self.global_raw.as_ref()
    }

    #[must_use]
    pub fn project_settings(&self) -> &Settings {
        &self.project
    }

    #[must_use]
    pub fn errors(&self) -> &[SettingsError] {
        &self.errors
    }

    /// Take and clear the recorded settings errors (warnings are printed once
    /// by the CLI commands that surface them).
    pub fn drain_errors(&mut self) -> Vec<SettingsError> {
        std::mem::take(&mut self.errors)
    }

    /// # Errors
    ///
    /// Never returns `Err`; load problems are recorded as load errors on
    /// the manager instead.
    pub fn reload(&mut self) -> Result<()> {
        let mut errors = std::mem::take(&mut self.errors);
        let (global, global_raw, global_load_error) =
            load_scope(self.storage.as_ref(), SettingsScope::Global, &mut errors);
        let (project, _, project_load_error) =
            load_scope(self.storage.as_ref(), SettingsScope::Project, &mut errors);
        self.global = global;
        self.pending_global_fields.clear();
        self.project = project;
        self.global_raw = global_raw;
        self.global_load_error = global_load_error;
        self.project_load_error = project_load_error;
        self.errors = errors;
        self.merged = deep_merge(&self.global, &self.project);
        Ok(())
    }

    /// Runtime overrides layered on top (CLI flags); not persisted.
    pub fn apply_overrides(&mut self, overrides: &Settings) {
        self.merged = deep_merge(&self.merged, overrides);
        self.runtime_overrides = deep_merge(&self.runtime_overrides, overrides);
    }

    /// Mutable global settings for the setters in sibling modules.
    pub(crate) fn global_mut(&mut self) -> &mut Settings {
        &mut self.global
    }

    /// # Errors
    ///
    /// Returns an error when the global settings file cannot be written.
    pub(crate) fn save_global_scope(&mut self, fields: &[&'static str]) -> Result<()> {
        self.save_global(fields)
    }

    // -- persisted setters --------------------------------------------------

    /// Append a model to `recentModels`.
    ///
    /// Private: its only caller is [`Self::set_default_model_and_provider`], which also
    /// persists the field. A `pub` method here invited callers to record a use without
    /// saving it, leaving `pending_global_fields` set for the next save to flush.
    pub(super) fn record_model_use(&mut self, provider: &str, model: &str) {
        let key = format!("{provider}/{model}");
        let mut recent: Vec<String> = vec![key];
        for existing in self.global.recent_models.iter().flatten() {
            if existing != &recent[0] {
                recent.push(existing.clone());
            }
        }
        recent.truncate(RECENT_MODELS_LIMIT);
        self.global.recent_models = Some(recent);
        self.pending_global_fields.insert("recentModels");
    }

    /// Write the global scope back (project scope is host-written, not
    /// user-set), then re-derive the effective settings.
    pub(super) fn save_global(&mut self, fields: &[&'static str]) -> Result<()> {
        self.merged = deep_merge(&self.global, &self.project);
        self.pending_global_fields.extend(fields.iter().copied());
        if let Some(message) = &self.global_load_error {
            self.errors.push(SettingsError {
                scope: SettingsScope::Global,
                message: format!(
                    "Global settings not saved: settings file failed to parse: {message}"
                ),
            });
            return Ok(());
        }
        let snapshot = serde_json::to_value(&self.global)?;
        let mut failure = None;
        let mut saved = None;
        self.storage
            .with_lock(SettingsScope::Global, &mut |current| {
                let result = (|| -> Result<String> {
                    let mut document = settings_document(current.as_deref())?;
                    for field in &self.pending_global_fields {
                        let (map, key, value) = if let Some((parent, child)) = field.split_once('.')
                        {
                            let nested = document
                                .entry(parent.to_string())
                                .or_insert_with(|| serde_json::json!({}));
                            if !nested.is_object() {
                                *nested = serde_json::json!({});
                            }
                            (
                                nested.as_object_mut().unwrap(),
                                child,
                                &snapshot[parent][child],
                            )
                        } else {
                            (&mut document, *field, &snapshot[*field])
                        };
                        if value.is_null() {
                            map.remove(key);
                        } else {
                            map.insert(key.to_string(), value.clone());
                        }
                    }
                    let value = serde_json::Value::Object(document);
                    let content = serde_json::to_string_pretty(&value)?;
                    saved = Some(value);
                    Ok(content)
                })();
                match result {
                    Ok(content) => Some(content),
                    Err(error) => {
                        failure = Some(error);
                        None
                    }
                }
            })?;
        if let Some(error) = failure {
            return Err(error);
        }
        self.global_raw = saved;
        self.pending_global_fields.clear();
        Ok(())
    }
}
