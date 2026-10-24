//! Live metadata for the `OpenCode` free tier.
//!
//! `opencode` serves its models but not their capabilities: `GET
//! /inference/v1/models` answers with `{id, object, created, owned_by}` and
//! nothing else (verified 2026-10-23; no per-model endpoint, no `?detailed`,
//! no catalog route). So existence and capabilities come from different places:
//!
//! - **existence** — the live list. It is the only source that knows which ids
//!   are still served, and it needs no credential (`Bearer public`, in fact the
//!   plain request works too). A deprecated id disappears from it.
//! - **capabilities** — `models.dev/api.json` (the `opencode` entry), the only
//!   source with context/output limits, modalities and reasoning flags. It is
//!   third-party and lags: of its 44 zero-cost opencode ids, 37 are no longer
//!   in the live list. It also invents nothing, so a disagreement with a
//!   hand-written table is a correction, not a conflict.
//!
//! The shape follows `9router` (`src/lib/modelCatalog/sync.js`, MIT): a daily
//! conditional fetch (`If-None-Match`; models.dev answers 304 with an empty
//! body), and a stored **delta** rather than a copy — the hand-written table in
//! `pa_ai::models_fork` stays authoritative until upstream disagrees by more
//! than [`LIMIT_TOLERANCE`]. Keeping the copy would silently adopt 37 dead ids.
//!
//! Free-ness is decided by the live list, not by an id suffix: `big-pickle` is
//! served free without one. The ids the live list adds but `models.dev` does
//! not describe carry `metadata: Estimated` so a caller can tell a measurement
//! from a guess.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::fetch::{CatalogFetcher, FetchError, FetchOutcome};

/// The live model list. No credential is required.
pub const OPENCODE_MODELS_URL: &str = "https://opencode.ai/inference/v1/models";
/// The capability catalog (third-party; `opencode` entry).
pub const MODELS_DEV_URL: &str = "https://models.dev/api.json";
/// One refresh per day: the list changes on the order of days, and the body is
/// ~5.7 MB when it changes at all.
pub const REFRESH_INTERVAL: Duration = Duration::from_hours(24);
/// Gateways round the same window differently (200000 vs 202752), so a smaller
/// relative gap is noise, not a correction.
pub const LIMIT_TOLERANCE: f64 = 0.1;
/// The provider this module describes.
pub const PROVIDER: &str = "opencode";

/// Where a capability came from, so a caller never mistakes a guess for a
/// measurement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MetadataSource {
    /// Read from `models.dev` for this exact id.
    Measured,
    /// Inferred from the id's family, because `models.dev` does not list it.
    Estimated,
}

/// What one live id is known to be.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LiveModel {
    /// The id exactly as the endpoint serves it.
    pub id: String,
    /// Context window in tokens, when known.
    pub context_window: Option<u64>,
    /// Max output tokens, when known.
    pub max_output: Option<u64>,
    /// Whether the model accepts image input.
    pub images: bool,
    /// Whether the model supports reasoning effort.
    pub reasoning: bool,
    /// How the capabilities above were obtained.
    pub metadata: MetadataSource,
}

/// One id the live list USED to serve and stopped: kept, named, dated.
///
/// The whole point of this record is that retirement is not deletion. A saved
/// `settings.json` naming the id must keep resolving, `model list` must show it
/// with the date it disappeared, and nothing may silently prune it (the
/// discovery contract, `docs/model-discovery-contract.md` §6).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeprecatedModel {
    /// The id exactly as the endpoint used to serve it.
    pub id: String,
    /// ISO-8601 of the refresh that first noticed the id was gone.
    pub deprecated_at: String,
}

/// The persisted snapshot: the live list, plus the capability deltas that
/// differed from the hand-written table, plus the ids it stopped serving.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct OpenCodeSnapshot {
    /// The `ETag` of the `models.dev` body this snapshot was built from; sent
    /// back as `If-None-Match` so an unchanged catalog costs 0 bytes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub models_dev_etag: Option<String>,
    /// ISO-8601 of the last successful refresh.
    #[serde(default)]
    pub synced_at: String,
    /// Every id the live list serves, with what is known about it.
    pub models: Vec<LiveModel>,
    /// Ids the live list stopped serving, oldest disappearance first. Accumulates
    /// across refreshes and never shrinks: the hand-written table stays the
    /// resolution authority for exactly these ids.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub deprecated: Vec<DeprecatedModel>,
}

impl OpenCodeSnapshot {
    /// The id exists on the live endpoint right now.
    #[must_use]
    pub fn serves(&self, id: &str) -> bool {
        self.models.iter().any(|model| model.id == id)
    }

    /// The ids the live endpoint serves, in its own order.
    #[must_use]
    pub fn ids(&self) -> Vec<&str> {
        self.models.iter().map(|model| model.id.as_str()).collect()
    }

    /// The id is a recorded retirement: not served now, served before, kept
    /// for resolution and display.
    #[must_use]
    pub fn is_deprecated(&self, id: &str) -> bool {
        self.deprecated.iter().any(|model| model.id == id)
    }

    /// Fold a refresh's live set into the accumulated retirement list: every id
    /// the previous snapshot carried and this one does not gains a dated entry;
    /// an id the endpoint serves AGAIN leaves the list (a returned model is not
    /// deprecated, and a second disappearance is dated again).
    #[must_use]
    pub fn with_deprecations(mut self, previous: Option<&Self>, now: &str) -> Self {
        let served_now: std::collections::HashSet<&str> =
            self.models.iter().map(|model| model.id.as_str()).collect();
        if let Some(previous) = previous {
            for model in &previous.models {
                if served_now.contains(model.id.as_str())
                    || self.deprecated.iter().any(|gone| gone.id == model.id)
                {
                    continue;
                }
                self.deprecated.push(DeprecatedModel {
                    id: model.id.clone(),
                    deprecated_at: now.to_string(),
                });
            }
        }
        self.deprecated
            .sort_by(|a, b| a.deprecated_at.cmp(&b.deprecated_at).then(a.id.cmp(&b.id)));
        self
    }
}

/// `models.dev` entry, trimmed to the fields this module reads. `limit` and
/// `modalities` are optional so one malformed provider entry cannot make the
/// whole fetch useless.
#[derive(Debug, Deserialize)]
struct ModelsDevModel {
    #[serde(default)]
    limit: Option<ModelsDevLimit>,
    #[serde(default)]
    modalities: Option<ModelsDevModalities>,
    #[serde(default)]
    reasoning: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct ModelsDevLimit {
    #[serde(default)]
    context: Option<u64>,
    #[serde(default)]
    output: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct ModelsDevModalities {
    #[serde(default)]
    input: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct ModelsDevProvider {
    #[serde(default)]
    models: BTreeMap<String, ModelsDevModel>,
}

/// What the hand-written table already claims for one id, so a refresh can
/// store a delta instead of a copy.
#[derive(Debug, Clone, Copy)]
pub struct KnownCapabilities {
    /// Context window the table asserts.
    pub context_window: u64,
    /// Max output tokens the table asserts.
    pub max_output: u64,
}

/// One parsed live entry.
#[derive(Debug, Deserialize)]
struct LiveEntry {
    id: String,
}

#[derive(Debug, Deserialize)]
struct LiveResponse {
    data: Vec<LiveEntry>,
}

fn relative_gap(current: u64, upstream: u64) -> f64 {
    if current == 0 {
        return f64::INFINITY;
    }
    let gap = upstream.abs_diff(current);
    // Token counts are far below 2^52, so the cast is exact for every value that
    // can arrive here; the saturating conversion only guards a nonsense input.
    let gap = f64::from(u32::try_from(gap).unwrap_or(u32::MAX));
    let current = f64::from(u32::try_from(current).unwrap_or(u32::MAX));
    gap / current
}

/// Build a snapshot from the two bodies.
///
/// `known` supplies the hand-written table's numbers: a value is adopted only
/// when it differs from the table by more than [`LIMIT_TOLERANCE`], so an
/// upstream rounding difference never rewrites a calibrated constant.
#[must_use]
pub fn build_snapshot(
    live: &[u8],
    models_dev: Option<&[u8]>,
    known: &(dyn Fn(&str) -> Option<KnownCapabilities> + Send + Sync),
    models_dev_etag: Option<String>,
    synced_at: String,
) -> Option<OpenCodeSnapshot> {
    let live: LiveResponse = serde_json::from_slice(live).ok()?;
    let dev = models_dev
        .and_then(|body| serde_json::from_slice::<BTreeMap<String, ModelsDevProvider>>(body).ok())
        .and_then(|mut providers| providers.remove(PROVIDER));

    let mut models = Vec::with_capacity(live.data.len());
    for entry in live.data {
        let upstream = dev
            .as_ref()
            .and_then(|provider| provider.models.get(&entry.id));
        let measured = upstream.and_then(|model| {
            let limit = model.limit.as_ref()?;
            let context = limit.context.filter(|context| *context > 0)?;
            Some((
                context,
                limit.output.filter(|output| *output > 0),
                model
                    .modalities
                    .as_ref()
                    .is_some_and(|modalities| modalities.input.iter().any(|kind| kind == "image")),
                model.reasoning.unwrap_or(false),
            ))
        });

        let (context_window, max_output, images, reasoning, metadata) = match measured {
            Some((context, output, images, reasoning)) => {
                let (context_used, output_used) = match known(&entry.id) {
                    Some(table) => {
                        let context_used =
                            if relative_gap(table.context_window, context) > LIMIT_TOLERANCE {
                                Some(context)
                            } else {
                                None
                            };
                        let output_used = match output {
                            Some(output)
                                if relative_gap(table.max_output, output) > LIMIT_TOLERANCE =>
                            {
                                Some(output)
                            }
                            _ => None,
                        };
                        (context_used, output_used)
                    }
                    None => (Some(context), output),
                };
                (
                    context_used,
                    output_used,
                    images,
                    reasoning,
                    MetadataSource::Measured,
                )
            }
            // The live endpoint serves it and models.dev does not describe it:
            // keep the id, mark the capabilities as a guess rather than dropping
            // a model the endpoint will answer for.
            None => (None, None, false, false, MetadataSource::Estimated),
        };

        models.push(LiveModel {
            id: entry.id,
            context_window,
            max_output,
            images,
            reasoning,
            metadata,
        });
    }

    Some(OpenCodeSnapshot {
        models_dev_etag,
        synced_at,
        models,
        // Filled by the caller (`refresh`), which owns the previous snapshot the
        // retirement list accumulates from.
        deprecated: Vec::new(),
    })
}

/// Where the snapshot lives: beside `models.json`, like the provider catalog.
#[must_use]
pub fn snapshot_path(models_dir: &Path) -> PathBuf {
    models_dir.join("opencode-models.v1.json")
}

/// Read the cached snapshot; `None` when it is absent or unreadable (a corrupt
/// file must not stop the session, only leave the table deciding).
#[must_use]
pub fn read_snapshot(models_dir: &Path) -> Option<OpenCodeSnapshot> {
    let body = std::fs::read(snapshot_path(models_dir)).ok()?;
    serde_json::from_slice(&body).ok()
}

/// Write the snapshot atomically: a reader never sees a half-written file.
///
/// # Errors
///
/// Propagates the write or rename failure.
pub fn write_snapshot(models_dir: &Path, snapshot: &OpenCodeSnapshot) -> std::io::Result<()> {
    let path = snapshot_path(models_dir);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let body = serde_json::to_vec_pretty(snapshot).unwrap_or_default();
    let temporary = path.with_extension("json.tmp");
    std::fs::write(&temporary, body)?;
    std::fs::rename(&temporary, &path)
}

/// One refresh: fetch the live list and (conditionally) the capability catalog,
/// then build and persist the snapshot.
///
/// Failures are reported, never fatal: a stale or missing snapshot leaves the
/// hand-written table deciding, which is the offline contract.
///
/// # Errors
///
/// Returns the live-list fetch failure. A `models.dev` failure is not an error:
/// the snapshot is then built with estimated capabilities only.
pub async fn refresh(
    fetcher: &CatalogFetcher,
    models_dir: &Path,
    known: &(dyn Fn(&str) -> Option<KnownCapabilities> + Send + Sync),
    synced_at: String,
) -> Result<OpenCodeSnapshot, FetchError> {
    let live = match fetcher.fetch(OPENCODE_MODELS_URL, None).await? {
        FetchOutcome::Fresh { body, .. } => body,
        // The live list is not conditional; a 304 here would mean a broken
        // server, so treat it as "nothing to do".
        FetchOutcome::NotModified => {
            return read_snapshot(models_dir).ok_or(FetchError::Transport {
                message: "opencode model list answered 304 with no cached snapshot".to_string(),
            });
        }
    };

    let previous = read_snapshot(models_dir);
    let etag = previous
        .as_ref()
        .and_then(|snapshot| snapshot.models_dev_etag.clone());
    let (dev_body, dev_etag) = match fetcher.fetch(MODELS_DEV_URL, etag.as_deref()).await {
        Ok(FetchOutcome::Fresh { body, etag }) => (Some(body), etag),
        // Unchanged: reuse the last snapshot's capabilities by rebuilding from
        // the live list alone would lose them, so keep the previous models when
        // the live set did not change.
        Ok(FetchOutcome::NotModified) => {
            if let Some(ref previous) = previous {
                if previous.models.len()
                    == serde_json::from_slice::<LiveResponse>(&live)
                        .map_or(0, |response| response.data.len())
                {
                    let snapshot = OpenCodeSnapshot {
                        models_dev_etag: etag,
                        synced_at: synced_at.clone(),
                        models: previous.models.clone(),
                        deprecated: previous.deprecated.clone(),
                    };
                    let _ = write_snapshot(models_dir, &snapshot);
                    return Ok(snapshot);
                }
            }
            (None, etag)
        }
        // A models.dev outage must not fail the refresh: existence is still
        // known, capabilities fall back to estimated.
        Err(_) => (None, etag),
    };

    let Some(mut snapshot) = build_snapshot(&live, dev_body.as_deref(), known, dev_etag, synced_at)
    else {
        return Err(FetchError::Transport {
            message: "opencode model list is not a JSON model list".to_string(),
        });
    };
    // Retirements accumulate from the previous snapshot BEFORE the new one
    // overwrites it on disk: a disappearance is only knowable by comparing the
    // two, and the dated record must survive restarts (`read_snapshot`).
    let now = snapshot.synced_at.clone();
    snapshot = snapshot.with_deprecations(previous.as_ref(), &now);
    let _ = write_snapshot(models_dir, &snapshot);
    Ok(snapshot)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn live_body(ids: &[&str]) -> Vec<u8> {
        let data: Vec<serde_json::Value> = ids
            .iter()
            .map(|id| {
                serde_json::json!({
                    "id": id,
                    "object": "model",
                    "created": 1,
                    "owned_by": "opencode"
                })
            })
            .collect();
        serde_json::to_vec(&serde_json::json!({"object": "list", "data": data})).unwrap()
    }

    fn dev_body(entries: &[(&str, u64, u64, bool)]) -> Vec<u8> {
        let mut models = serde_json::Map::new();
        for (id, context, output, images) in entries {
            let input = if *images {
                vec!["text", "image"]
            } else {
                vec!["text"]
            };
            models.insert(
                (*id).to_string(),
                serde_json::json!({
                    "id": id,
                    "limit": {"context": context, "output": output},
                    "modalities": {"input": input},
                    "reasoning": true
                }),
            );
        }
        serde_json::to_vec(&serde_json::json!({"opencode": {"id": "opencode", "models": models}}))
            .unwrap()
    }

    fn no_table(_id: &str) -> Option<KnownCapabilities> {
        None
    }

    /// The live list decides existence: an id models.dev still advertises but the
    /// endpoint no longer serves is simply absent, which is how the dead
    /// zero-cost entries stop being offered.
    #[test]
    fn only_the_live_list_decides_which_ids_exist() {
        let live = live_body(&["served-free", "big-pickle"]);
        let dev = dev_body(&[
            ("served-free", 200_000, 32_000, true),
            ("dead-free", 100, 100, false),
        ]);
        let snapshot = build_snapshot(&live, Some(&dev), &no_table, None, "now".into()).unwrap();
        assert_eq!(snapshot.ids(), vec!["served-free", "big-pickle"]);
        assert!(
            !snapshot.serves("dead-free"),
            "a models.dev-only id must not appear"
        );
    }

    /// Capabilities come from models.dev; an id it does not describe keeps the id
    /// and is marked estimated rather than dropped.
    #[test]
    fn an_undescribed_live_id_is_kept_and_marked_estimated() {
        let live = live_body(&["measured-free", "big-pickle"]);
        let dev = dev_body(&[("measured-free", 200_000, 32_000, true)]);
        let snapshot = build_snapshot(&live, Some(&dev), &no_table, None, "now".into()).unwrap();

        let measured = snapshot
            .models
            .iter()
            .find(|model| model.id == "measured-free")
            .unwrap();
        assert_eq!(measured.context_window, Some(200_000));
        assert_eq!(measured.max_output, Some(32_000));
        assert!(measured.images);
        assert_eq!(measured.metadata, MetadataSource::Measured);

        let guessed = snapshot
            .models
            .iter()
            .find(|model| model.id == "big-pickle")
            .unwrap();
        assert_eq!(guessed.metadata, MetadataSource::Estimated);
    }

    /// A delta, not a copy: a table value within tolerance stays, so upstream
    /// rounding never rewrites a calibrated constant; a larger gap is adopted.
    #[test]
    fn a_capability_within_tolerance_keeps_the_table_value() {
        let live = live_body(&["glm-free"]);
        let table = |_id: &str| {
            Some(KnownCapabilities {
                context_window: 200_000,
                max_output: 32_000,
            })
        };

        let dev = dev_body(&[("glm-free", 202_752, 31_000, false)]);
        let snapshot = build_snapshot(&live, Some(&dev), &table, None, "now".into()).unwrap();
        let model = &snapshot.models[0];
        assert_eq!(
            model.context_window, None,
            "a 1.4% gateway rounding must not be recorded as a correction"
        );
        assert_eq!(model.max_output, None);

        let dev = dev_body(&[("glm-free", 1_000_000, 32_000, false)]);
        let snapshot = build_snapshot(&live, Some(&dev), &table, None, "now".into()).unwrap();
        assert_eq!(
            snapshot.models[0].context_window,
            Some(1_000_000),
            "a five-fold difference is a correction, not noise"
        );
    }

    /// A models.dev outage must not lose the live list: every served id survives,
    /// with estimated capabilities.
    #[test]
    fn a_missing_capability_catalog_still_yields_the_live_ids() {
        let live = live_body(&["a-free", "b-free"]);
        let snapshot = build_snapshot(&live, None, &no_table, None, "now".into()).unwrap();
        assert_eq!(snapshot.ids(), vec!["a-free", "b-free"]);
        assert!(snapshot
            .models
            .iter()
            .all(|model| model.metadata == MetadataSource::Estimated));
    }

    /// A body that is not a model list is refused rather than written as an empty
    /// snapshot, which would erase a good cache.
    #[test]
    fn a_malformed_live_body_is_refused() {
        assert!(build_snapshot(b"{", None, &no_table, None, "now".into()).is_none());
        assert!(build_snapshot(br#"{"oops": 1}"#, None, &no_table, None, "now".into()).is_none());
    }

    /// The snapshot round-trips through the cache file, so a restart does not
    /// depend on the network.
    #[test]
    fn the_snapshot_round_trips_through_the_cache_file() {
        let dir = tempfile::TempDir::new().unwrap();
        let live = live_body(&["x-free"]);
        let dev = dev_body(&[("x-free", 200_000, 32_000, true)]);
        let snapshot = build_snapshot(
            &live,
            Some(&dev),
            &no_table,
            Some("etag-1".into()),
            "now".into(),
        )
        .unwrap();
        write_snapshot(dir.path(), &snapshot).unwrap();
        let read = read_snapshot(dir.path()).expect("cached snapshot");
        assert_eq!(read, snapshot);
        assert_eq!(read.models_dev_etag.as_deref(), Some("etag-1"));
    }

    /// The discovery contract (§6): retirement is not deletion. A model the
    /// endpoint stopped serving is recorded with the date it disappeared, the
    /// record survives the next refresh, and a model that comes BACK leaves the
    /// list (a returned model is served, not retired; a second disappearance is
    /// dated again).
    #[test]
    fn retirements_accumulate_and_a_returning_id_leaves_the_list() {
        let first = build_snapshot(
            &live_body(&["alpha", "beta"]),
            None,
            &no_table,
            None,
            "2026-10-01".into(),
        )
        .unwrap();
        assert_eq!(first.deprecated.len(), 0, "nothing disappeared yet");

        // beta disappears.
        let second = build_snapshot(
            &live_body(&["alpha"]),
            None,
            &no_table,
            None,
            "2026-10-02".into(),
        )
        .unwrap()
        .with_deprecations(Some(&first), "2026-10-02");
        assert_eq!(second.deprecated.len(), 1);
        assert_eq!(second.deprecated[0].id, "beta");
        assert_eq!(second.deprecated[0].deprecated_at, "2026-10-02");
        assert!(second.is_deprecated("beta"));

        // beta returns: served again, not retired.
        let third = build_snapshot(
            &live_body(&["alpha", "beta"]),
            None,
            &no_table,
            None,
            "2026-10-03".into(),
        )
        .unwrap()
        .with_deprecations(Some(&second), "2026-10-03");
        assert!(
            !third.is_deprecated("beta"),
            "a model the endpoint serves again is served, not retired"
        );
        assert_eq!(third.deprecated.len(), 0);

        // beta disappears again: the record is dated with the NEW disappearance.
        let fourth = build_snapshot(
            &live_body(&["alpha"]),
            None,
            &no_table,
            None,
            "2026-10-04".into(),
        )
        .unwrap()
        .with_deprecations(Some(&third), "2026-10-04");
        assert_eq!(fourth.deprecated.len(), 1);
        assert_eq!(fourth.deprecated[0].deprecated_at, "2026-10-04");
    }

    /// An unreadable cache reads as absent, never a panic: a corrupt file must not
    /// stop a session.
    #[test]
    fn a_corrupt_cache_reads_as_absent() {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::write(snapshot_path(dir.path()), b"not json").unwrap();
        assert!(read_snapshot(dir.path()).is_none());
    }
}
