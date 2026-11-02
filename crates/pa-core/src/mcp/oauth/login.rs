//! One interactive login: discover, register, race the browser callback
//! against a manual paste, and exchange the code.

use anyhow::{anyhow, bail, Result};
use tokio::time::Duration;
use url::Url;

use crate::auth::types::AuthCredential;

use super::super::oauth_callback::{CallbackCode, CallbackServer};
use super::super::oauth_discovery::{
    discover, exchange_token, generate_pkce, parse_redirect_input, random_state, register_client,
    AudienceMode,
};
use super::super::oauth_http::OAuthHttp;
use super::credentials::{to_credentials, ResourceBinding};
use super::ui::{McpLoginUi, McpOAuthConfig};

/// The authorization URL a login opens: the PKCE challenge, the CSRF state,
/// the callback redirect, the scopes, and the resource indicator.
fn authorization_url(endpoint: &str, params: &[(String, String)]) -> Result<String> {
    let mut url = Url::parse(endpoint).map_err(|_| anyhow!("Authorization endpoint is invalid"))?;
    for (name, value) in params {
        url.query_pairs_mut().append_pair(name, value);
    }
    Ok(url.to_string())
}

/// What one manual paste settled into.
enum ManualOutcome {
    /// A valid pasted redirect.
    Code(CallbackCode),
    /// The paste surface cancelled without usable input.
    Cancelled,
    /// A real paste failed validation (bad state, no code).
    Failed(anyhow::Error),
}

/// Run one interactive login; the returned credential is ready to persist under `mcp:<server>`.
///
/// # Errors
///
/// Returns an error when discovery, registration, the callback server, the
/// paste validation, or the token exchange fails.
pub async fn mcp_login(
    http: &dyn OAuthHttp,
    config: &McpOAuthConfig,
    ui: &dyn McpLoginUi,
) -> Result<AuthCredential> {
    super::super::oauth_callback::validate_callback_port(config.callback_port)?;
    let discovery = discover(http, &config.url).await?;
    ui.on_progress(&format!(
        "Discovered {}",
        discovery
            .issuer
            .as_deref()
            .unwrap_or(discovery.metadata.issuer.as_str())
    ));

    // SEP-835: configured scopes, then the protected-resource metadata's
    // scopes; never a blind AS-wide join.
    let scope = config
        .scopes
        .clone()
        .or_else(|| {
            discovery
                .protected_resource
                .as_ref()
                .and_then(|protected| protected.scopes_supported.as_ref())
                .map(|scopes| scopes.join(" "))
        })
        .unwrap_or_default();

    let client_id = if let Some(client_id) = &config.client_id {
        client_id.clone()
    } else {
        let Some(registration_endpoint) = &discovery.metadata.registration_endpoint else {
            bail!(
                "{} does not support dynamic client registration and no clientId was \
                     configured. Set a pre-registered client id for this server.",
                config.label
            );
        };
        ui.on_progress("Registering OAuth client…");
        register_client(
            http,
            registration_endpoint,
            &config.label,
            (!scope.is_empty()).then_some(scope.as_str()),
            config.callback_port,
        )
        .await?
    };

    let (verifier, challenge) = generate_pkce();
    // `state` is independent of the PKCE verifier: the verifier is the
    // token-exchange secret, `state` is echoed on the redirect URL.
    let state = random_state();
    let callback = CallbackServer::start(&config.label, config.callback_port).await?;
    let redirect_uri = callback.redirect_uri();
    let mut auth_params: Vec<(String, String)> = vec![
        ("client_id".to_string(), client_id.clone()),
        ("response_type".to_string(), "code".to_string()),
        ("redirect_uri".to_string(), redirect_uri.clone()),
        ("code_challenge".to_string(), challenge),
        ("code_challenge_method".to_string(), "S256".to_string()),
        ("state".to_string(), state.clone()),
    ];
    if !scope.is_empty() {
        auth_params.push(("scope".to_string(), scope));
    }
    if let Some(resource) = &discovery.resource {
        auth_params.push(("resource".to_string(), resource.clone()));
    }
    // Google issues a refresh token only for offline access, and re-issues one only on consent.
    if discovery.metadata.issuer == "https://accounts.google.com" {
        auth_params.push(("access_type".to_string(), "offline".to_string()));
        auth_params.push(("prompt".to_string(), "consent".to_string()));
    }
    let auth_url = authorization_url(&discovery.metadata.authorization_endpoint, &auth_params)?;
    ui.on_auth(
        &auth_url,
        "Complete login in your browser. If the browser is on another machine, paste the \
         final redirect URL here.",
    );

    // Race the local callback server against a manual paste. A real paste cancels the callback
    // waiter; manual cancellation settles it after a short grace so an in-flight browser redirect
    // can win first.
    let mut manual_input = ui.on_manual_code_input().map(|manual| {
        // Keep this future inside the login: a detached task could retain the callback
        // server after the user cancelled the parent login.
        let callback = &callback;
        let state = state.clone();
        Box::pin(async move {
            let Some(input) = manual.await else {
                tokio::time::sleep(Duration::from_millis(500)).await;
                callback.cancel().await;
                return ManualOutcome::Cancelled;
            };
            match parse_redirect_input(&input, &state) {
                Ok((code, state)) => {
                    callback.cancel().await;
                    ManualOutcome::Code(CallbackCode { code, state })
                }
                Err(error) => {
                    // A validation error on a real paste is a genuine failure the caller surfaces;
                    // a cancelled surface is not.
                    let genuine = error.to_string().contains("state mismatch")
                        || error.to_string().contains("authorization code");
                    tokio::time::sleep(Duration::from_millis(500)).await;
                    callback.cancel().await;
                    if genuine {
                        ManualOutcome::Failed(error)
                    } else {
                        ManualOutcome::Cancelled
                    }
                }
            }
        })
    });

    let (mut result, manual_outcome) = if let Some(input) = manual_input.as_mut() {
        callback.wait_for_code_or_input(input.as_mut()).await
    } else {
        (callback.wait_for_code().await, None)
    };
    let mut manual_error: Option<anyhow::Error> = None;
    if result.is_none() {
        if let Some(input) = manual_input.take() {
            let outcome = match manual_outcome {
                Some(outcome) => outcome,
                None => input.await,
            };
            match outcome {
                ManualOutcome::Code(code) => result = Some(code),
                ManualOutcome::Cancelled => {}
                ManualOutcome::Failed(error) => manual_error = Some(error),
            }
        } else {
            // No paste surface: a blocking prompt is the fallback once the
            // callback has settled without a redirect.
            let input = ui
                .on_prompt(
                    "Paste the authorization code or full redirect URL:",
                    &redirect_uri,
                )
                .await?;
            let (code, state) = parse_redirect_input(&input, &state)?;
            result = Some(CallbackCode { code, state });
        }
    }
    drop(manual_input);
    callback.cancel().await;
    let Some(result) = result else {
        return Err(manual_error.unwrap_or_else(|| anyhow!("Missing authorization code")));
    };
    if result.state != state {
        bail!("OAuth state mismatch");
    }

    ui.on_progress("Exchanging authorization code for tokens…");
    let mut token_params: Vec<(String, String)> = vec![
        ("grant_type".to_string(), "authorization_code".to_string()),
        ("code".to_string(), result.code),
        ("redirect_uri".to_string(), redirect_uri),
        ("client_id".to_string(), client_id.clone()),
        ("code_verifier".to_string(), verifier),
    ];
    if let Some(client_secret) = &config.client_secret {
        token_params.push(("client_secret".to_string(), client_secret.clone()));
    }
    if let Some(resource) = &discovery.resource {
        token_params.push(("resource".to_string(), resource.clone()));
    }
    let token = exchange_token(http, &discovery.metadata.token_endpoint, &token_params).await?;
    Ok(to_credentials(
        token,
        &discovery.metadata.token_endpoint,
        &client_id,
        Some(&config.url),
        &ResourceBinding {
            resource: discovery.resource.clone(),
            issuer: discovery.issuer.clone(),
            audience_mode: discovery
                .audience_mode
                .map(AudienceMode::as_str)
                .map(str::to_string),
        },
        None,
    ))
}
