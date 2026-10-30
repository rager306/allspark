//! The one credential-building step both the login and the refresh share:
//! the persisted `expires`, refresh-token carry-over, and audience binding.

use crate::auth::types::AuthCredential;

use super::super::oauth_discovery::{TokenResponse, TOKEN_EXPIRY_BUFFER_MS};

/// Wall-clock milliseconds since the epoch (the `expires` convention).
fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(i64::MAX, |elapsed| elapsed.as_millis() as i64)
}

/// The audience binding a login persists with the credential (the TS
/// `Discovery` trio): the declared resource, the PRM-selected issuer, and
/// how the resource associates with the configured endpoint.
#[derive(Debug, Clone, Default)]
pub(super) struct ResourceBinding {
    pub(super) resource: Option<String>,
    pub(super) issuer: Option<String>,
    pub(super) audience_mode: Option<String>,
}

/// Build the credential the flow persists. A missing `expires_in` defaults to one hour; some
/// servers omit `refresh_token`, so the prior one is kept.
pub(super) fn to_credentials(
    token: TokenResponse,
    token_endpoint: &str,
    client_id: &str,
    endpoint: Option<&str>,
    binding: &ResourceBinding,
    previous_refresh: Option<&str>,
) -> AuthCredential {
    AuthCredential::Oauth {
        access: token.access_token,
        refresh: token
            .refresh_token
            .or_else(|| previous_refresh.map(str::to_string)),
        expires: now_ms()
            + token
                .expires_in
                .map_or(3600 * 1000, |seconds| seconds * 1000)
            - TOKEN_EXPIRY_BUFFER_MS,
        account_id: None,
        endpoint: endpoint.map(str::to_string),
        token_endpoint: Some(token_endpoint.to_string()),
        client_id: (!client_id.is_empty()).then(|| client_id.to_string()),
        resource: binding.resource.clone(),
        issuer: binding.issuer.clone(),
        audience_mode: binding.audience_mode.clone(),
        enterprise_url: None,
    }
}
