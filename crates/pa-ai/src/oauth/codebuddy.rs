//! The `CodeBuddy` (codebuddy.ai) OAuth token refresh.
//!
//! `CodeBuddy`'s IDE/CLI plan authenticates with a browser-driven device flow
//! (`POST /v2/plugin/auth/state` -> user approves -> `GET /v2/plugin/auth/token`)
//! that yields an access token plus a long-lived refresh token. The access
//! token expires (~1 year, but revocable), so a stored credential refreshes at
//! `POST /v2/plugin/auth/token/refresh` with the refresh token in the
//! `X-Refresh-Token` header and an empty JSON body. The endpoint rotates the
//! refresh token on every call, so the new one must replace the old.
//!
//! The login (device) flow itself lives with the other subscription logins; this
//! module owns only the refresh the auth store runs when a credential expires.

use super::provider_http::{ProviderHttp, ProviderHttpMethod, ProviderHttpRequest};

/// The refresh endpoint (international gateway).
pub const CODEBUDDY_REFRESH_URL: &str = "https://www.codebuddy.ai/v2/plugin/auth/token/refresh";
/// The IDE user agent the plugin-auth endpoints expect.
pub const CODEBUDDY_USER_AGENT: &str = "IDE/2.63.2 CodeBuddy/2.63.2";
/// The refresh runs under the auth store's file lock, which a peer declares stale
/// after 10 seconds; the request must fit inside that window.
pub const REFRESH_TIMEOUT_MS: u64 = 8_000;
/// The lifetime assumed when the endpoint omits `expiresIn`: the observed token
/// lifetime (one year), so the credential is never treated as already-expired.
const FALLBACK_LIFETIME_SECS: i64 = 365 * 24 * 60 * 60;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodeBuddyCredentials {
    pub access: String,
    /// The rotated refresh token (the endpoint always issues a new one).
    pub refresh: Option<String>,
    /// Wall-clock epoch milliseconds. The endpoint reports `expiresIn`; when it
    /// omits one, a conservative one-year lifetime is assumed so the credential
    /// is never treated as already-expired.
    pub expires: i64,
}

/// Refresh one `CodeBuddy` access token. `previous_refresh` is the stored refresh
/// token, sent in the `X-Refresh-Token` header.
///
/// # Errors
///
/// Returns an error when the request fails, the endpoint answers non-2xx, or the
/// body carries no `accessToken`.
pub async fn refresh_codebuddy_token(
    http: &dyn ProviderHttp,
    previous_refresh: &str,
) -> Result<CodeBuddyCredentials, String> {
    let response = http
        .request(
            ProviderHttpRequest {
                method: ProviderHttpMethod::Post,
                url: CODEBUDDY_REFRESH_URL.to_string(),
                headers: vec![
                    ("Content-Type".to_string(), "application/json".to_string()),
                    ("Accept".to_string(), "application/json".to_string()),
                    ("User-Agent".to_string(), CODEBUDDY_USER_AGENT.to_string()),
                    ("X-Requested-With".to_string(), "XMLHttpRequest".to_string()),
                    ("X-Domain".to_string(), "www.codebuddy.ai".to_string()),
                    ("X-Refresh-Token".to_string(), previous_refresh.to_string()),
                    ("X-Auth-Refresh-Source".to_string(), "plugin".to_string()),
                    ("X-Product".to_string(), "SaaS".to_string()),
                ],
                body: Some("{}".to_string()),
                // The plugin endpoints answer in place; a redirect is a failure.
                follow_redirects: false,
            },
            REFRESH_TIMEOUT_MS,
        )
        .await?;
    if !(200..300).contains(&response.status) {
        return Err(format!(
            "CodeBuddy OAuth token refresh failed (HTTP {})",
            response.status
        ));
    }
    let body: serde_json::Value = serde_json::from_str(&response.body)
        .map_err(|error| format!("CodeBuddy OAuth refresh returned invalid JSON: {error}"))?;
    // The envelope is `{code, msg, data:{accessToken, refreshToken, expiresIn}}`.
    if let Some(code) = body.get("code").and_then(serde_json::Value::as_i64) {
        if code != 0 {
            let msg = body
                .get("msg")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("unknown");
            return Err(format!("CodeBuddy OAuth refresh error: {msg}"));
        }
    }
    let data = body.get("data").unwrap_or(&body);
    let access = data
        .get("accessToken")
        .and_then(serde_json::Value::as_str)
        .filter(|token| !token.trim().is_empty())
        .ok_or_else(|| "CodeBuddy OAuth refresh response is missing accessToken".to_string())?
        .to_string();
    let refresh = data
        .get("refreshToken")
        .and_then(serde_json::Value::as_str)
        .filter(|token| !token.trim().is_empty())
        .map(str::to_string);
    // The endpoint reports `expiresIn` seconds; a missing/zero value falls back
    // to a conservative one-year lifetime so the credential is never treated as
    // already-expired.
    let lifetime = data
        .get("expiresIn")
        .and_then(serde_json::Value::as_i64)
        .filter(|seconds| *seconds > 0)
        .unwrap_or(FALLBACK_LIFETIME_SECS);
    let expires = now_ms() + lifetime * 1000;
    Ok(CodeBuddyCredentials {
        access,
        refresh,
        expires,
    })
}

/// The current wall-clock time in epoch milliseconds.
#[allow(clippy::cast_possible_truncation)]
fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(i64::MAX, |elapsed| elapsed.as_millis() as i64)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::future::Future;
    use std::pin::Pin;

    use super::*;
    use crate::oauth::ProviderHttpResponse;

    /// A scripted transport (url -> response); unknown urls fail.
    struct ScriptedHttp(HashMap<String, ProviderHttpResponse>);

    impl ProviderHttp for ScriptedHttp {
        fn request(
            &self,
            request: ProviderHttpRequest,
            _timeout_ms: u64,
        ) -> Pin<Box<dyn Future<Output = Result<ProviderHttpResponse, String>> + Send + '_>>
        {
            let response = self.0.get(&request.url).cloned();
            Box::pin(async move { response.ok_or_else(|| format!("{} not scripted", request.url)) })
        }
    }

    fn http_with(status: u16, body: &str) -> ScriptedHttp {
        ScriptedHttp(
            [(
                CODEBUDDY_REFRESH_URL.to_string(),
                ProviderHttpResponse {
                    status,
                    body: body.to_string(),
                },
            )]
            .into_iter()
            .collect(),
        )
    }

    #[tokio::test]
    async fn refresh_reads_the_rotated_tokens() {
        let http = http_with(
            200,
            r#"{"code":0,"msg":"OK","data":{"accessToken":"new-access","refreshToken":"new-refresh","expiresIn":3600}}"#,
        );
        let credentials = refresh_codebuddy_token(&http, "old-refresh").await.unwrap();
        assert_eq!(credentials.access, "new-access");
        assert_eq!(credentials.refresh.as_deref(), Some("new-refresh"));
        assert!(credentials.expires > now_ms());
    }

    #[tokio::test]
    async fn refresh_survives_an_omitted_rotation() {
        let http = http_with(200, r#"{"code":0,"data":{"accessToken":"new-access"}}"#);
        let credentials = refresh_codebuddy_token(&http, "old-refresh").await.unwrap();
        assert_eq!(credentials.access, "new-access");
        assert_eq!(credentials.refresh, None);
        // The fallback lifetime keeps the credential usable.
        assert!(credentials.expires > now_ms());
    }

    #[tokio::test]
    async fn refresh_reports_a_nonzero_envelope_code() {
        let http = http_with(200, r#"{"code":14001,"msg":"refresh token expired"}"#);
        let error = refresh_codebuddy_token(&http, "stale").await.unwrap_err();
        assert!(error.contains("refresh token expired"), "{error}");
    }

    #[tokio::test]
    async fn refresh_reports_http_and_missing_token_failures() {
        let http = http_with(401, "unauthorized");
        assert!(refresh_codebuddy_token(&http, "stale")
            .await
            .unwrap_err()
            .contains("HTTP 401"));
        let http = http_with(200, r#"{"code":0,"data":{}}"#);
        assert!(refresh_codebuddy_token(&http, "stale")
            .await
            .unwrap_err()
            .contains("missing accessToken"));
    }
}
