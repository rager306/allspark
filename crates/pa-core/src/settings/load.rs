//! Lenient settings loading: one wrong-typed value loads as unset (the TS
//! access-time typecheck semantics) and never a load error, and never takes a
//! valid sibling down with it.

use serde_json::{Map, Value};

use super::types::Settings;

/// One settings value the lenient load dropped: the dotted path from the
/// document root plus the reason it did not fit the typed schema.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IgnoredSetting {
    /// Dotted path from the document root, e.g. `lsp.idleTimeoutMs`.
    pub path: String,
    /// The deserializer's explanation ("invalid type: string \"soon\", expected u64").
    pub reason: String,
}

/// Extract each settings value independently; a value whose JSON type does not match the Rust
/// schema is dropped (the TS getters do the same check at access time), its valid siblings at
/// every nesting depth survive, and unknown keys land in `extra`.
///
/// Test-only: the shipping path must surface what it dropped, so `load_scope` calls
/// [`from_value_lenient_reporting`].
#[cfg(test)]
pub fn from_value_lenient(value: &Value) -> Settings {
    from_value_lenient_reporting(value).0
}

/// [`from_value_lenient`], plus one [`IgnoredSetting`] per dropped value: the caller surfaces
/// them as warnings, so a discarded configuration is never silent.
pub fn from_value_lenient_reporting(value: &Value) -> (Settings, Vec<IgnoredSetting>) {
    if let Ok(settings) = serde_json::from_value::<Settings>(value.clone()) {
        return (settings, Vec::new());
    }
    let Some(object) = value.as_object() else {
        return (Settings::default(), Vec::new());
    };
    let mut ignored = Vec::new();
    let merged = salvage_object(object, &[], &mut ignored);
    match serde_json::from_value::<Settings>(Value::Object(merged)) {
        Ok(settings) => (settings, ignored),
        // Every block is verified on its own before it is kept, so this cannot
        // happen; if a schema change ever makes it happen, it is reported rather
        // than silently loading defaults.
        Err(error) => {
            ignored.push(IgnoredSetting {
                path: String::new(),
                reason: format!("the recovered settings document does not parse: {error}"),
            });
            (Settings::default(), ignored)
        }
    }
}

/// Dotted paths in `document` that the settings schema does not know.
///
/// The lenient load reports a value whose TYPE is wrong and keeps every unknown key it
/// does not recognise — nested ones silently, because `Settings` has a flatten only at
/// the ROOT. So `retry.provider.waitForUsage.enabled` parsed as nothing at all: no value,
/// no warning, no error, and an operator who set it saw no change (review #31 — the last
/// of the four removed keys, and the mechanism the finding names).
///
/// Measured by round-trip instead of by a second schema: parse the document, serialize
/// the result back, and report every OBJECT path the input had and the output does not.
/// Two deliberate limits keep it honest:
///   * arrays are not descended — `packages`, `mcpServers` and `skills` hold arbitrary
///     user data whose shape the schema does not describe, so a key inside one is not an
///     unknown setting;
///   * a value already reported as wrong-typed is not reported again here (the caller
///     filters by path), so one key produces one message.
#[must_use]
pub fn keys_outside_the_schema(document: &Value, settings: &Settings) -> Vec<String> {
    let Ok(round_trip) = serde_json::to_value(settings) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    collect_missing_keys(document, &round_trip, &[], &mut out);
    out.sort();
    out.dedup();
    out
}

fn collect_missing_keys(input: &Value, parsed: &Value, spine: &[&str], out: &mut Vec<String>) {
    let (Some(input), Some(parsed)) = (input.as_object(), parsed.as_object()) else {
        return;
    };
    for (key, value) in input {
        let Some(known) = parsed.get(key) else {
            let mut path: Vec<&str> = spine.to_vec();
            path.push(key);
            out.push(path.join("."));
            continue;
        };
        let mut path: Vec<&str> = spine.to_vec();
        path.push(key);
        collect_missing_keys(value, known, &path, out);
    }
}

/// Keep the values of one JSON object that fit the settings schema, descending into a nested
/// object whose own value did not fit instead of discarding the whole block.
///
/// `spine` is the key path from the document root to `object`; a candidate value is tested by
/// rebuilding the one-key document that reaches it (`{"lsp":{"enabled":false}}`) and asking the
/// real `Settings` deserializer whether it fits. A block that loses every value this way is
/// dropped rather than left as an empty object.
fn salvage_object(
    object: &Map<String, Value>,
    spine: &[&str],
    ignored: &mut Vec<IgnoredSetting>,
) -> Map<String, Value> {
    let mut kept = Map::new();
    for (key, value) in object {
        let mut path = spine.to_vec();
        path.push(key);
        match fits_settings(&path, value) {
            Ok(()) => {
                kept.insert(key.clone(), value.clone());
            }
            Err(reason) => {
                // The value itself is the wrong shape; its own fields may still be right.
                let Some(nested) = value.as_object() else {
                    ignored.push(IgnoredSetting {
                        path: path.join("."),
                        reason,
                    });
                    continue;
                };
                let before = ignored.len();
                let salvaged = salvage_object(nested, &path, ignored);
                // A block is kept only if what survived still parses as that block: a field can
                // fit alone and not next to its siblings (a tagged enum entry missing its tag),
                // and keeping such a half-object would fail the whole document's final parse.
                let reassembled = Value::Object(salvaged);
                if salvaged_is_usable(&path, &reassembled) {
                    kept.insert(key.clone(), reassembled);
                } else {
                    // Nothing survives: one diagnostic for the block, naming the value that
                    // actually did not fit, instead of noise from every field it lost.
                    ignored.truncate(before);
                    ignored.push(IgnoredSetting {
                        path: path.join("."),
                        reason,
                    });
                }
            }
        }
    }
    kept
}

/// Whether a block built from its surviving fields is worth keeping: it must be non-empty and
/// still parse as that block.
fn salvaged_is_usable(spine: &[&str], block: &Value) -> bool {
    block
        .as_object()
        .is_some_and(|fields| !fields.is_empty() && fits_settings(spine, block).is_ok())
}

/// Ask the real deserializer whether `value` fits at `spine`: rebuild the one-key document that
/// reaches it and parse that. Using the shipped `Settings` type (rather than a second list of
/// known keys) is what keeps the two from drifting.
fn fits_settings(spine: &[&str], value: &Value) -> Result<(), String> {
    let mut document = value.clone();
    for key in spine.iter().rev() {
        let mut wrapper = Map::new();
        wrapper.insert((*key).to_string(), document);
        document = Value::Object(wrapper);
    }
    serde_json::from_value::<Settings>(document)
        .map(|_| ())
        .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A key nested inside a KNOWN block was invisible: it is not in `extra`, because the
    /// flatten is only at the root, and it is not wrong-typed, so the lenient load had
    /// nothing to say about it. The operator sets it, nothing reads it, and nothing says
    /// so — the exact mechanism review #31 names ("silently accepted and ignored").
    #[test]
    fn a_key_nested_inside_a_known_block_is_reported() {
        let document: Value = serde_json::json!({
            "retry": { "provider": { "waitForUsage": {
                "maxWaitMs": 12_345,
                "enabled": true,
                "baseDelayMs": 500,
                "maxDelayMs": 9_000,
                "maxAttempts": 3
            }}}
        });
        let (settings, _) = from_value_lenient_reporting(&document);
        let unknown = keys_outside_the_schema(&document, &settings);

        assert_eq!(
            unknown,
            vec![
                "retry.provider.waitForUsage.baseDelayMs".to_string(),
                "retry.provider.waitForUsage.enabled".to_string(),
                "retry.provider.waitForUsage.maxAttempts".to_string(),
                "retry.provider.waitForUsage.maxDelayMs".to_string(),
            ],
            "every unknown nested key is named, and the known one is not"
        );
        // The known sibling still lands: the report must not cost a valid value.
        assert_eq!(
            settings
                .retry
                .as_ref()
                .and_then(|retry| retry.provider.as_ref())
                .and_then(|provider| provider.wait_for_usage.as_ref())
                .and_then(|wait| wait.max_wait_ms),
            Some(12_345)
        );
    }

    /// User data inside an ARRAY is not a setting, whatever the schema says.
    ///
    /// `packages`, `mcpServers` and `skills` hold arbitrary user objects; descending into
    /// them would report every key of every entry as unknown, which is the same failure
    /// as silence, one turn later: an operator learns to ignore the warning.
    #[test]
    fn keys_inside_arrays_are_not_reported() {
        let document: Value = serde_json::json!({
            "packages": [{ "whatever": 1, "nested": { "deep": true } }],
            "mcpServers": { "s": { "command": "x", "args": ["a"] } }
        });
        let (settings, _) = from_value_lenient_reporting(&document);
        assert_eq!(
            keys_outside_the_schema(&document, &settings),
            Vec::<String>::new()
        );
    }

    #[test]
    fn wrong_typed_fields_load_as_none() {
        let value: Value = serde_json::json!({
            "defaultProvider": 42,
            "steeringMode": "all",
            "recentModels": "not-an-array",
            "unknownKey": { "kept": true }
        });
        let settings = from_value_lenient(&value);
        assert_eq!(settings.default_provider, None);
        assert_eq!(
            settings.steering_mode,
            Some(super::super::types::QueueModeSetting::All)
        );
        assert_eq!(settings.recent_models, None);
        assert_eq!(
            settings.extra.get("unknownKey"),
            Some(&serde_json::json!({ "kept": true }))
        );
    }

    #[test]
    fn wrong_typed_python_cell_timeout_loads_as_unset() {
        // A known field: a wrong-typed value must behave as unset (the
        // getter's default), not survive raw in `extra` where a save
        // would persist the bad key.
        let value: Value = serde_json::json!({
            "pythonCellTimeoutMs": "sixty seconds",
            "pythonCellTimeoutValid": 30_000
        });
        let settings = from_value_lenient(&value);
        assert_eq!(settings.python_cell_timeout_ms, None);
        assert!(
            !settings.extra.contains_key("pythonCellTimeoutMs"),
            "a wrong-typed known field never rides extra into a save"
        );
        assert_eq!(
            settings.extra.get("pythonCellTimeoutValid"),
            Some(&serde_json::json!(30_000)),
            "genuine unknown keys keep flowing through extra"
        );
    }

    #[test]
    fn clean_document_strict_parses() {
        let value: Value = serde_json::json!({ "theme": "prime", "rlmMaxDepth": 4 });
        let settings = from_value_lenient(&value);
        assert_eq!(settings.theme.as_deref(), Some("prime"));
        assert_eq!(settings.rlm_max_depth, Some(4));
    }

    #[test]
    fn removed_extensions_key_loads_gracefully() {
        let value: Value = serde_json::json!({
            "extensions": ["~/my-ext/index.ts"],
            "skills": ["~/my-skill"]
        });
        let settings = from_value_lenient(&value);
        assert_eq!(settings.skills, Some(vec!["~/my-skill".to_string()]));
        assert_eq!(
            settings.extra.get("extensions"),
            Some(&serde_json::json!(["~/my-ext/index.ts"])),
            "the dead key is preserved untouched in extra, never parsed"
        );
    }

    /// A typo in ONE sub-field must not take its valid siblings down with it:
    /// `lsp`/`graph` default ON, so dropping an explicit `enabled: false` turns
    /// the tool back on against the operator's instruction.
    #[test]
    fn a_wrong_typed_subfield_keeps_its_valid_siblings() {
        let settings = from_value_lenient(&serde_json::json!({
            "lsp": { "enabled": false, "idleTimeoutMs": "soon" }
        }));
        assert_eq!(
            settings.lsp.as_ref().and_then(|lsp| lsp.enabled),
            Some(false),
            "the explicit disable survives the typo beside it: {settings:?}"
        );

        let settings = from_value_lenient(&serde_json::json!({
            "graph": { "enabled": false, "auto": 3 }
        }));
        assert_eq!(
            settings.graph.as_ref().and_then(|graph| graph.enabled),
            Some(false),
            "the explicit graph disable survives: {settings:?}"
        );

        let settings = from_value_lenient(&serde_json::json!({
            "toolProfile": { "context": 5, "excludedTools": ["bash"] }
        }));
        assert_eq!(
            settings
                .tool_profile
                .as_ref()
                .map(|profile| profile.excluded_tools.clone()),
            Some(vec!["bash".to_string()]),
            "the tool exclusions survive the typo in the context preset: {settings:?}"
        );
    }

    /// Every value the lenient load drops is named with its dotted path: the
    /// caller prints these, so a mistyped setting never disappears silently.
    #[test]
    fn a_dropped_value_is_named_with_its_path() {
        let (settings, ignored) = from_value_lenient_reporting(&serde_json::json!({
            "lsp": { "enabled": false, "idleTimeoutMs": "soon" },
            "theme": 42,
            "graph": { "enabled": false, "auto": "watch" }
        }));
        assert_eq!(
            ignored,
            vec![
                IgnoredSetting {
                    path: "lsp.idleTimeoutMs".to_string(),
                    reason: "invalid type: string \"soon\", expected u64".to_string(),
                },
                IgnoredSetting {
                    path: "theme".to_string(),
                    reason: "invalid type: integer `42`, expected a string".to_string(),
                },
            ]
        );
        assert!(!settings.lsp.unwrap().enabled.unwrap());
        assert_eq!(settings.theme, None);
        assert_eq!(settings.graph.unwrap().enabled, Some(false));

        // A whole block of the wrong type is one diagnostic, not one per field
        // it never had.
        let (settings, ignored) = from_value_lenient_reporting(&serde_json::json!({
            "jev": "yes",
            "rlmMaxDepth": 4
        }));
        assert_eq!(
            ignored,
            vec![IgnoredSetting {
                path: "jev".to_string(),
                reason: "invalid type: string \"yes\", expected struct JevSettings".to_string(),
            }]
        );
        assert!(settings.jev.is_none());
        assert_eq!(settings.rlm_max_depth, Some(4));
    }

    /// The invariant the deleted `KNOWN_FIELDS` list tried to express, taken
    /// from the schema itself: whenever the shipped `Settings` rejects a value,
    /// the lenient load must drop it (never park it in `extra`, where a save
    /// would persist the bad key) and must report it. The key set comes from
    /// serializing the default document, so a new field is covered without
    /// touching this test - and a field the loader mishandles cannot hide.
    #[test]
    fn no_value_the_schema_rejects_rides_into_extra() {
        let keys: Vec<String> = serde_json::to_value(Settings::default())
            .unwrap()
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect();
        assert!(
            keys.len() > 60,
            "the key set is the test's input, not a copy: {} keys",
            keys.len()
        );
        let junk = [
            serde_json::json!("zzz"),
            serde_json::json!(1),
            serde_json::json!(true),
            serde_json::json!([1]),
            serde_json::json!(-1.5),
        ];
        let mut rejected = 0;
        for key in &keys {
            for value in &junk {
                let mut document = Map::new();
                document.insert(key.clone(), value.clone());
                let document = Value::Object(document);
                // The schema is the oracle: only a value it rejects is in scope.
                if serde_json::from_value::<Settings>(document.clone()).is_ok() {
                    continue;
                }
                rejected += 1;
                let (settings, ignored) = from_value_lenient_reporting(&document);
                assert!(
                    !settings.extra.contains_key(key),
                    "`{key}` = {value} must not ride into extra: {:?}",
                    settings.extra
                );
                assert_eq!(
                    ignored
                        .iter()
                        .filter(|dropped| dropped.path == *key)
                        .count(),
                    1,
                    "`{key}` = {value} is reported exactly once: {ignored:?}"
                );
            }
        }
        assert!(
            rejected > 100,
            "the sweep must actually reject values, rejected {rejected}"
        );
    }

    /// The control: an all-valid document parses exactly as before and reports
    /// nothing at all - the diagnostic list is not a false alarm per field.
    #[test]
    fn a_valid_document_parses_whole_and_reports_nothing() {
        let (settings, ignored) = from_value_lenient_reporting(&serde_json::json!({
            "theme": "prime",
            "lsp": { "enabled": false },
            "graph": { "enabled": false, "auto": "watch" },
            "unknownKey": { "kept": true }
        }));
        assert_eq!(ignored, Vec::new());
        assert_eq!(settings.theme.as_deref(), Some("prime"));
        assert_eq!(
            settings.lsp.as_ref().and_then(|lsp| lsp.enabled),
            Some(false)
        );
        let graph = settings.graph.as_ref().unwrap();
        assert_eq!(graph.enabled, Some(false));
        assert_eq!(graph.auto, Some(super::super::types::GraphAuto::Watch));
        assert_eq!(
            settings.extra.get("unknownKey"),
            Some(&serde_json::json!({ "kept": true }))
        );
    }

    /// A wrong-typed `requestTiming` behaves as unset, never a surviving raw
    /// value.
    #[test]
    fn wrong_typed_request_timing_loads_as_none() {
        let value: Value = serde_json::json!({ "requestTiming": "yes" });
        let settings = from_value_lenient(&value);
        assert_eq!(settings.request_timing, None);
        assert!(
            settings.extra.get("requestTiming").is_none(),
            "the wrong-typed known field drops out entirely: {:?}",
            settings.extra
        );
        let settings = from_value_lenient(&serde_json::json!({ "requestTiming": true }));
        assert_eq!(settings.request_timing, Some(true));
    }
}
