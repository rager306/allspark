//! `SettingsManager`: loads global + project settings, merges them, tracks
//! modified fields, and writes back only what this session changed.

use std::collections::BTreeSet;
use std::sync::Arc;

use anyhow::{anyhow, Context, Result};

// `pub(super)` so the `manager_parts` submodules inherit the same prelude: they are
// `impl SettingsManager` blocks moved out of this file, and re-listing a dozen imports in
// each would drift the moment this file gains one.
pub(super) use super::load::from_value_lenient;
pub(super) use super::merge::migrate;
pub(super) use super::storage::{SettingsScope, SettingsStorage};
pub(super) use super::types::Settings;

pub const RECENT_MODELS_LIMIT: usize = 20;
pub const DEFAULT_IDLE_EVICTION_MINUTES: u64 = 90;
/// The compiled per-`(provider, model)` admission limit: the AIMD start
/// limit AND the default `[agents] maxConcurrentPerProvider` ceiling.
pub const DEFAULT_PROVIDER_CONCURRENCY_LIMIT: usize = 6;

#[derive(Debug, Clone)]
pub struct SettingsError {
    pub scope: SettingsScope,
    pub message: String,
}

pub struct SettingsManager {
    storage: Arc<dyn SettingsStorage>,
    global: Settings,
    pending_global_fields: BTreeSet<&'static str>,
    project: Settings,
    merged: Settings,
    runtime_overrides: Settings,
    errors: Vec<SettingsError>,
    /// The raw global document (parsed JSON before the lenient load): the allowlist
    /// gate distinguishes an ABSENT `allowedModels` key (unrestricted) from a
    /// PRESENT-but-malformed one (fails closed); typed `Settings` drops both to `None`.
    global_raw: Option<serde_json::Value>,
    /// Load failures per scope; a scope whose file failed to parse is never written back.
    global_load_error: Option<String>,
    project_load_error: Option<String>,
}

/// FORK-ONLY: the resolved debug-log configuration handed to the
/// rotating-subscriber installer (`pa_daemon::debug_log::init`).
#[derive(Debug, Clone, Default)]
pub struct DebugLogConfig {
    pub enabled: bool,
    pub level: String,
    pub max_files: u32,
    pub max_bytes: u64,
}

#[path = "manager_parts/mod.rs"]
mod manager_parts;

/// Parse inside the storage lock; malformed or non-object documents must never
/// be replaced with defaults, including when another writer changed them after load.
pub(super) fn settings_document(
    current: Option<&str>,
) -> Result<serde_json::Map<String, serde_json::Value>> {
    let value: serde_json::Value = serde_json::from_str(current.unwrap_or("{}"))
        .context("parsing settings.json; settings not saved")?;
    let serde_json::Value::Object(mut map) = value else {
        return Err(anyhow!(
            "settings.json must contain a JSON object; settings not saved"
        ));
    };
    migrate(&mut map);
    Ok(map)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdleEviction {
    Minutes(u64),
    Off,
}

pub(super) fn strings(array: &[serde_json::Value]) -> Vec<String> {
    array
        .iter()
        .filter_map(|value| value.as_str().map(str::to_string))
        .collect()
}

/// One loaded scope: the leniently-parsed settings, the migrated raw document,
/// and the load error (`Some` when the document exists but cannot be read/parsed).
#[allow(clippy::type_complexity)]
pub(super) fn load_scope(
    storage: &dyn SettingsStorage,
    scope: SettingsScope,
    errors: &mut Vec<SettingsError>,
) -> (Settings, Option<serde_json::Value>, Option<String>) {
    let mut load_error: Option<String> = None;
    // The pure-read arm: a locked protocol read on any cache miss, the
    // process-cached copy on a hit (see `SettingsStorage::read`).
    let content = match storage.read(scope) {
        Ok(content) => content,
        Err(error) => {
            let message = error.to_string();
            errors.push(SettingsError {
                scope,
                message: message.clone(),
            });
            return (Settings::default(), None, Some(message));
        }
    };
    let Some(content) = content else {
        return (Settings::default(), None, None);
    };
    let value: serde_json::Value = match serde_json::from_str(&content) {
        Ok(value) => value,
        Err(error) => {
            load_error = Some(error.to_string());
            serde_json::Value::Null
        }
    };
    if let Some(message) = load_error {
        errors.push(SettingsError {
            scope,
            message: message.clone(),
        });
        return (Settings::default(), None, Some(message));
    }
    let migrated = match value {
        serde_json::Value::Object(mut map) => {
            migrate(&mut map);
            serde_json::Value::Object(map)
        }
        other => other,
    };
    (from_value_lenient(&migrated), Some(migrated), None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::types::{MarkdownSettings, DEFAULT_PYTHON_CELL_TIMEOUT_MS};
    use crate::settings::types::{QueueModeSetting, SubagentModelChoice, SubagentModelRung};

    /// Serializes the debug-log tests that touch the process-wide
    /// `ALLSPARK_DEBUG` variable (`std::env` is shared across test threads).
    fn debug_log_env_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        LOCK.lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Runs `body` with `ALLSPARK_DEBUG` unset, restoring the prior value.
    fn without_debug_log_env<T>(body: impl FnOnce() -> T) -> T {
        let saved = std::env::var_os("ALLSPARK_DEBUG");
        std::env::remove_var("ALLSPARK_DEBUG");
        let outcome = body();
        match saved {
            Some(value) => std::env::set_var("ALLSPARK_DEBUG", value),
            None => std::env::remove_var("ALLSPARK_DEBUG"),
        }
        outcome
    }

    /// `[agents] maxConcurrentPerProvider`: unset reads the compiled default
    /// (the AIMD start), a positive value is the ceiling, and a `0` or a
    /// wrong-typed value behaves as unset instead of closing the pool.
    #[test]
    fn agents_max_concurrent_per_provider_resolves() {
        let unset = SettingsManager::in_memory(&Settings::default());
        assert_eq!(
            unset.get_max_concurrent_per_provider(),
            DEFAULT_PROVIDER_CONCURRENCY_LIMIT
        );

        let configured: Settings = serde_json::from_value(
            serde_json::json!({ "agents": { "maxConcurrentPerProvider": 12 } }),
        )
        .unwrap();
        let configured = SettingsManager::in_memory(&configured);
        assert_eq!(configured.get_max_concurrent_per_provider(), 12);

        // `0` is not a pool: it reads the default, never a closed gate.
        let zero: Settings = serde_json::from_value(
            serde_json::json!({ "agents": { "maxConcurrentPerProvider": 0 } }),
        )
        .unwrap();
        assert_eq!(
            SettingsManager::in_memory(&zero).get_max_concurrent_per_provider(),
            DEFAULT_PROVIDER_CONCURRENCY_LIMIT
        );

        // A wrong-typed value loads as unset (the lenient-load contract).
        let lenient = crate::settings::load::from_value_lenient(&serde_json::json!({
            "agents": { "maxConcurrentPerProvider": "lots" }
        }));
        assert_eq!(
            SettingsManager::in_memory(&lenient).get_max_concurrent_per_provider(),
            DEFAULT_PROVIDER_CONCURRENCY_LIMIT
        );
    }

    /// The `debugLog` block reads its level/rotation bounds; an invalid level
    /// normalizes to `debug` and unset bounds fall back to the defaults.
    #[test]
    fn debug_log_settings_resolve() {
        let _lock = debug_log_env_lock();
        without_debug_log_env(|| {
            // Unset: off, with the default level and bounds.
            let cfg = SettingsManager::in_memory(&Settings::default()).get_debug_log();
            assert!(!cfg.enabled);
            assert_eq!(cfg.level, "debug");
            assert_eq!(cfg.max_files, 10);
            assert_eq!(cfg.max_bytes, 10 * 1024 * 1024);

            // The settings block: enabled + custom level/bounds.
            let settings: Settings = serde_json::from_value(serde_json::json!({
                "debugLog": { "enabled": true, "level": "warn", "maxFiles": 3, "maxBytes": 2048 }
            }))
            .unwrap();
            let cfg = SettingsManager::in_memory(&settings).get_debug_log();
            assert!(cfg.enabled);
            assert_eq!(cfg.level, "warn");
            assert_eq!(cfg.max_files, 3);
            assert_eq!(cfg.max_bytes, 2048);

            // An invalid level reads as `debug`.
            let settings: Settings = serde_json::from_value(serde_json::json!({
                "debugLog": { "enabled": true, "level": "verbose" }
            }))
            .unwrap();
            assert_eq!(
                SettingsManager::in_memory(&settings).get_debug_log().level,
                "debug",
                "an unknown level falls back to debug"
            );
        });
    }

    /// `ALLSPARK_DEBUG=1` enables the log even with no settings block.
    #[test]
    fn debug_log_env_flag_forces_enablement() {
        let _lock = debug_log_env_lock();
        let saved = std::env::var_os("ALLSPARK_DEBUG");
        std::env::set_var("ALLSPARK_DEBUG", "1");
        let enabled = SettingsManager::in_memory(&Settings::default())
            .get_debug_log()
            .enabled;
        match saved {
            Some(value) => std::env::set_var("ALLSPARK_DEBUG", value),
            None => std::env::remove_var("ALLSPARK_DEBUG"),
        }
        assert!(enabled, "ALLSPARK_DEBUG=1 forces the debug log on");
    }

    #[test]
    fn in_memory_loads_merges_and_saves() {
        let mut manager = SettingsManager::in_memory(&Settings::default());
        manager
            .set_default_model_and_provider("prime-inference", "z-ai/glm-5.3")
            .unwrap();
        assert_eq!(manager.get_default_provider(), Some("prime-inference"));
        assert_eq!(manager.get_default_model(), Some("z-ai/glm-5.3"));
        assert_eq!(
            manager.get_recent_models(),
            vec!["prime-inference/z-ai/glm-5.3".to_string()]
        );
        manager.reload().unwrap();
        assert_eq!(manager.get_default_model(), Some("z-ai/glm-5.3"));
    }

    /// `factory.enabled` reads the GLOBAL scope only (the agent-dir
    /// document the kernel's factory gate and `set_factory_enabled` use):
    /// a project-scope override can never flip the gate out from under the
    /// kernel — a project `.prime/agent/settings.json` with `enabled: true`
    /// leaves the factory disabled while the global setting says nothing
    /// (Macroscope review finding: the merged read could diverge the
    /// daemon's lane advertisement and the client's `/factory status`
    /// from what the kernel would do).
    #[test]
    fn factory_enabled_reads_the_global_scope_only() {
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path().join("cwd");
        let agent_dir = dir.path().join("agent");
        std::fs::create_dir_all(cwd.join(".prime/agent")).unwrap();
        std::fs::create_dir_all(&agent_dir).unwrap();
        std::fs::write(
            cwd.join(".prime/agent/settings.json"),
            r#"{ "factory": { "enabled": true } }"#,
        )
        .unwrap();

        let mut manager = SettingsManager::create(&cwd, &agent_dir);
        assert!(
            !manager.get_factory_enabled(),
            "a project-scope override never enables the gate"
        );

        manager
            .set_factory_enabled(true)
            .expect("set factory enabled");
        assert!(manager.get_factory_enabled());
        let content =
            std::fs::read_to_string(agent_dir.join("settings.json")).expect("global file");
        assert!(
            content.contains(r#""enabled": true"#),
            "the write lands in the global document the kernel gate reads: {content}"
        );

        manager
            .set_factory_enabled(false)
            .expect("set factory disabled");
        assert!(!manager.get_factory_enabled());
    }

    #[test]
    fn reopen_keeps_the_runtime_overrides_in_the_merged_settings() {
        let mut manager = SettingsManager::in_memory(&Settings::default());
        manager.apply_overrides(&Settings {
            markdown: Some(MarkdownSettings {
                code_block_indent: Some("    ".to_string()),
                mermaid: None,
            }),
            ..Settings::default()
        });
        assert_eq!(manager.reopen().get_code_block_indent(), "    ");
    }

    #[test]
    fn code_block_indent_reads_markdown_settings_with_ts_default() {
        let manager = SettingsManager::in_memory(&Settings::default());
        assert_eq!(manager.get_code_block_indent(), "  ");

        let settings = Settings {
            markdown: Some(MarkdownSettings {
                code_block_indent: Some("    ".to_string()),
                mermaid: None,
            }),
            ..Settings::default()
        };
        let manager = SettingsManager::in_memory(&settings);
        assert_eq!(manager.get_code_block_indent(), "    ");
    }

    #[test]
    fn chat_detail_persists_the_chosen_level_with_the_startup_fallback() {
        let mut manager = SettingsManager::in_memory(&Settings::default());
        assert_eq!(manager.get_chat_detail(), "overview");
        manager.set_chat_detail("all").unwrap();
        assert_eq!(manager.get_chat_detail(), "all");
        manager.reload().unwrap();
        assert_eq!(
            manager.get_chat_detail(),
            "all",
            "the saved level survives a reload (a later chat re-reads it)"
        );
        manager.set_chat_detail("details").unwrap();
        assert_eq!(
            manager.get_chat_detail(),
            "details",
            "a saved details level reads back exactly (the Ctrl+O thinking reveal persists)"
        );
        manager.set_chat_detail("verbose").unwrap();
        assert_eq!(
            manager.get_chat_detail(),
            "overview",
            "an invalid value falls back to the startup default"
        );
    }

    #[test]
    fn steering_mode_defaults_to_all_follow_ups_stay_one_at_a_time() {
        let mut manager = SettingsManager::in_memory(&Settings::default());
        assert_eq!(manager.get_steering_mode(), QueueModeSetting::All);
        assert_eq!(manager.get_follow_up_mode(), QueueModeSetting::OneAtATime);
        manager
            .set_steering_mode(QueueModeSetting::OneAtATime)
            .unwrap();
        assert_eq!(
            manager.get_steering_mode(),
            QueueModeSetting::OneAtATime,
            "the explicit one-at-a-time setting still selects one-per-turn"
        );
    }

    #[test]
    fn migrations_apply_on_load() {
        let storage: Arc<dyn SettingsStorage> =
            Arc::new(super::super::storage::InMemorySettingsStorage::default());
        storage
            .with_lock(SettingsScope::Global, &mut |_| {
                Some(r#"{ "queueMode": "all", "telemetry": true }"#.into())
            })
            .unwrap();
        let manager = SettingsManager::from_storage(storage);
        assert_eq!(manager.get_steering_mode(), QueueModeSetting::All);
        assert!(manager.get_telemetry_enabled());
    }

    #[test]
    fn wrong_typed_fields_never_fail_load() {
        let storage: Arc<dyn SettingsStorage> =
            Arc::new(super::super::storage::InMemorySettingsStorage::default());
        storage
            .with_lock(SettingsScope::Global, &mut |_| {
                Some(r#"{ "defaultProvider": 42, "theme": "prime" }"#.into())
            })
            .unwrap();
        let manager = SettingsManager::from_storage(storage);
        assert_eq!(manager.get_default_provider(), None);
        assert_eq!(manager.get_theme(), Some("prime"));
    }

    #[test]
    fn idle_eviction_semantics() {
        let mut manager = SettingsManager::in_memory(&Settings::default());
        assert_eq!(
            manager.get_idle_eviction(),
            IdleEviction::Minutes(DEFAULT_IDLE_EVICTION_MINUTES)
        );
        manager.global.idle_eviction_minutes = Some(serde_json::json!("off"));
        assert_eq!(manager.get_idle_eviction(), IdleEviction::Off);
        manager.global.idle_eviction_minutes = Some(serde_json::json!(0));
        assert_eq!(
            manager.get_idle_eviction(),
            IdleEviction::Minutes(DEFAULT_IDLE_EVICTION_MINUTES)
        );
        manager.global.idle_eviction_minutes = Some(serde_json::json!(45));
        assert_eq!(manager.get_idle_eviction(), IdleEviction::Minutes(45));
    }

    #[test]
    fn agent_traces_default_off_and_persist_the_opt_in() {
        let mut manager = SettingsManager::in_memory(&Settings::default());
        assert!(!manager.get_agent_traces_enabled());
        manager.set_agent_traces_enabled(true).unwrap();
        assert!(manager.get_agent_traces_enabled());
        manager.reload().unwrap();
        assert!(manager.get_agent_traces_enabled());
        manager.set_agent_traces_enabled(false).unwrap();
        assert!(!manager.get_agent_traces_enabled());
    }

    #[test]
    fn agent_traces_choice_written_marks_a_provisioned_home() {
        let mut manager = SettingsManager::in_memory(&Settings::default());
        assert!(!manager.agent_traces_choice_written());
        manager.set_agent_traces_enabled(false).unwrap();
        assert!(manager.agent_traces_choice_written());
        manager.reload().unwrap();
        assert!(manager.agent_traces_choice_written());
        manager.set_agent_traces_enabled(true).unwrap();
        assert!(manager.agent_traces_choice_written());
    }

    #[test]
    fn compaction_toggle_defaults_on_and_persists() {
        let mut manager = SettingsManager::in_memory(&Settings::default());
        assert!(manager.get_compaction_enabled());
        manager.set_compaction_enabled(false).unwrap();
        assert!(!manager.get_compaction_enabled());
        manager.reload().unwrap();
        assert!(!manager.get_compaction_enabled());
    }

    #[test]
    fn python_cell_timeout_reads_setting_or_default() {
        let manager = SettingsManager::in_memory(&Settings::default());
        assert_eq!(
            manager.get_python_cell_timeout_ms(),
            DEFAULT_PYTHON_CELL_TIMEOUT_MS
        );
        let manager = SettingsManager::in_memory(&Settings {
            python_cell_timeout_ms: Some(90_000),
            ..Settings::default()
        });
        assert_eq!(manager.get_python_cell_timeout_ms(), 90_000);
        // Zero is the explicit disable, not the default.
        let manager = SettingsManager::in_memory(&Settings {
            python_cell_timeout_ms: Some(0),
            ..Settings::default()
        });
        assert_eq!(manager.get_python_cell_timeout_ms(), 0);
    }

    #[test]
    fn image_model_reads_trimmed_or_unset() {
        let manager = SettingsManager::in_memory(&Settings {
            image_model: Some("  battery/mock-vision  ".to_string()),
            ..Settings::default()
        });
        assert_eq!(
            manager.get_image_model().as_deref(),
            Some("battery/mock-vision")
        );
        let manager = SettingsManager::in_memory(&Settings {
            image_model: Some("   ".to_string()),
            ..Settings::default()
        });
        assert_eq!(manager.get_image_model(), None);
        let manager = SettingsManager::in_memory(&Settings::default());
        assert_eq!(manager.get_image_model(), None);
    }
    #[test]
    fn native_tool_gates_default_on_and_honour_disable() {
        let manager = SettingsManager::in_memory(&Settings::default());
        assert!(manager.get_lsp_enabled(), "lsp defaults on");
        assert!(manager.get_graph_enabled(), "graph defaults on");
        assert_eq!(manager.get_graph_db_path(), None);

        let settings: Settings = serde_json::from_value(serde_json::json!({
            "lsp": { "enabled": false },
            "graph": { "enabled": false, "dbPath": "/tmp/custom-graph.db" },
        }))
        .unwrap();
        let manager = SettingsManager::in_memory(&settings);
        assert!(!manager.get_lsp_enabled());
        assert!(!manager.get_graph_enabled());
        assert_eq!(
            manager.get_graph_db_path(),
            Some(std::path::PathBuf::from("/tmp/custom-graph.db"))
        );
    }

    #[test]
    fn edit_hybrid_gate_and_mode_policy() {
        let manager = SettingsManager::in_memory(&Settings::default());
        assert!(manager.get_edit_hybrid_enabled(), "edit defaults on");
        assert_eq!(manager.get_edit_hybrid_default_mode(), None);
        assert_eq!(manager.get_edit_hybrid_allow_modes(), Vec::<String>::new());

        let settings: Settings = serde_json::from_value(serde_json::json!({
            "editHybrid": {
                "enabled": false,
                "defaultMode": "  Hashline  ",
                "allowModes": ["replace", "  hashline ", ""],
            },
        }))
        .unwrap();
        let manager = SettingsManager::in_memory(&settings);
        assert!(!manager.get_edit_hybrid_enabled());
        assert_eq!(
            manager.get_edit_hybrid_default_mode(),
            Some("hashline".to_string())
        );
        assert_eq!(
            manager.get_edit_hybrid_allow_modes(),
            vec!["replace".to_string(), "hashline".to_string()]
        );
    }

    #[test]
    fn graph_db_path_expands_a_home_prefix() {
        let home = pa_types::platform::home_dir().expect("home dir");
        let settings: Settings = serde_json::from_value(serde_json::json!({
            "graph": { "dbPath": "~/graph.db" },
        }))
        .unwrap();
        let manager = SettingsManager::in_memory(&settings);
        assert_eq!(manager.get_graph_db_path(), Some(home.join("graph.db")));
    }

    /// `lsp.idleTimeoutMs` had no reader: the field was declared and documented
    /// ("idle timeout for a live language-server client") while nothing consumed
    /// it, and testgov's loose substring rule hid that because the unrelated
    /// `resolve_stdin_idle_timeout_ms` contains the name. The accessor exists and
    /// the manager reads it now; a zero reads as unset so a stray `0` cannot turn
    /// a server into a spawn-per-request loop.
    #[test]
    fn lsp_idle_timeout_reads_the_setting_and_treats_zero_as_unset() {
        let read = |json: serde_json::Value| {
            let settings: Settings = serde_json::from_value(json).unwrap();
            SettingsManager::in_memory(&settings).get_lsp_idle_timeout()
        };
        assert_eq!(
            read(serde_json::json!({})),
            None,
            "unset stays session-long"
        );
        assert_eq!(
            read(serde_json::json!({ "lsp": { "enabled": true } })),
            None
        );
        assert_eq!(
            read(serde_json::json!({ "lsp": { "idleTimeoutMs": 90_000 } })),
            Some(std::time::Duration::from_secs(90))
        );
        assert_eq!(
            read(serde_json::json!({ "lsp": { "idleTimeoutMs": 0 } })),
            None,
            "zero must not mean evict-on-every-use"
        );
    }

    /// The image-model reference: `imageModel` wins, the legacy
    /// `providerBackupModel` string is the fallback, malformed values behave
    /// as unset.
    #[test]
    fn image_model_reads_the_legacy_provider_backup_alias() {
        let choice = |json: serde_json::Value| {
            let settings: Settings = serde_json::from_value(json).unwrap();
            SettingsManager::in_memory(&settings).get_image_model()
        };
        // Unset everywhere.
        assert_eq!(choice(serde_json::json!({})), None);
        // imageModel wins; the legacy alias alone serves.
        assert_eq!(
            choice(serde_json::json!({
                "imageModel": "prov/img",
                "providerBackupModel": "prov/old",
            }))
            .unwrap(),
            "prov/img"
        );
        assert_eq!(
            choice(serde_json::json!({ "providerBackupModel": " prov/old " })).unwrap(),
            "prov/old"
        );
        // Malformed values behave as unset.
        assert_eq!(
            choice(serde_json::json!({
                "imageModel": "   ",
                "providerBackupModel": "",
            })),
            None
        );
    }

    /// The `subagentModel` effort ladder: the effort picks its rung, unset
    /// rungs fall through to `default` and then the legacy string, `normal`
    /// and `medium` are the same rung, and malformed values behave as unset.
    #[test]
    fn subagent_model_ladder_resolves_by_effort() {
        let choice = |settings: &Settings, effort: Option<&str>| {
            SettingsManager::in_memory(settings).get_subagent_model_for_effort(effort)
        };

        // Unset everywhere: the parent inherits.
        assert_eq!(
            choice(&Settings::default(), Some("high")),
            None,
            "an unset ladder inherits the parent"
        );

        // The effort picks its rung; every other effort takes `default`.
        let settings: Settings = serde_json::from_value(serde_json::json!({
            "subagentModel": {
                "default": "prov/base",
                "low": "prov/cheap",
                "normal": { "model": "prov/mid", "thinking": "low" },
                "high": "prov/smart",
                "xhigh": "prov/genius",
                "max": { "model": "prov/max", "thinking": "xhigh" },
            },
        }))
        .unwrap();
        assert_eq!(choice(&settings, Some("low")).unwrap().model, "prov/cheap");
        assert_eq!(choice(&settings, Some("high")).unwrap().model, "prov/smart");
        // `medium` and `normal` are the same rung, and the pinned thinking rides it.
        assert_eq!(
            choice(&settings, Some("medium")),
            Some(SubagentModelChoice {
                model: "prov/mid".to_string(),
                thinking: Some("low".to_string()),
            })
        );
        assert_eq!(
            choice(&settings, Some("normal")),
            choice(&settings, Some("medium"))
        );
        // The pinned rung object: model + thinking.
        assert_eq!(
            choice(&settings, Some("max")),
            Some(SubagentModelChoice {
                model: "prov/max".to_string(),
                thinking: Some("xhigh".to_string()),
            })
        );
        // No effort (and unknown/off/minimal efforts) take `default`.
        assert_eq!(choice(&settings, None).unwrap().model, "prov/base");
        assert_eq!(
            choice(&settings, Some("minimal")).unwrap().model,
            "prov/base"
        );
        assert_eq!(choice(&settings, Some("bogus")).unwrap().model, "prov/base");

        // A missing rung falls through to `default`.
        let partial: Settings = serde_json::from_value(serde_json::json!({
            "subagentModel": { "default": "prov/base", "max": "prov/max" },
        }))
        .unwrap();
        assert_eq!(choice(&partial, Some("low")).unwrap().model, "prov/base");
        assert_eq!(choice(&partial, Some("max")).unwrap().model, "prov/max");

        // The legacy string is the last fallback and the legacy `default` rung.
        let legacy: Settings =
            serde_json::from_value(serde_json::json!({ "subagentDefaultModel": " prov/old " }))
                .unwrap();
        assert_eq!(
            choice(&legacy, Some("max")),
            Some(SubagentModelChoice {
                model: "prov/old".to_string(),
                thinking: None,
            })
        );
        // The ladder wins over the legacy string; the `default` rung still catches it.
        let both: Settings = serde_json::from_value(serde_json::json!({
            "subagentDefaultModel": "prov/old",
            "subagentModel": { "max": "prov/max" },
        }))
        .unwrap();
        assert_eq!(choice(&both, Some("max")).unwrap().model, "prov/max");
        assert_eq!(choice(&both, Some("low")).unwrap().model, "prov/old");

        // Malformed values behave as unset: whitespace references, empty
        // strings, and blank pinned thinking all fall through.
        let malformed: Settings = serde_json::from_value(serde_json::json!({
            "subagentModel": {
                "default": "   ",
                "high": { "model": "", "thinking": "  " },
            },
            "subagentDefaultModel": "",
        }))
        .unwrap();
        assert_eq!(
            choice(&malformed, Some("high")),
            None,
            "empty rungs fall through"
        );
        assert_eq!(
            choice(&malformed, None),
            None,
            "blank legacy behaves as unset"
        );

        // The `medium` JSON alias lands on the `normal` rung.
        let aliased: Settings = serde_json::from_value(serde_json::json!({
            "subagentModel": { "medium": "prov/via-alias" },
        }))
        .unwrap();
        assert_eq!(
            choice(&aliased, Some("normal")).unwrap().model,
            "prov/via-alias"
        );

        // Deep merge: the project file pins one rung over the global ladder
        // without wiping the rest (the deep-merge contract for nested objects).
        let global: Settings = serde_json::from_value(serde_json::json!({
            "subagentModel": { "default": "prov/base", "max": "prov/global-max" },
        }))
        .unwrap();
        let project: Settings = serde_json::from_value(serde_json::json!({
            "subagentModel": { "max": "prov/project-max" },
        }))
        .unwrap();
        let merged = super::super::merge::deep_merge(&global, &project);
        assert_eq!(
            choice(&merged, Some("max")).unwrap().model,
            "prov/project-max"
        );
        assert_eq!(
            choice(&merged, Some("low")).unwrap().model,
            "prov/base",
            "the unpinned rung survives the project overlay"
        );
    }

    /// The rung wire shapes: a bare reference string, the pinned object, the
    /// `medium` key alias, and a wrong-typed rung (the lenient-load contract).
    #[test]
    fn subagent_model_rung_wire_shapes() {
        // Bare string.
        let rung: SubagentModelRung =
            serde_json::from_value(serde_json::json!("prov/one")).unwrap();
        assert_eq!(rung, SubagentModelRung::Reference("prov/one".to_string()));

        // Pinned object, thinking present and absent.
        let rung: SubagentModelRung = serde_json::from_value(serde_json::json!({
            "model": "prov/two", "thinking": "high"
        }))
        .unwrap();
        assert_eq!(
            rung,
            SubagentModelRung::Pinned {
                model: "prov/two".to_string(),
                thinking: Some("high".to_string()),
            }
        );
        let rung: SubagentModelRung =
            serde_json::from_value(serde_json::json!({ "model": "prov/three" })).unwrap();
        assert_eq!(
            rung,
            SubagentModelRung::Pinned {
                model: "prov/three".to_string(),
                thinking: None,
            }
        );

        // A wrong-typed ladder value loads as unset (the lenient-load
        // contract: a bad document never blocks the session).
        let lenient = super::super::load::from_value_lenient(&serde_json::json!({
            "subagentModel": { "max": 42 },
        }));
        assert!(
            lenient.subagent_model.is_none()
                || matches!(
                    lenient.subagent_model.as_ref().and_then(|l| l.max.as_ref()),
                    Some(SubagentModelRung::Reference(_)) | None
                ),
            "a wrong-typed rung must not panic the loader"
        );
    }
}
