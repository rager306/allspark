//! The typed-decision (judgment) layer: FORK-ONLY, not in upstream. See
//! `.research/adr-pa-jev-judge-layer.md`.
//!
//! One [`Judge`] trait answers typed questions (choice / score / noul) about
//! a state. The wire vocabulary, dialects, and the HTTP client live in the
//! `pa-jev` leaf crate; this module owns everything above the wire:
//!
//! - [`ChainJudge`]: the candidate ladder - cooldown of rejected accounts
//!   (401/402/403, 5 minutes, keyed by candidate identity), no-key skips,
//!   abort/timeout rethrow without fallback, and the native-only rule
//!   (after the first native candidate, prompted candidates never run).
//! - [`AnswerCache`]: the answer cache (Phase 1: in-memory, same interface
//!   the ADR's SQLite backend will keep).
//! - [`NativeJudge`]: one `pa-jev` client behind the cache, as a [`Judge`].
//! - [`JevTool`]: the agent-facing `jev` tool (ask / classify / route /
//!   check).
//! - [`resolve_judge`] + [`should_install_jev_tool`]: settings- and
//!   registry-driven construction, default OFF.

pub mod auto_thinking;
pub mod cache;
pub mod chain;
// FORK-ONLY (Phase 2): the offline corpus gate every consumer must pass
// before it may be enabled by default.
pub mod eval;
pub mod graph_rerank;
pub mod native;
pub mod tool;
// FORK-ONLY (Phase 2): the tool-selection arbiter (bash -> graph/lsp/edit
// routing hints; advisory only, never a redirect).
pub mod tool_arbiter;

#[cfg(test)]
mod tests;

pub use auto_thinking::{Difficulty, DifficultyClassifier};
pub use cache::{AnswerCache, CachedAnswers};
pub use chain::{ChainJudge, JudgeCandidate, JudgeKind, CANDIDATE_REJECTION_COOLDOWN};
pub use graph_rerank::{GraphReranker, RerankCandidate, RerankOutcome};
pub use native::NativeJudge;
pub use tool::{params_schema as jev_tool_params_schema, JevTool};
pub use tool_arbiter::{ToolArbiter, ToolAvailability};

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use pa_jev::JevClient;

use crate::models::ModelRegistry;
use crate::settings::JevEnabledSetting;

/// The pinned default judge model (the ADR pins a version, never
/// `jev-latest`: thresholds calibrate per release).
pub const DEFAULT_JEV_MODEL: &str = "typesafe/jev-1.13.0";
/// `TypeSafe`'s public API root (the official SDK's default).
pub const TYPESAFE_DEFAULT_BASE_URL: &str = "https://api.typesafe.ai";
/// The official SDK's env override for the base URL.
pub const TYPESAFE_BASE_URL_ENV: &str = "TYPESAFE_BASE_URL";
/// The official SDK's env override for the API key (resolved through the
/// auth registry like any provider credential).
pub const TYPESAFE_API_KEY_ENV: &str = "TYPESAFE_API_KEY";

/// A judgment failure as the chain and consumers see it.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum JudgeError {
    /// The judgment was cancelled; rethrows without spending the next
    /// candidate.
    #[error("judgment aborted")]
    Aborted,
    /// Every attempt timed out; rethrows without spending the next
    /// candidate (a slow endpoint is not a candidate-quality signal).
    #[error("judgment timed out after {attempts} attempts")]
    Timeout { attempts: u32 },
    /// One candidate failed (the chain walks to the next one).
    #[error("judge candidate {identity} failed: {message}")]
    Candidate {
        identity: String,
        message: String,
        status: Option<u16>,
    },
    /// Every candidate failed or was unavailable.
    #[error(
        "judgment: every judge candidate failed: {}",
        last_failure.as_deref().or(last_unavailable.as_deref()).unwrap_or("no candidate available")
    )]
    AllCandidatesFailed {
        last_failure: Option<String>,
        last_unavailable: Option<String>,
    },
}

impl JudgeError {
    /// Whether the account rejected the request outright (401/402/403).
    #[must_use]
    pub fn is_account_rejection(&self) -> bool {
        matches!(
            self,
            JudgeError::Candidate {
                status: Some(401..=403),
                ..
            }
        )
    }
}

/// Answers typed questions about a state. Implementations are stateless and
/// safe to share.
///
/// `async_trait` (not RPITIT) because every consumer holds `Arc<dyn Judge>`
/// - the chain, the tool, and the Phase-2 consumers all erase the backend.
#[async_trait::async_trait]
pub trait Judge: Send + Sync {
    /// Backend description for logs, e.g. `typesafe/jev-1.13.0`.
    fn label(&self) -> &str;
    /// Answer `request`'s questions.
    ///
    /// # Errors
    ///
    /// Every failure is [`JudgeError`]; aborts and timeouts rethrow at the
    /// chain without fallback.
    async fn judge(
        &self,
        request: &pa_jev::JudgmentRequest,
    ) -> Result<pa_jev::JudgmentResult, JudgeError>;
}

/// The `[jev]` settings snapshot the resolver needs, captured before the
/// settings manager moves into the resource loader.
#[derive(Debug, Clone)]
pub struct JevResolvedSettings {
    pub enabled: JevEnabledSetting,
    /// The pinned primary model (`provider/model-id`).
    pub model: String,
    /// Fallback candidates in attempt order.
    pub fallback_chain: Vec<String>,
    pub timeout: Duration,
    pub cache_enabled: bool,
}

/// Build the judge chain for `[jev] model` + `fallbackChain`.
///
/// Each candidate resolves through the model registry (catalog or
/// `models.json` entry: its `api` picks the wire dialect, its `baseUrl` the
/// endpoint). A bare `typesafe/*` model with no catalog entry synthesizes
/// the default `TypeSafe` endpoint (`TYPESAFE_BASE_URL` /
/// `https://api.typesafe.ai`). Candidates without a credential stay in the
/// chain as skipped (the chain records them unavailable), so a key added
/// later needs only a session restart - not a settings change.
///
/// Returns `None` when no candidate id resolves to a judgment model at all.
#[must_use]
pub fn resolve_judge(
    settings: &JevResolvedSettings,
    registry: &mut ModelRegistry,
) -> Option<Arc<ChainJudge>> {
    let mut ids = vec![settings.model.trim().to_string()];
    ids.extend(
        settings
            .fallback_chain
            .iter()
            .map(|id| id.trim().to_string()),
    );
    ids.retain(|id| !id.is_empty());
    ids.dedup();
    let cache = settings
        .cache_enabled
        .then(|| Arc::new(AnswerCache::default()));
    let mut candidates = Vec::new();
    for id in ids {
        if let Some(candidate) = resolve_candidate(&id, settings, registry, cache.as_ref()) {
            candidates.push(candidate);
        }
    }
    // The process-shared ledger: a re-resolved chain (next session, next
    // consumer) inherits another chain's cooldowns, like the omp
    // registry-carried rejection map.
    (!candidates.is_empty()).then(|| Arc::new(ChainJudge::new_shared_rejections(candidates)))
}

/// Whether the chain's first candidate is a native judgment backend - the
/// `jev.enabled = "auto"` gate (judge-heavy consumers stay deterministic
/// until a calibrated judge actually resolves).
#[must_use]
pub fn has_native_judge(settings: &JevResolvedSettings, registry: &mut ModelRegistry) -> bool {
    resolve_judge(settings, registry)
        .as_deref()
        .is_some_and(ChainJudge::starts_native)
}

/// The create-session gate: whether the `jev` tool joins the roster.
///
/// - `off`: never (the default - a paid third-party endpoint).
/// - `on`: whenever a judge chain resolved.
/// - `auto`: only when the chain starts native (no prompted stand-in: an
///   uncalibrated model would silently break the 0.7/0.9 thresholds).
#[must_use]
pub fn should_install_jev_tool(enabled: JevEnabledSetting, chain: Option<&ChainJudge>) -> bool {
    match enabled {
        JevEnabledSetting::Off => false,
        JevEnabledSetting::On => chain.is_some(),
        JevEnabledSetting::Auto => chain.is_some_and(ChainJudge::starts_native),
    }
}

/// Resolve one `provider/model-id` string into a chain candidate.
fn resolve_candidate(
    id: &str,
    settings: &JevResolvedSettings,
    registry: &mut ModelRegistry,
    cache: Option<&Arc<AnswerCache>>,
) -> Option<JudgeCandidate> {
    let (provider, model_id) = id.split_once('/').unwrap_or(("typesafe", id));
    let catalog_model = registry
        .get_all()
        .iter()
        .find(|model| model.provider == provider && model.id == model_id)
        .cloned();
    // The endpoint's own input price, when the catalog knows it: the chain can
    // hold TypeSafe (0.042/MTok) and SubQ Snap (0.05/MTok) at once, so pricing
    // is read per candidate, never from one chain-wide constant.
    let catalog_input_price = catalog_model
        .as_ref()
        .map(|model| model.cost.input.as_f64())
        .filter(|price| *price > 0.0);
    let (dialect, base_url, wire_model, api_key, headers, identity) = match catalog_model {
        Some(model) => {
            // Classification is always by wire dialect, never by provider.
            let dialect = pa_jev::WireDialect::from_api(&model.api)?;
            let auth = registry.get_api_key_and_headers(&model, None);
            (
                dialect,
                model.base_url.trim_end_matches('/').to_string(),
                model.id.clone(),
                auth.api_key,
                auth.headers.unwrap_or_default(),
                format!("{}/{}", model.provider, model.id),
            )
        }
        None if provider == "typesafe" => {
            // No catalog entry: the official SDK's default endpoint, with
            // its env overrides.
            let base = std::env::var(TYPESAFE_BASE_URL_ENV)
                .ok()
                .map(|value| value.trim().trim_end_matches('/').to_string())
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| TYPESAFE_DEFAULT_BASE_URL.to_string());
            let api_key = std::env::var(TYPESAFE_API_KEY_ENV)
                .ok()
                .filter(|key| !key.is_empty())
                .or_else(|| registry.auth.get_api_key("typesafe"));
            (
                pa_jev::WireDialect::Typesafe,
                base,
                model_id.to_string(),
                api_key,
                BTreeMap::default(),
                id.to_string(),
            )
        }
        None => return None,
    };
    let judge = api_key.map(|api_key| {
        let mut config = pa_jev::JevClientConfig::new(
            dialect,
            base_url,
            wire_model,
            provider.to_string(),
            api_key,
        );
        config.timeout = settings.timeout;
        config.extra_headers = headers;
        // The endpoint's question cap: `SubQ Snap` rejects a 17-question request
        // with 422, so a chain that can reach it must split larger batches.
        // An unknown provider keeps `None` (send whole), which is what every
        // other endpoint in the chain accepts.
        config.max_questions = pa_ai::models_fork::judgment_question_cap(provider);
        // Price the tokens at THIS endpoint's rate: the chain mixes TypeSafe
        // (0.042/MTok) with SubQ Snap (0.05/MTok), so a single chain-wide
        // constant would bill one provider's tokens at another's price.
        // `None` keeps the no-catalog default for the `typesafe` path.
        if let Some(price) = catalog_input_price {
            config.input_price_per_mtok = price;
        }
        Arc::new(NativeJudge::new(
            JevClient::new(config),
            identity.clone(),
            cache.cloned(),
        )) as Arc<dyn Judge>
    });
    Some(JudgeCandidate {
        identity,
        kind: JudgeKind::Native,
        judge,
    })
}
