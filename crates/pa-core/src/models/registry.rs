//! `ModelRegistry`: composes built-in, custom (models.json), and Prime Inference
//! catalogs; resolves request auth per provider/model.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use pa_types::ai::{
    CompatKind, Model, ModelCompat, ModelThinkingLevel, OpenAiResponsesCompat, ThinkingLevelMap,
};

use crate::auth::manager::AuthStorage;
use crate::auth::types::{AuthCredential, PRIME_INFERENCE_PROVIDER_ID};

use crate::packages::is_offline_mode_enabled;

use super::catalog_chain;
use super::custom::{apply_model_override, load_custom_models, merge_compat, CustomModelsResult};
use super::prime_inference::is_private_prime_inference_model;

/// Request-auth bits a provider can configure in models.json.
#[derive(Debug, Clone, Default)]
pub struct ProviderRequestConfig {
    pub api_key: Option<String>,
    /// Ordered (`BTreeMap`): merged into request-header maps that
    /// providers iterate deterministically.
    pub headers: Option<BTreeMap<String, String>>,
    pub auth_header: Option<bool>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ResolvedRequestAuth {
    pub ok: bool,
    pub api_key: Option<String>,
    /// Ordered (`BTreeMap`): unordered iteration would order the
    /// composed request headers randomly.
    pub headers: Option<BTreeMap<String, String>>,
    pub error: Option<String>,
    /// The per-call request model (TS `requestModel`): for xAI, the model
    /// shaped for the credential the lookup serves — the stored
    /// subscription's openai-responses shape or the configured API-key
    /// shape. `None` keeps the caller's model.
    pub request_model: Option<Model>,
}

/// Why a `set_model` selection failed to resolve: the provider is not
/// signed in (the client offers the sign-in flow and retries) or the model
/// is not in the catalog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SetModelSelectionError {
    /// The model exists, but its provider has no credential and none is
    /// stale: sign in, then retry the switch.
    ProviderUnauthenticated { provider: String },
    /// No such model in the catalog.
    NotFound { provider: String, model_id: String },
}

impl SetModelSelectionError {
    /// The refused provider of the sign-in variant; `None` on not-found.
    #[must_use]
    pub fn unauthenticated_provider(&self) -> Option<&str> {
        match self {
            SetModelSelectionError::ProviderUnauthenticated { provider } => Some(provider),
            SetModelSelectionError::NotFound { .. } => None,
        }
    }
}

impl std::fmt::Display for SetModelSelectionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SetModelSelectionError::ProviderUnauthenticated { provider } => write!(
                f,
                "Provider \"{provider}\" is not signed in. Sign in to the provider (the TUI's /login command), then set the model again."
            ),
            SetModelSelectionError::NotFound { provider, model_id } => {
                write!(f, "Model not found: {provider}/{model_id}")
            }
        }
    }
}

impl std::error::Error for SetModelSelectionError {}

/// Composed model catalog with auth-aware availability.
pub struct ModelRegistry {
    pub auth: AuthStorage,
    models_json_path: Option<PathBuf>,
    models: Vec<Model>,
    load_error: Option<String>,
    provider_request_configs: HashMap<String, ProviderRequestConfig>,
    model_request_headers: HashMap<String, BTreeMap<String, String>>,
    /// TS `xaiModelSources`: every xAI model id's configured
    /// (pre-adaptation) model, re-recorded at every load. The per-call
    /// request model in [`Self::get_api_key_and_headers`] re-resolves
    /// against this map, so it follows the credential that serves the
    /// call, not the credential the load saw.
    xai_model_sources: HashMap<String, Model>,
    /// The process-shared live catalog chain (`catalog_chain::catalog_for`);
    /// `resolve()` sources built-ins from it.
    catalog: std::sync::Arc<pa_models::ModelCatalog>,
    /// FORK: the last `OpenCode` snapshot seen (fresh or cached): the source of
    /// the retirement list `deprecated_opencode_ids` reports.
    opencode_snapshot: Option<pa_models::opencode_models::OpenCodeSnapshot>,
    /// FORK: the provider-discovery snapshot (`doc/discovery.json`): which ids
    /// each live list serves right now.
    discovery_snapshot: Option<pa_models::discovery::DiscoverySnapshot>,
    /// FORK: the HTTP fetcher every network path shares.
    fetcher: std::sync::Arc<pa_models::fetch::CatalogFetcher>,
}

/// Under a stored xAI subscription the provider's models switch onto the
/// openai-responses API with the subscription thinking maps and
/// `supportsLongCacheRetention: false` compat.
fn xai_subscription_model(model: &Model) -> Model {
    let mut adapted = model.clone();
    adapted.api = "openai-responses".to_string();
    adapted.base_url = "https://api.x.ai/v1".to_string();
    // TS fills only an absent map: a model that already declares its
    // levels keeps them.
    if adapted.thinking_level_map.is_none() {
        adapted.thinking_level_map = Some(xai_subscription_thinking_map(&model.id));
    }
    adapted.compat = Some(ModelCompat::from_kind(CompatKind::OpenAiResponses(
        OpenAiResponsesCompat {
            send_session_id_header: None,
            supports_long_cache_retention: Some(false),
        },
    )));
    adapted
}

/// The TS flow's explicit thinking maps for the grok models it names,
/// and the all-levels-null default for every other xAI model (reasoning
/// stays; unverified effort controls never send).
fn xai_subscription_thinking_map(model_id: &str) -> ThinkingLevelMap {
    // The value mirrors the TS table: `None` for the null mapping
    // (unsupported), the wire string for a named level; an absent level
    // keeps its default support.
    let null = || None;
    let value = |text: &str| Some(text.to_string());
    let mut map = ThinkingLevelMap::new();
    match model_id {
        "grok-4.3" => {
            map.insert(ModelThinkingLevel::Off, value("none"));
            map.insert(ModelThinkingLevel::Minimal, null());
        }
        "grok-4.5" => {
            map.insert(ModelThinkingLevel::Off, null());
            map.insert(ModelThinkingLevel::Minimal, null());
        }
        "grok-4.6" | "grok-4.7" => {
            map.insert(ModelThinkingLevel::Off, null());
            map.insert(ModelThinkingLevel::Minimal, null());
            map.insert(ModelThinkingLevel::Xhigh, value("xhigh"));
        }
        _ => {
            for level in [
                ModelThinkingLevel::Off,
                ModelThinkingLevel::Minimal,
                ModelThinkingLevel::Low,
                ModelThinkingLevel::Medium,
                ModelThinkingLevel::High,
                ModelThinkingLevel::Xhigh,
                ModelThinkingLevel::Max,
            ] {
                map.insert(level, null());
            }
        }
    }
    map
}

#[cfg_attr(not(test), allow(dead_code))]
fn native_free_api_key(model: &Model) -> Option<&'static str> {
    let url = url::Url::parse(&model.base_url).ok()?;
    (model.provider == "opencode"
        && model.api == "openai-completions"
        && model.id.ends_with("-free")
        && url.scheme() == "https"
        && url.host_str() == Some("opencode.ai")
        && url.port_or_known_default() == Some(443)
        && url.path().trim_end_matches('/') == "/inference/v1"
        && url.username().is_empty()
        && url.password().is_none()
        && url.query().is_none()
        && url.fragment().is_none())
    .then_some("public")
}

impl ModelRegistry {
    pub fn create(auth: AuthStorage, models_json_path: impl Into<PathBuf>) -> Self {
        Self::new(auth, Some(models_json_path.into()))
    }

    pub fn in_memory(auth: AuthStorage) -> Self {
        Self::new(auth, None)
    }

    /// FORK: a registry whose network paths use `fetcher` rather than a fresh one.
    pub fn create_with_fetcher(
        auth: AuthStorage,
        models_json_path: impl Into<PathBuf>,
        fetcher: std::sync::Arc<pa_models::fetch::CatalogFetcher>,
    ) -> Self {
        let mut registry = Self::new(auth, Some(models_json_path.into()));
        registry.fetcher = fetcher;
        registry
    }

    fn new(auth: AuthStorage, models_json_path: Option<PathBuf>) -> Self {
        let catalog = catalog_chain::catalog_for(models_json_path.as_deref());
        let mut registry = Self {
            auth,
            models_json_path,
            models: Vec::new(),
            load_error: None,
            provider_request_configs: HashMap::new(),
            model_request_headers: HashMap::new(),
            xai_model_sources: HashMap::new(),
            catalog,
            opencode_snapshot: None,
            discovery_snapshot: None,
            fetcher: std::sync::Arc::new(pa_models::fetch::CatalogFetcher::new()),
        };
        registry.load_models();
        // The SAVED discovery snapshot is applied at construction, not only on a
        // refresh: an offline restart (no network, no forced refresh) must still
        // offer the ids discovery found last time. Applying it here keeps
        // `load_models`' order (built-in set first, discovery appends after) —
        // the same order `refresh_available_models_forced` uses (audit §3.2).
        registry.apply_saved_discovery_snapshot();
        registry
    }

    fn apply_saved_discovery_snapshot(&mut self) {
        let Some(models_dir) = self
            .models_json_path
            .as_ref()
            .and_then(|path| path.parent().map(Path::to_path_buf))
        else {
            return;
        };
        let Some(snapshot) = pa_models::discovery::read_snapshot(&models_dir) else {
            return;
        };
        if snapshot.providers.is_empty() {
            return;
        }
        self.apply_discovery_snapshot(&snapshot, None);
        self.discovery_snapshot = Some(snapshot);
    }

    /// The Prime Inference credentials for the credentialed catalog layer
    /// (Bearer + team headers, as the TS refresh sends).
    fn prime_credentials(&mut self) -> Option<pa_models::PrimeCredentials> {
        let api_key = self.auth.get_api_key(PRIME_INFERENCE_PROVIDER_ID)?;
        let team_id = self
            .auth
            .get_provider_headers(PRIME_INFERENCE_PROVIDER_ID)
            .and_then(|headers| headers.get("X-Prime-Team-ID").cloned());
        Some(pa_models::PrimeCredentials { api_key, team_id })
    }

    pub fn get_error(&self) -> Option<&str> {
        self.load_error.as_deref()
    }

    /// Built-in + custom models (auth not filtered).
    pub fn get_all(&self) -> &[Model] {
        &self.models
    }

    /// Models whose provider has configured auth. Private Prime Inference
    /// models need no extra gate: only the account's scoped snapshot and
    /// models.json list them. The per-model auth probe is answered once per
    /// provider: each probe rebuilds the auth-source candidates.
    pub fn get_available(&self) -> Vec<&Model> {
        let mut auth_by_provider: HashMap<&str, bool> = HashMap::new();
        self.models
            .iter()
            .filter(|model| {
                *auth_by_provider
                    .entry(model.provider.as_str())
                    .or_insert_with(|| self.has_configured_auth(model))
            })
            .collect()
    }

    /// Models `rlm.find_models` may search: auth-configured, and not on a
    /// stale or expired provider credential (probe answered once per provider).
    pub fn get_rlm_searchable_models(&self) -> Vec<&Model> {
        let mut status_by_provider: HashMap<&str, bool> = HashMap::new();
        self.get_available()
            .into_iter()
            .filter(|model| {
                *status_by_provider
                    .entry(model.provider.as_str())
                    .or_insert_with(|| {
                        let status = self.auth.get_auth_status(&model.provider);
                        status.source != Some(crate::auth::types::AuthSource::Stale)
                            && status.label.as_deref() != Some("expired")
                    })
            })
            .collect()
    }

    pub fn has_configured_auth(&self, model: &Model) -> bool {
        self.auth.has_auth(&model.provider)
            || self.has_configured_provider_request_auth(&model.provider)
    }

    /// The provider's auth status without credential values: callers
    /// classifying unauthenticated vs stale providers read this instead of
    /// probing for keys.
    pub fn get_provider_auth_status(&self, provider: &str) -> crate::auth::types::AuthStatus {
        self.auth.get_auth_status(provider)
    }

    /// `set_model`'s model resolution: an available model resolves directly; a
    /// model whose provider has no credential (none stale) is the typed sign-in
    /// refusal; stale-auth providers keep the switch (the lookup never mutates
    /// stale state).
    ///
    /// # Errors
    ///
    /// `ProviderUnauthenticated` when the provider has no credential;
    /// `NotFound` when no catalog model matches.
    pub fn resolve_set_model_selection(
        &self,
        provider: &str,
        model_id: &str,
    ) -> Result<&Model, SetModelSelectionError> {
        let matches = |model: &Model| model.provider == provider && model.id == model_id;
        if let Some(model) = self
            .get_available()
            .into_iter()
            .find(|model| matches(model))
        {
            return Ok(model);
        }
        // A model can exist while its provider is not signed in — the
        // picker's discovery path. No credential at all is the sign-in refusal;
        // the unauthorized private Prime Inference model keeps the not-found
        // refusal — never a switch to an unentitled model.
        let Some(model) = self.get_all().iter().find(|model| matches(model)) else {
            return Err(SetModelSelectionError::NotFound {
                provider: provider.to_string(),
                model_id: model_id.to_string(),
            });
        };
        if self.get_provider_auth_status(provider).source
            == Some(crate::auth::types::AuthSource::Stale)
        {
            return Ok(model);
        }
        if !self.has_configured_auth(model) {
            return Err(SetModelSelectionError::ProviderUnauthenticated {
                provider: provider.to_string(),
            });
        }
        Err(SetModelSelectionError::NotFound {
            provider: provider.to_string(),
            model_id: model_id.to_string(),
        })
    }

    fn has_configured_provider_request_auth(&self, provider: &str) -> bool {
        let Some(config) = self.provider_request_configs.get(provider) else {
            return false;
        };
        if config.headers.is_some() || config.auth_header.is_some() {
            return true;
        }
        config.api_key.as_deref().is_some_and(|key| {
            crate::auth::resolve_config_value::resolve_config_value(key).is_some()
        })
    }

    pub fn refresh(&mut self) {
        self.provider_request_configs.clear();
        self.model_request_headers.clear();
        self.load_error = None;
        self.auth.reload();
        self.load_models();
    }

    fn load_models(&mut self) {
        let path = self.models_json_path.clone();
        let result = match &path {
            Some(path) => self.load_custom_models_file(path),
            None => CustomModelsResult::default(),
        };
        if let Some(error) = &result.error {
            self.load_error = Some(error.clone());
        }
        let credentials = self.prime_credentials();
        let built_in = self.load_built_in_models(&result, credentials.as_ref());
        self.models = Self::merge_custom_models(built_in, result.models);
        self.apply_subscription_model_adaptations();
    }

    /// Per-model base-URL/API overrides plus the xAI subscription switch,
    /// applied at load: stored credentials rewrite the affected models, and
    /// the registry reloads after every credential change.
    fn apply_subscription_model_adaptations(&mut self) {
        // Re-record the configured (pre-adaptation) model for every xAI id
        // at every load (TS `xaiModelSources`): the per-call request model
        // resolves against this map, so it follows the credential that
        // serves the call, never the credential the load saw.
        self.xai_model_sources.clear();
        for model in &self.models {
            if model.provider == crate::auth::XAI_PROVIDER_ID {
                self.xai_model_sources
                    .insert(model.id.clone(), model.clone());
            }
        }
        let copilot_credential = self
            .auth
            .get_all()
            .credential(crate::auth::GITHUB_COPILOT_PROVIDER_ID);
        if let Some(AuthCredential::Oauth {
            access,
            enterprise_url,
            ..
        }) = copilot_credential
        {
            let base_url =
                pa_ai::oauth::get_github_copilot_base_url(Some(&access), enterprise_url.as_deref());
            for model in &mut self.models {
                if model.provider == crate::auth::GITHUB_COPILOT_PROVIDER_ID {
                    model.base_url.clone_from(&base_url);
                }
            }
        }
        // The stored credential is OAuth and the store is the winning
        // source (a stale credential does not serve it).
        let xai_stored = matches!(
            self.auth.get_all().credential(crate::auth::XAI_PROVIDER_ID),
            Some(AuthCredential::Oauth { .. })
        ) && self
            .auth
            .get_auth_status(crate::auth::XAI_PROVIDER_ID)
            .source
            == Some(crate::auth::types::AuthSource::Stored);
        if xai_stored {
            for model in &mut self.models {
                if model.provider == crate::auth::XAI_PROVIDER_ID {
                    *model = xai_subscription_model(model);
                }
            }
        }
    }

    fn load_custom_models_file(&mut self, path: &Path) -> CustomModelsResult {
        if !path.exists() {
            return CustomModelsResult::default();
        }
        let Ok(content) = std::fs::read_to_string(path) else {
            return CustomModelsResult {
                error: Some(format!(
                    "Failed to load models.json\n\nFile: {}",
                    path.display()
                )),
                ..Default::default()
            };
        };
        let mut result = load_custom_models(
            &content,
            &|provider| !pa_ai::models_generated::get_models(provider).is_empty(),
            &|provider| {
                pa_ai::models_generated::get_models(provider)
                    .first()
                    .map(|model| (model.api.clone(), model.base_url.clone()))
            },
        );
        // Provider/model request config from the parsed document.
        if result.error.is_none() {
            if let Ok(config) =
                super::custom::parse_models_config(&super::custom::strip_json_comments(&content))
            {
                for (provider, provider_config) in config.providers {
                    if provider_config.api_key.is_some()
                        || provider_config.headers.is_some()
                        || provider_config.auth_header.is_some()
                    {
                        self.provider_request_configs.insert(
                            provider.clone(),
                            ProviderRequestConfig {
                                api_key: provider_config.api_key,
                                headers: provider_config.headers,
                                auth_header: provider_config.auth_header,
                            },
                        );
                    }
                    if let Some(model_overrides) = provider_config.model_overrides {
                        for (model_id, model_override) in model_overrides {
                            self.store_model_headers(
                                &provider,
                                &model_id,
                                model_override.headers.clone(),
                            );
                        }
                    }
                    if let Some(model_defs) = provider_config.models {
                        for model_def in model_defs {
                            self.store_model_headers(
                                &provider,
                                &model_def.id,
                                model_def.headers.clone(),
                            );
                        }
                    }
                }
            }
        }
        if let Some(error) = &result.error {
            // Keep built-ins only; message mirrors the TS load failure format.
            if !error.contains("models.json") {
                result.error = Some(format!("{error}\n\nFile: {}", path.display()));
            }
        }
        result
    }

    fn store_model_headers(
        &mut self,
        provider: &str,
        model_id: &str,
        headers: Option<BTreeMap<String, String>>,
    ) {
        let key = format!("{provider}:{model_id}");
        match headers {
            Some(headers) if !headers.is_empty() => {
                self.model_request_headers.insert(key, headers);
            }
            _ => {
                self.model_request_headers.remove(&key);
            }
        }
    }

    /// Built-ins through the no-cold-start chain (validated disk snapshot |
    /// bundled asset | compiled fallback), merged with the credentialed
    /// live Prime Inference snapshot for `credentials`.
    fn load_built_in_models(
        &self,
        custom: &CustomModelsResult,
        credentials: Option<&pa_models::PrimeCredentials>,
    ) -> Vec<Model> {
        self.catalog
            .resolve(credentials)
            .into_iter()
            // FORK-ONLY: the fork's own providers (the OpenCode free tier among
            // them) ride the built-in set — the remote catalog never carries
            // them. A models.json override below still applies to them like any
            // other provider.
            .chain(pa_ai::models_fork::fork_provider_models())
            .map(|mut model| {
                if let Some(provider_override) = custom.provider_overrides.get(&model.provider) {
                    if let Some(base_url) = &provider_override.base_url {
                        model.base_url = base_url.clone();
                        // The live catalog's serving class labels the
                        // official endpoint; a user-replaced endpoint voids
                        // it (the label must not claim a serving path the
                        // route no longer uses).
                        if model.provider == PRIME_INFERENCE_PROVIDER_ID {
                            model.serving = None;
                        }
                    }
                    if provider_override.compat.is_some() {
                        model.compat =
                            merge_compat(model.compat.as_ref(), provider_override.compat.clone());
                    }
                }
                if let Some(model_override) = custom
                    .model_overrides
                    .get(&model.provider)
                    .and_then(|overrides| overrides.get(&model.id))
                {
                    model = apply_model_override(&model, model_override);
                }
                model
            })
            .collect()
    }

    /// Custom models win on provider+id conflicts.
    fn merge_custom_models(mut built_in: Vec<Model>, custom: Vec<Model>) -> Vec<Model> {
        for custom_model in custom {
            match built_in.iter().position(|model| {
                model.provider == custom_model.provider && model.id == custom_model.id
            }) {
                Some(index) => built_in[index] = custom_model,
                None => built_in.push(custom_model),
            }
        }
        built_in
    }

    /// Reload local state and refresh the live catalog (the provider catalog
    /// and the account's scoped Prime Inference snapshot, which carries its
    /// private routes). Both fetches are awaited, so the resolved catalog
    /// reflects them when the call returns.
    pub async fn refresh_available_models(&mut self) -> Vec<Model> {
        self.refresh_available_models_forced(false).await
    }

    /// [`ModelRegistry::refresh_available_models`] with the refresh trigger's
    /// gating: forced triggers (startup, auth change) skip the hourly catalog
    /// gate, picker-open keeps it.
    pub async fn refresh_available_models_with_trigger(
        &mut self,
        trigger: pa_models::RefreshTrigger,
    ) -> Vec<Model> {
        self.refresh_available_models_forced(trigger.forced()).await
    }

    /// The awaited refresh body: reload, the gated-or-forced chain fetch,
    /// then the auth-filtered catalog.
    async fn refresh_available_models_forced(&mut self, force: bool) -> Vec<Model> {
        self.refresh();
        let catalog_credentials = self.prime_credentials();
        self.catalog
            .refresh_with_credentials(force, catalog_credentials.as_ref())
            .await;
        // FORK: the OpenCode free-tier metadata and the provider-discovery
        // snapshot are the fork's own lanes; upstream #4044 removed only the
        // private-authorization one (the credential-scoped snapshot owns it).
        self.refresh_opencode_free_tier(force).await;
        if force {
            if let Some(models_dir) = self
                .models_json_path
                .as_ref()
                .and_then(|path| path.parent().map(Path::to_path_buf))
            {
                self.refresh_discovery(models_dir.as_path()).await;
            }
        }
        self.load_models();
        self.get_available().into_iter().cloned().collect()
    }

    /// Resolve request auth: API key + merged headers for a model.
    pub fn get_api_key_and_headers(
        &mut self,
        model: &Model,
        request_headers: Option<&BTreeMap<String, String>>,
    ) -> ResolvedRequestAuth {
        let stored = self
            .auth
            .get_api_key_with_source_token(&model.provider, false);
        let mut api_key = stored.api_key;
        let provider_config = self.provider_request_configs.get(&model.provider).cloned();
        if api_key.is_none() {
            if let Some(config) = &provider_config {
                if let Some(configured) = &config.api_key {
                    if let Some(resolved) =
                        crate::auth::resolve_config_value::resolve_config_value(configured)
                    {
                        api_key = Some(resolved);
                    }
                }
            }
        }
        // FORK-ONLY: the native OpenCode free endpoint needs no credential of its own.
        if api_key.is_none() {
            api_key = native_free_api_key(model).map(str::to_string);
        }
        // A failed OAuth refresh with nothing else to serve: fail with the re-login sentence.
        if let (None, Some(failure)) = (&api_key, &stored.refresh_failure) {
            return ResolvedRequestAuth {
                ok: false,
                error: Some(failure.to_string()),
                ..Default::default()
            };
        }
        // TS `getApiKeyAndHeaders` resolves the xAI request model per call
        // from the lookup's own result (v0.9.8 model-registry.ts:1604-1619):
        // the subscription shape only when the stored OAuth credential
        // serves THIS call, the configured API-key shape otherwise — a
        // failed refresh that falls back to the configured key never sends
        // that key on the subscription request shape, and no key at all
        // fails the call instead of streaming keyless. Other providers
        // keep the caller's model.
        let mut request_model = None;
        let mut xai_subscription_serving = false;
        if model.provider == crate::auth::XAI_PROVIDER_ID {
            if api_key.is_none() {
                return ResolvedRequestAuth {
                    ok: false,
                    error: Some(
                        "No usable xAI credential. Use /login and select an xAI entry to sign in again or configure an API key."
                            .to_string(),
                    ),
                    ..Default::default()
                };
            }
            let configured = self
                .xai_model_sources
                .get(&model.id)
                .cloned()
                .unwrap_or_else(|| model.clone());
            xai_subscription_serving = stored.credential_type == Some("oauth")
                && matches!(
                    &stored.source_token,
                    Some(token) if token.source == crate::auth::types::AuthSource::Stored
                );
            request_model = Some(if xai_subscription_serving {
                xai_subscription_model(&configured)
            } else {
                configured
            });
        }
        let provider_headers = provider_config
            .as_ref()
            .and_then(|config| config.headers.clone());
        let auth_storage_headers = self.auth.get_provider_headers(&model.provider);
        let model_request_key = format!("{}:{}", model.provider, model.id);
        let model_request_headers = self.model_request_headers.get(&model_request_key).cloned();

        let mut headers: BTreeMap<String, String> = BTreeMap::new();
        if let Some(model_headers) = &model.headers {
            headers.extend(model_headers.clone());
        }
        if let Some(auth_storage_headers) = auth_storage_headers {
            headers.extend(auth_storage_headers);
        }
        if let Some(provider_headers) = provider_headers {
            headers.extend(provider_headers);
        }
        if let Some(model_request_headers) = model_request_headers {
            headers.extend(model_request_headers);
        }
        if provider_config
            .as_ref()
            .and_then(|config| config.auth_header)
            .unwrap_or(false)
        {
            let Some(api_key) = &api_key else {
                return ResolvedRequestAuth {
                    ok: false,
                    error: Some(format!("No API key found for \"{}\"", model.provider)),
                    ..Default::default()
                };
            };
            headers.insert("Authorization".to_string(), format!("Bearer {api_key}"));
        }
        if let Some(request_headers) = request_headers {
            headers.extend(request_headers.clone());
        }
        if xai_subscription_serving {
            // TS refuses a custom Authorization header on the subscription
            // shape (model-registry.ts:1653-1657): only the bearer of the
            // serving credential may ride the request.
            if let Some(bearer) = api_key.as_deref().map(|key| format!("Bearer {key}")) {
                if headers.iter().any(|(name, value)| {
                    name.eq_ignore_ascii_case("authorization") && *value != bearer
                }) {
                    return ResolvedRequestAuth {
                        ok: false,
                        error: Some(
                            "Grok subscription cannot use a custom Authorization header. Remove the header or use /login and select the xAI API-key entry."
                                .to_string(),
                        ),
                        ..Default::default()
                    };
                }
            }
        }
        ResolvedRequestAuth {
            ok: true,
            api_key,
            headers: (!headers.is_empty()).then_some(headers),
            error: None,
            request_model,
        }
    }

    async fn refresh_discovery(&mut self, models_dir: &Path) {
        // Offline means offline (review #10-adjacent): this fetches `models.dev` and then every
        // dictionary provider's own endpoint, and it was the THIRD network path with no
        // `PI_OFFLINE` guard. The cached snapshot below still applies, so an offline run keeps
        // whatever discovery last found instead of silently contacting the network.
        if is_offline_mode_enabled() {
            if let Some(snapshot) = pa_models::discovery::read_snapshot(models_dir) {
                self.apply_discovery_snapshot(&snapshot, None);
            }
            return;
        }
        let dictionary = match pa_models::discovery::load_embedded_dictionary() {
            Ok(dictionary) => dictionary,
            Err(error) => {
                tracing::debug!(%error, "model discovery dictionary is invalid");
                return;
            }
        };
        let mut snapshot = pa_models::discovery::read_snapshot(models_dir).unwrap_or_default();
        let fetcher = std::sync::Arc::clone(&self.fetcher);
        // models.dev is the precedence-3 properties source for every dictionary provider that
        // names a key; fetched once per refresh and shared by the fold below.
        let models_dev = match fetcher
            .fetch(pa_models::opencode_models::MODELS_DEV_URL, None)
            .await
        {
            Ok(pa_models::fetch::FetchOutcome::Fresh { body, .. }) => {
                serde_json::from_slice::<serde_json::Value>(&body).ok()
            }
            _ => None,
        };
        // opencode passes an empty stamp the same way; the snapshot mtime is the clock.
        let synced_at = String::new();
        for entry in &dictionary.providers {
            let headers = self.provider_discovery_headers(&entry.provider).clone();
            match pa_models::discovery::discover_provider_with_headers(
                &fetcher,
                entry,
                &headers,
                synced_at.clone(),
            )
            .await
            {
                Ok(discovered) => snapshot.merge(discovered),
                Err(error) => {
                    tracing::debug!(provider = %entry.provider, %error, "model discovery skipped");
                }
            }
        }
        if snapshot.providers.is_empty() {
            return;
        }
        self.apply_discovery_snapshot(&snapshot, models_dev.as_ref());
        let _ = pa_models::discovery::write_snapshot(models_dir, &snapshot);
        self.discovery_snapshot = Some(snapshot);
    }

    async fn refresh_opencode_free_tier(&mut self, force: bool) {
        let Some(models_json_path) = self.models_json_path.clone() else {
            return;
        };
        let Some(models_dir) = models_json_path.parent() else {
            return;
        };
        let cached = pa_models::opencode_models::read_snapshot(models_dir);
        if !force {
            if let Some(cached) = &cached {
                let fresh =
                    std::fs::metadata(pa_models::opencode_models::snapshot_path(models_dir))
                        .and_then(|metadata| metadata.modified())
                        .ok()
                        .and_then(|modified| modified.elapsed().ok())
                        .is_some_and(|age| age < pa_models::opencode_models::REFRESH_INTERVAL);
                if fresh {
                    self.apply_opencode_snapshot(cached);
                    self.opencode_snapshot = Some(cached.clone());
                    return;
                }
            }
        }
        // The ONLY network refresh without this guard. `refresh_private_prime_inference_
        // authorization` and the catalog refresh both return early when offline; this one
        // fetched anyway, so an offline daemon still reached the network — and in tests it
        // reached the LIVE endpoint, which is how `refresh_failure_keeps_the_last_good_
        // catalog_and_stays_silent` started failing: the primary refresh failed as designed,
        // while this one succeeded and changed the payload under it.
        if is_offline_mode_enabled() {
            if let Some(cached) = cached {
                self.apply_opencode_snapshot(&cached);
                self.opencode_snapshot = Some(cached);
            }
            return;
        }
        let fetcher = std::sync::Arc::clone(&self.fetcher);
        let known = |id: &str| {
            pa_ai::models_fork::fork_provider_models()
                .into_iter()
                .find(|model| {
                    model.provider == pa_models::opencode_models::PROVIDER && model.id == id
                })
                .map(|model| pa_models::opencode_models::KnownCapabilities {
                    context_window: model.context_window,
                    max_output: model.max_tokens,
                })
        };
        // A failed refresh keeps whatever the cache held.
        if let Ok(snapshot) =
            pa_models::opencode_models::refresh(&fetcher, models_dir, &known, String::new()).await
        {
            self.apply_opencode_snapshot(&snapshot);
            self.opencode_snapshot = Some(snapshot);
        } else if let Some(cached) = cached {
            self.apply_opencode_snapshot(&cached);
            self.opencode_snapshot = Some(cached);
        }
    }

    fn apply_discovery_snapshot(
        &mut self,
        snapshot: &pa_models::discovery::DiscoverySnapshot,
        models_dev: Option<&serde_json::Value>,
    ) {
        // A live id with no hand-table row is appended as a discovery model. Its properties
        // follow the discovery contract's precedence: a `models.dev` entry wins
        // (`CatalogDelta`), otherwise `Estimated` — "existence outranks properties": a working
        // selection must not turn into NotFound. A hand-table row the endpoint stopped serving
        // stays in place — deprecate, not delete; the deprecated list names it.
        let dictionary = pa_models::discovery::load_embedded_dictionary().ok();
        let mut appended: Vec<pa_types::ai::Model> = Vec::new();
        for entry in snapshot.providers.values() {
            let hand_table: Vec<&str> = self
                .models
                .iter()
                .filter(|model| model.provider == entry.provider)
                .map(|model| model.id.as_str())
                .collect();
            let dev_key = dictionary
                .as_ref()
                .and_then(|dictionary| {
                    dictionary
                        .providers
                        .iter()
                        .find(|row| row.provider == entry.provider)
                })
                .and_then(|row| row.models_dev_key.clone());
            for id in &entry.ids {
                if hand_table.contains(&id.as_str()) {
                    continue;
                }
                let dev = dev_key
                    .as_deref()
                    .zip(models_dev)
                    .and_then(|(key, payload)| pa_models::discovery::dev_entry(payload, key, id))
                    .map(pa_models::discovery::dev_properties);
                let (context_window, max_tokens) = match dev.as_ref() {
                    Some(properties) => (
                        properties.context_window.unwrap_or(128_000),
                        properties.max_output.unwrap_or(4_096),
                    ),
                    None => (128_000, 4_096),
                };
                let thinking_level_map = dev.as_ref().and_then(|properties| {
                    let values = properties.reasoning_effort_values.as_ref()?;
                    Some(Self::build_thinking_level_map(values))
                });
                let input = if dev
                    .as_ref()
                    .and_then(|properties| properties.input_images)
                    .unwrap_or(false)
                {
                    vec![
                        pa_types::ai::ModelInput::Text,
                        pa_types::ai::ModelInput::Image,
                    ]
                } else {
                    vec![pa_types::ai::ModelInput::Text]
                };
                let name =
                    Self::dev_entry_name(models_dev, dev_key.as_deref().unwrap_or_default(), id);
                appended.push(pa_types::ai::Model {
                    id: id.clone(),
                    name,
                    api: "openai-completions".to_string(),
                    provider: entry.provider.clone(),
                    base_url: String::new(),
                    reasoning: dev
                        .as_ref()
                        .and_then(|properties| properties.reasoning)
                        .unwrap_or(false),
                    thinking_level_map,
                    input,
                    cost: pa_types::ai::ModelCost {
                        input: pa_types::JsNumber(
                            dev.as_ref()
                                .and_then(|properties| properties.cost_input)
                                .unwrap_or(0.0),
                        ),
                        output: pa_types::JsNumber(
                            dev.as_ref()
                                .and_then(|properties| properties.cost_output)
                                .unwrap_or(0.0),
                        ),
                        cache_read: pa_types::JsNumber(
                            dev.as_ref()
                                .and_then(|properties| properties.cost_cache_read)
                                .unwrap_or(0.0),
                        ),
                        cache_write: pa_types::JsNumber(0.0),
                    },
                    context_window,
                    max_tokens,
                    max_tokens_explicit: false,
                    featured: None,
                    // A discovered dictionary provider is not a Prime Inference route.
                    serving: None,
                    headers: None,
                    compat: None,
                });
            }
        }
        if appended.is_empty() {
            return;
        }
        tracing::debug!(
            count = appended.len(),
            providers = %appended.iter().map(|model| model.provider.clone()).collect::<Vec<_>>().join(","),
            "model discovery appended models"
        );
        self.models.extend(appended);
    }

    fn apply_opencode_snapshot(&mut self, snapshot: &pa_models::opencode_models::OpenCodeSnapshot) {
        for model in &mut self.models {
            if model.provider != pa_models::opencode_models::PROVIDER {
                continue;
            }
            let Some(live) = snapshot.models.iter().find(|live| live.id == model.id) else {
                continue;
            };
            if let Some(context) = live.context_window {
                model.context_window = context;
            }
            if let Some(output) = live.max_output {
                model.max_tokens = output;
            }
        }
    }

    pub fn get_discoverable_models(&self) -> Vec<&Model> {
        let available: Vec<&Model> = self.get_available();
        let known: std::collections::HashSet<(&str, &str)> = available
            .iter()
            .map(|model| (model.provider.as_str(), model.id.as_str()))
            .collect();
        let mut discoverable = available;
        discoverable.extend(
            self.models
                .iter()
                .filter(|model| {
                    !known.contains(&(model.provider.as_str(), model.id.as_str()))
                        && native_free_api_key(model).is_some()
                })
                .filter(|model| !is_private_prime_inference_model(model)),
        );
        discoverable
    }

    fn provider_discovery_headers(&mut self, provider: &str) -> Vec<(String, String)> {
        let mut headers: Vec<(String, String)> = self
            .auth
            .get_provider_headers(provider)
            .map(|headers| headers.into_iter().collect())
            .unwrap_or_default();
        if !headers
            .iter()
            .any(|(name, _)| name.eq_ignore_ascii_case("authorization"))
        {
            if let Some(api_key) = self
                .auth
                .get_api_key_with_source_token(provider, false)
                .api_key
            {
                if !api_key.trim().is_empty() {
                    headers.push(("Authorization".to_string(), format!("Bearer {api_key}")));
                }
            }
        }
        headers
    }

    pub fn deprecated_opencode_ids(&self) -> Vec<(String, String)> {
        let mut ids: Vec<&str> = self
            .models
            .iter()
            .filter(|model| model.provider == pa_models::opencode_models::PROVIDER)
            .map(|model| model.id.as_str())
            .collect();
        ids.sort_unstable();
        ids.dedup();
        self.opencode_snapshot
            .as_ref()
            .map(|snapshot| {
                snapshot
                    .deprecated
                    .iter()
                    .filter(|gone| ids.binary_search(&gone.id.as_str()).is_ok())
                    .map(|gone| (gone.id.clone(), gone.deprecated_at.clone()))
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn has_request_auth(&self, model: &Model) -> bool {
        self.has_configured_auth(model) || native_free_api_key(model).is_some()
    }

    fn build_thinking_level_map(values: &[String]) -> pa_types::ai::ThinkingLevelMap {
        use pa_types::ai::ModelThinkingLevel;
        let effort_of: std::collections::BTreeMap<&str, &str> = values
            .iter()
            .map(|value| (value.as_str(), value.as_str()))
            .collect();
        let pick = |level: ModelThinkingLevel| {
            let name = level.wire_name();
            effort_of.get(name).map(|effort| (*effort).to_string())
        };
        ThinkingLevelMap::from([
            (ModelThinkingLevel::Off, None),
            (
                ModelThinkingLevel::Minimal,
                pick(ModelThinkingLevel::Minimal),
            ),
            (ModelThinkingLevel::Low, pick(ModelThinkingLevel::Low)),
            (ModelThinkingLevel::Medium, pick(ModelThinkingLevel::Medium)),
            (ModelThinkingLevel::High, pick(ModelThinkingLevel::High)),
            (ModelThinkingLevel::Xhigh, pick(ModelThinkingLevel::Xhigh)),
            (ModelThinkingLevel::Max, pick(ModelThinkingLevel::Max)),
        ])
    }

    /// FORK: the display name `models.dev` carries for an id, else the id itself.
    fn dev_entry_name(
        models_dev: Option<&serde_json::Value>,
        models_dev_key: &str,
        id: &str,
    ) -> String {
        models_dev
            .and_then(|payload| payload.get(models_dev_key))
            .and_then(|provider| provider.get("models"))
            .and_then(|models| models.get(id))
            .and_then(|entry| entry.get("name"))
            .and_then(serde_json::Value::as_str)
            .map_or_else(|| id.to_string(), str::to_string)
    }
}

#[cfg(test)]
mod tests;
