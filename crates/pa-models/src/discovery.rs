//! Generic live model-list discovery, driven by a YAML dictionary.
//!
//! ADR-0001 §D / the model-discovery contract (`docs/model-discovery-contract.md` §2):
//! a provider that answers a list endpoint gets its EXISTENCE from the live list, and its
//! properties from the hand-written table. One dictionary row per provider, no per-provider
//! code: adding a provider is a YAML edit, not a Rust change.
//!
//! The snapshot records what the endpoint serves *right now*. Ids the endpoint stopped
//! serving are kept in `deprecated` and never pruned: a saved `settings.json` naming the id
//! must keep resolving (the hand table stays the resolution authority for those).

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

use crate::fetch::{CatalogFetcher, FetchOutcome};

/// The dictionary, embedded so a stripped install still has it (`include_str!`, the same
/// discipline as the bundled skills).
pub const DISCOVERY_DICTIONARY: &str = include_str!("discovery.json");
pub const DISCOVERY_SCHEMA: &str = "allspark-model-discovery/v2";

/// How a provider's list request authenticates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AuthKind {
    /// `Authorization: Bearer <key>` — the provider api key.
    Bearer,
    /// No credentials on the discovery request.
    None,
}

/// One dictionary row.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProviderEntry {
    pub provider: String,
    pub list_url: String,
    pub auth: AuthKind,
    /// JSON path to the array of model ids: `data.[].id` nests then indexes.
    pub id_path: String,
    /// The provider's key inside the `models.dev` catalog, when it has one: the
    /// source of context/output/cost/thinking for ids the hand table does not
    /// describe. `None` means "no models.dev entry" — those ids stay `Estimated`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub models_dev_key: Option<String>,
}

/// The parsed dictionary.
#[derive(Debug, Clone, Default)]
pub struct DiscoveryDictionary {
    pub providers: Vec<ProviderEntry>,
}

/// A malformed dictionary is a startup defect, not a skipped provider: the entries are
/// hand-maintained contract data, exactly like `fsm.yaml`.
/// Parse and validate a discovery dictionary.
///
/// # Errors
///
/// Errors when the text is not valid JSON, the schema does not match, a row
/// misses a required key, an `auth` kind is unknown, a `list_url` is not
/// https, or a provider appears twice.
pub fn load_dictionary(text: &str) -> Result<DiscoveryDictionary, String> {
    let document: serde_json::Value =
        serde_json::from_str(text).map_err(|error| format!("discovery JSON: {error}"))?;
    let schema = document
        .get("schema")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();
    if schema != DISCOVERY_SCHEMA {
        return Err(format!(
            "discovery schema is {schema:?}, expected {DISCOVERY_SCHEMA:?}"
        ));
    }
    let mut providers = Vec::new();
    let rows = document
        .get("providers")
        .and_then(serde_json::Value::as_array)
        .ok_or("discovery: missing `providers` list")?;
    for row in rows {
        let get = |key: &str| {
            row.get(key)
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
                .ok_or_else(|| format!("discovery row missing `{key}`: {row:?}"))
        };
        let auth = match get("auth")?.as_str() {
            "bearer" => AuthKind::Bearer,
            "none" => AuthKind::None,
            other => return Err(format!("discovery: unknown auth {other:?}")),
        };
        let models_dev_key = row
            .get("models_dev_key")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string);
        providers.push(ProviderEntry {
            provider: get("provider")?,
            list_url: get("list_url")?,
            auth,
            id_path: get("id_path")?,
            models_dev_key,
        });
    }
    let mut seen = std::collections::BTreeSet::new();
    for entry in &providers {
        if !seen.insert(entry.provider.as_str()) {
            return Err(format!("discovery: duplicate provider {}", entry.provider));
        }
        if !entry.list_url.starts_with("https://") {
            return Err(format!(
                "discovery {}: list_url must be https, got {}",
                entry.provider, entry.list_url
            ));
        }
    }
    Ok(DiscoveryDictionary { providers })
}

/// Parse the dictionary embedded at build time.
///
/// # Errors
///
/// Same conditions as [`load_dictionary`]: the embedded text is compiled in,
/// so an error there is a build-time defect surfaced here.
pub fn load_embedded_dictionary() -> Result<DiscoveryDictionary, String> {
    load_dictionary(DISCOVERY_DICTIONARY)
}

/// Extract the ids at `id_path`.
///
/// # Errors
///
/// Errors when the payload does not follow `id_path`: a missing key, a `[]`
/// applied to a non-array, or a non-string leaf. A silently-empty result would
/// hide a provider, so the wrong path is an error, not an empty list.
pub fn ids_at(payload: &serde_json::Value, id_path: &str) -> Result<Vec<String>, String> {
    let mut current: Vec<&serde_json::Value> = vec![payload];
    for segment in id_path.split('.').filter(|segment| !segment.is_empty()) {
        if segment == "[]" {
            let mut next = Vec::new();
            for node in &current {
                let items = node
                    .as_array()
                    .ok_or_else(|| "id_path: [] applied to a non-array".to_string())?;
                next.extend(items.iter());
            }
            current = next;
        } else {
            let mut next = Vec::new();
            for node in &current {
                next.push(node.get(segment).ok_or_else(|| {
                    format!("id_path: key {segment:?} is missing on the payload")
                })?);
            }
            current = next;
        }
    }
    let mut ids = Vec::new();
    for node in current {
        let id = node
            .as_str()
            .ok_or_else(|| "id_path: leaf is not a string".to_string())?
            .to_string();
        if !ids.contains(&id) {
            ids.push(id);
        }
    }
    Ok(ids)
}

/// What one refresh learned about one provider: the live ids, and which of the
/// previously-served ids have disappeared.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct DiscoveredProvider {
    pub provider: String,
    pub synced_at: String,
    pub ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub deprecated: Vec<DeprecatedId>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeprecatedId {
    pub id: String,
    pub deprecated_at: String,
}

/// All providers, keyed by provider id (`BTreeMap`: stable serialization order).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[must_use]
pub struct DiscoverySnapshot {
    pub synced_at: String,
    pub providers: BTreeMap<String, DiscoveredProvider>,
}

impl DiscoverySnapshot {
    /// Merge one refresh into the snapshot, accumulating deprecations.
    pub fn merge(&mut self, discovered: DiscoveredProvider) {
        let entry = self
            .providers
            .entry(discovered.provider.clone())
            .or_default();
        for id in &entry.ids {
            if !discovered.ids.contains(id) && !entry.deprecated.iter().any(|d| &d.id == id) {
                entry.deprecated.push(DeprecatedId {
                    id: id.clone(),
                    deprecated_at: discovered.synced_at.clone(),
                });
            }
        }
        entry.synced_at = discovered.synced_at;
        entry.ids = discovered.ids;
        entry.deprecated = discovered.deprecated;
    }

    /// Whether provider `provider` serves `id` right now.
    #[must_use]
    pub fn serves(&self, provider: &str, id: &str) -> bool {
        self.providers
            .get(provider)
            .is_some_and(|entry| entry.ids.iter().any(|served| served == id))
    }

    /// Ids `provider` used to serve and stopped (oldest first).
    #[must_use]
    pub fn deprecated(&self, provider: &str) -> Vec<&str> {
        self.providers
            .get(provider)
            .map(|entry| {
                entry
                    .deprecated
                    .iter()
                    .map(|deprecated| deprecated.id.as_str())
                    .collect()
            })
            .unwrap_or_default()
    }
}

/// Model properties resolved from a `models.dev` entry (precedence 3:
/// `CatalogDelta`). Every field is optional — the entry may describe only
/// what the provider documents.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct DevProperties {
    pub context_window: Option<u64>,
    pub max_output: Option<u64>,
    pub reasoning: Option<bool>,
    pub cost_input: Option<f64>,
    pub cost_output: Option<f64>,
    pub cost_cache_read: Option<f64>,
    pub input_images: Option<bool>,
    /// The exact `reasoning_options[].values` the provider documents, when the
    /// entry carries an effort option — the authoritative thinking levels.
    pub reasoning_effort_values: Option<Vec<String>>,
}

fn as_u64(value: &serde_json::Value) -> Option<u64> {
    // A models.dev limit is a JSON number that may arrive as 2.0e5: reject a
    // negative/NaN value instead of `as`-casting (clippy::cast_possible_truncation /
    // cast_sign_loss), and saturate above u64::MAX.
    value.as_u64().or_else(|| {
        value
            .as_f64()
            .filter(|number| number.is_finite() && *number >= 0.0)
            .map(|number| {
                // 2^63 is far above any real context window; everything past it
                // saturates. `floor` cannot truncate a value this side of the bound.
                if number >= 9_223_372_036_854_775_808.0 {
                    u64::MAX
                } else {
                    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                    // bounded by the saturation branch above
                    {
                        number.floor() as u64
                    }
                }
            })
    })
}

fn as_f64(value: &serde_json::Value) -> Option<f64> {
    value.as_f64()
}

/// Read one `models.dev` model entry into properties.
#[must_use]
pub fn dev_properties(entry: &serde_json::Value) -> DevProperties {
    let mut properties = DevProperties::default();
    let limit = entry.get("limit");
    properties.context_window = limit.and_then(|limit| as_u64(&limit["context"]));
    properties.max_output = limit.and_then(|limit| as_u64(&limit["output"]));
    properties.reasoning = entry.get("reasoning").and_then(serde_json::Value::as_bool);
    let cost = entry.get("cost");
    properties.cost_input = cost.and_then(|cost| as_f64(&cost["input"]));
    properties.cost_output = cost.and_then(|cost| as_f64(&cost["output"]));
    properties.cost_cache_read = cost.and_then(|cost| as_f64(&cost["cache_read"]));
    properties.input_images = entry
        .get("modalities")
        .and_then(|modalities| modalities.get("input"))
        .and_then(serde_json::Value::as_array)
        .map(|inputs| inputs.iter().any(|input| input.as_str() == Some("image")));
    // The effort option documents the accepted `reasoning_effort` values verbatim:
    // muse-spark-1.1 lists `[minimal..xhigh]` (no `max`), measured 400 confirms it.
    properties.reasoning_effort_values = entry
        .get("reasoning_options")
        .and_then(serde_json::Value::as_array)
        .and_then(|options| {
            options.iter().find(|option| {
                option.get("type").and_then(serde_json::Value::as_str) == Some("effort")
            })
        })
        .and_then(|option| option.get("values"))
        .and_then(serde_json::Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(serde_json::Value::as_str)
                .map(str::to_string)
                .collect()
        });
    properties
}

/// The `models.dev` entry for one id under a provider key, or `None`.
#[must_use]
pub fn dev_entry<'a>(
    models_dev: &'a serde_json::Value,
    models_dev_key: &str,
    id: &str,
) -> Option<&'a serde_json::Value> {
    models_dev
        .get(models_dev_key)
        .and_then(|provider| provider.get("models"))
        .and_then(|models| models.get(id))
}

/// Fetch one provider's live list and reduce it to ids.
///
/// # Errors
///
/// Errors on transport failure, a non-JSON body, or an `id_path` that does not
/// fit the payload. A bearer provider without a supplied credential is also an
/// error: an anonymous discovery would silently under-report the list.
pub async fn discover_provider_with_headers(
    fetcher: &CatalogFetcher,
    entry: &ProviderEntry,
    extra_headers: &[(String, String)],
    synced_at: String,
) -> Result<DiscoveredProvider, String> {
    let headers: Vec<(String, String)> = extra_headers.to_vec();
    if entry.auth == AuthKind::Bearer {
        // The credential arrives as an `Authorization` header when the caller already
        // has one; `get_provider_headers` supplies it for key-auth providers.
        if !headers
            .iter()
            .any(|(name, _)| name.eq_ignore_ascii_case("authorization"))
        {
            return Err(format!(
                "{}: discovery needs an api key and none is configured",
                entry.provider
            ));
        }
    }
    let body = match fetcher
        .fetch_with(&entry.list_url, None, &headers)
        .await
        .map_err(|error| format!("{}: {error}", entry.provider))?
    {
        FetchOutcome::Fresh { body, .. } => body,
        FetchOutcome::NotModified => {
            return Err(format!(
                "{}: live list answered 304 with no cache",
                entry.provider
            ));
        }
    };
    let payload: serde_json::Value = serde_json::from_slice(&body)
        .map_err(|error| format!("{}: live list is not JSON: {error}", entry.provider))?;
    let ids =
        ids_at(&payload, &entry.id_path).map_err(|error| format!("{}: {error}", entry.provider))?;
    Ok(DiscoveredProvider {
        provider: entry.provider.clone(),
        synced_at,
        ids,
        deprecated: Vec::new(),
    })
}

/// The snapshot file lives beside the other model caches (`models/`).
#[must_use]
pub fn snapshot_path(models_dir: &Path) -> std::path::PathBuf {
    models_dir.join("discovery-snapshot.v1.json")
}

/// Read the discovery snapshot, when one has been written.
#[must_use]
pub fn read_snapshot(models_dir: &Path) -> Option<DiscoverySnapshot> {
    let text = std::fs::read_to_string(snapshot_path(models_dir)).ok()?;
    serde_json::from_str(&text).ok()
}

/// Write the discovery snapshot.
///
/// # Errors
///
/// Errors when `models_dir` cannot be created or the snapshot cannot be
/// written; the caller treats both as best-effort (the hand table keeps
/// deciding offline).
pub fn write_snapshot(models_dir: &Path, snapshot: &DiscoverySnapshot) -> std::io::Result<()> {
    std::fs::create_dir_all(models_dir)?;
    let json = serde_json::to_string_pretty(snapshot)?;
    std::fs::write(snapshot_path(models_dir), json)
}
