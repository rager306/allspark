//! Refreshing a stored MCP credential, refusing anything whose binding drifted.

use anyhow::{bail, Result};

use crate::auth::types::AuthCredential;

use super::super::oauth_discovery::{
    canonical_resource, discover, exchange_token, validated_https_url, AudienceMode,
};
use super::super::oauth_http::OAuthHttp;
use super::credentials::{to_credentials, ResourceBinding};
use super::ui::McpOAuthConfig;

/// Refresh stored credentials. Every binding the login established must
/// still hold; anything drifted requires a fresh login.
///
/// # Errors
///
/// Returns an error when the stored credential is not OAuth, is no longer
/// bound to the same endpoint, resource audience (exact grants pin to the
/// canonical endpoint, origin-mode grants to the endpoint's origin),
/// issuer, or token endpoint, carries an unknown audience mode, no refresh
/// token, when discovery fails or changed modes, or when the token exchange
/// fails.
///
/// # Panics
///
/// The `expect` on the discovery resource is unreachable.
pub async fn mcp_refresh_token(
    http: &dyn OAuthHttp,
    config: &McpOAuthConfig,
    credentials: &AuthCredential,
) -> Result<AuthCredential> {
    let AuthCredential::Oauth {
        refresh,
        endpoint,
        token_endpoint,
        client_id,
        resource,
        issuer,
        audience_mode,
        ..
    } = credentials
    else {
        bail!(
            "Stored credentials for {} are not OAuth; re-run /mcp login {}",
            config.label,
            config.server
        );
    };
    if endpoint.as_deref() != Some(config.url.as_str()) {
        bail!(
            "Stored OAuth credentials are not bound to {}; re-run /mcp login {}",
            config.url,
            config.server
        );
    }
    let endpoint_url = validated_https_url(&config.url, "MCP endpoint")?;
    let configured_resource = canonical_resource(&endpoint_url);
    if let Some(stored_resource) = resource {
        // Audience-aware binding (TS parity): exact grants (and legacy
        // grants, which were only ever issued under exact matching) pin to
        // the canonical endpoint; origin-mode grants pin to the endpoint's
        // exact origin. Anything else fails closed.
        let stored_audience = canonical_resource(&validated_https_url(
            stored_resource,
            "Stored resource audience",
        )?);
        let allowed_audience = if audience_mode.as_deref() == Some("origin") {
            endpoint_url.origin().ascii_serialization()
        } else {
            configured_resource.clone()
        };
        if stored_audience != allowed_audience {
            bail!(
                "Stored OAuth credentials are not bound to {allowed_audience}; re-run /mcp \
                 login {}",
                config.server
            );
        }
    }
    match (resource.is_some(), issuer.is_some()) {
        (true, true) | (false, false) => {}
        _ => bail!(
            "Stored OAuth credentials for {} have incomplete resource binding; re-run /mcp \
             login {}",
            config.label,
            config.server
        ),
    }
    // Stored audience modes are JSON-sourced; unknown values require
    // re-login.
    if resource.is_some()
        && audience_mode.is_some()
        && audience_mode.as_deref() != Some("exact")
        && audience_mode.as_deref() != Some("origin")
    {
        bail!(
            "Stored OAuth credentials for {} have an unknown audience mode; re-run /mcp \
             login {}",
            config.label,
            config.server
        );
    }
    // Legacy credentials predate audience modes: they were only ever
    // issued under exact resource matching, so only "exact" re-discovery
    // may serve them.
    let legacy_audience = resource.is_some() && audience_mode.is_none();
    if let Some(issuer) = issuer {
        validated_https_url(issuer, "Stored authorization server issuer")?;
    }
    let Some(refresh_token) = refresh.as_deref().filter(|token| !token.is_empty()) else {
        bail!(
            "No refresh token stored for {}; re-run /mcp login {}",
            config.label,
            config.server
        );
    };

    let discovery = discover(http, &config.url).await?;
    if resource.is_some() != discovery.resource.is_some() {
        bail!(
            "OAuth discovery mode changed for {}; re-run /mcp login {}",
            config.url,
            config.server
        );
    }
    if let Some(stored_resource) = resource {
        let current_resource = discovery
            .resource
            .as_deref()
            .expect("mode change rejected above");
        if stored_resource != current_resource || issuer.as_deref() != discovery.issuer.as_deref() {
            bail!(
                "Stored OAuth credentials do not match current protected-resource metadata \
                 for {}",
                config.url
            );
        }
        let current_mode = discovery.audience_mode;
        if legacy_audience && current_mode != Some(AudienceMode::Exact) {
            bail!(
                "Stored OAuth credentials for {} predate origin-level resource audiences and \
                 may not be refreshed against them; re-run /mcp login {}",
                config.label,
                config.server
            );
        }
        if !legacy_audience && current_mode.map(AudienceMode::as_str) != audience_mode.as_deref() {
            bail!(
                "Stored OAuth credentials do not match current resource audience for {}; \
                 re-run /mcp login {}",
                config.url,
                config.server
            );
        }
    }
    let stored_token_endpoint = token_endpoint.clone();
    let token_endpoint = stored_token_endpoint
        .clone()
        .unwrap_or_else(|| discovery.metadata.token_endpoint.clone());
    if let Some(stored_endpoint) = stored_token_endpoint {
        if stored_endpoint != discovery.metadata.token_endpoint {
            bail!(
                "Stored OAuth token endpoint does not match current authorization-server \
                 metadata for {}",
                config.url
            );
        }
    }
    let client_id = client_id
        .clone()
        .or_else(|| config.client_id.clone())
        .unwrap_or_default();
    if token_endpoint.is_empty() {
        bail!(
            "No token endpoint stored for {}; re-run /mcp login {}",
            config.label,
            config.server
        );
    }
    let mut refresh_params: Vec<(String, String)> = vec![
        ("grant_type".to_string(), "refresh_token".to_string()),
        ("refresh_token".to_string(), refresh_token.to_string()),
    ];
    if !client_id.is_empty() {
        refresh_params.push(("client_id".to_string(), client_id.clone()));
    }
    if let Some(client_secret) = &config.client_secret {
        refresh_params.push(("client_secret".to_string(), client_secret.clone()));
    }
    if let Some(resource) = resource {
        refresh_params.push(("resource".to_string(), resource.clone()));
    }
    let token = exchange_token(http, &token_endpoint, &refresh_params).await?;
    Ok(to_credentials(
        token,
        &token_endpoint,
        &client_id,
        endpoint.as_deref(),
        &ResourceBinding {
            resource: resource.clone(),
            issuer: issuer.clone(),
            audience_mode: audience_mode.clone(),
        },
        refresh.as_deref(),
    ))
}
