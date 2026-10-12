//! The `CodeBuddy` (codebuddy.ai) OAuth login and token refresh.
//!
//! `CodeBuddy`'s IDE/CLI plan authenticates with a browser-driven device flow
//! (`POST /v2/plugin/auth/state` -> user approves -> `GET /v2/plugin/auth/token`)
//! that yields an access token plus a long-lived refresh token. The access
//! token expires (~1 year, but revocable), so a stored credential refreshes at
//! `POST /v2/plugin/auth/token/refresh` with the refresh token in the
//! `X-Refresh-Token` header and an empty JSON body. The endpoint rotates the
//! refresh token on every call, so the new one must replace the old when it is
//! present.

use std::time::Duration;

use url::Url;

use super::provider_http::{ProviderHttp, ProviderHttpMethod, ProviderHttpRequest};
use super::types::OAuthLoginUi;

/// The device-flow state endpoint (international gateway).
pub const CODEBUDDY_STATE_URL: &str = "https://www.codebuddy.ai/v2/plugin/auth/state?platform=ide";
/// The device-flow token polling endpoint (international gateway).
pub const CODEBUDDY_TOKEN_URL: &str = "https://www.codebuddy.ai/v2/plugin/auth/token";
/// The refresh endpoint (international gateway).
pub const CODEBUDDY_REFRESH_URL: &str = "https://www.codebuddy.ai/v2/plugin/auth/token/refresh";
/// The IDE user agent the plugin-auth endpoints expect.
pub const CODEBUDDY_USER_AGENT: &str = "IDE/2.63.2 CodeBuddy/2.63.2";
/// One interactive request should fail promptly instead of pinning the auth panel.
pub const REQUEST_TIMEOUT_MS: u64 = 30_000;
/// The refresh runs under the auth store's file lock, which a peer declares stale
/// after 10 seconds; the request must fit inside that window.
pub const REFRESH_TIMEOUT_MS: u64 = 8_000;
/// The cancel error the driving surface maps to the silent cancelled outcome.
pub const LOGIN_CANCELLED: &str = "Login cancelled";
/// `CodeBuddy`'s pending code for an unapproved browser login.
const PENDING_CODE: i64 = 11217;
/// The state endpoint carries no device-code lifetime, so the client bounds
/// the poll to keep `/login` from waiting forever after a lost browser flow.
const LOGIN_EXPIRES_IN_SECS: u64 = 10 * 60;
/// The endpoint contract says to poll every five seconds unless it starts
/// returning an explicit interval.
const DEFAULT_POLL_INTERVAL_MS: u64 = 5_000;
/// The cooperative cancel flag ends the wait mid-sleep.
const CANCEL_POLL_INTERVAL: Duration = Duration::from_millis(250);
/// The lifetime assumed when the refresh endpoint omits `expiresIn`: the
/// observed token lifetime (one year), so the credential is never treated as
/// already-expired.
const FALLBACK_LIFETIME_SECS: i64 = 365 * 24 * 60 * 60;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodeBuddyCredentials {
    pub access: String,
    /// The login flow requires a refresh token; the refresh endpoint may omit a
    /// rotation, in which case the caller keeps the prior stored refresh token.
    pub refresh: Option<String>,
    /// Wall-clock epoch milliseconds. The login endpoint reports `expiresIn`;
    /// the refresh endpoint may omit it, in which case a conservative one-year
    /// lifetime is assumed so the credential is never treated as already-expired.
    pub expires: i64,
}

/// Run the `CodeBuddy` IDE device flow and return the OAuth credential to store.
///
/// # Errors
///
/// Returns an error when the login is cancelled ([`LOGIN_CANCELLED`]), the state
/// request fails, the browser authorization is denied or expires, or the token
/// response fails validation.
pub async fn login_codebuddy(
    http: &dyn ProviderHttp,
    ui: &dyn OAuthLoginUi,
) -> Result<CodeBuddyCredentials, String> {
    let state_response = request_json(
        http,
        ui,
        ProviderHttpRequest {
            method: ProviderHttpMethod::Post,
            url: CODEBUDDY_STATE_URL.to_string(),
            headers: plugin_headers(PluginAuthRequest::State),
            body: Some("{}".to_string()),
            // The plugin endpoints answer in place; a redirect is a failure.
            follow_redirects: false,
        },
        REQUEST_TIMEOUT_MS,
    )
    .await?;
    if !state_response.ok {
        return Err(request_failure("device authorization", &state_response));
    }
    let code = envelope_code(&state_response.body, "device authorization")?;
    if code != 0 {
        return Err(envelope_failure(
            "device authorization",
            code,
            &state_response.body,
        ));
    }
    let data = required_data(&state_response.body, "device authorization")?;
    let state = required_string(data, "state", "device authorization")?;
    validate_state(&state)?;
    let auth_url = trusted_auth_url(
        &required_string(data, "authUrl", "device authorization")?,
        &state,
    )?;
    let mut interval_ms =
        response_interval(&state_response.body).unwrap_or(DEFAULT_POLL_INTERVAL_MS);
    if ui.is_cancelled() {
        return Err(LOGIN_CANCELLED.to_string());
    }
    ui.on_auth(&auth_url, None);
    if ui.is_cancelled() {
        return Err(LOGIN_CANCELLED.to_string());
    }

    let deadline = std::time::Instant::now() + Duration::from_secs(LOGIN_EXPIRES_IN_SECS);
    while std::time::Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        cancel_aware_sleep(ui, remaining.min(Duration::from_millis(interval_ms))).await?;
        if std::time::Instant::now() >= deadline {
            break;
        }
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        // The login deadline is ten minutes, so the remaining millisecond count fits `u64`.
        #[allow(clippy::cast_possible_truncation)]
        let token_response = request_json(
            http,
            ui,
            ProviderHttpRequest {
                method: ProviderHttpMethod::Get,
                url: token_url(&state),
                headers: plugin_headers(PluginAuthRequest::TokenPoll),
                body: None,
                // The plugin endpoints answer in place; a redirect is a failure.
                follow_redirects: false,
            },
            remaining.as_millis() as u64,
        )
        .await?;
        if !token_response.ok {
            return Err(request_failure("device token polling", &token_response));
        }
        let code = envelope_code(&token_response.body, "device token polling")?;
        if code == PENDING_CODE {
            if let Some(next_interval_ms) = response_interval(&token_response.body) {
                interval_ms = next_interval_ms;
            }
            continue;
        }
        if code == 0 {
            let data = required_data(&token_response.body, "device token polling")?;
            return credentials_from_login_data(data);
        }
        return Err(envelope_failure(
            "device token polling",
            code,
            &token_response.body,
        ));
    }
    Err("CodeBuddy device code expired; sign in again".to_string())
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
    let expires = now_ms().saturating_add(lifetime.saturating_mul(1000));
    Ok(CodeBuddyCredentials {
        access,
        refresh,
        expires,
    })
}

struct CodeBuddyResponse {
    ok: bool,
    status: u16,
    body: serde_json::Map<String, serde_json::Value>,
}

#[derive(Clone, Copy)]
enum PluginAuthRequest {
    State,
    TokenPoll,
}

fn plugin_headers(request: PluginAuthRequest) -> Vec<(String, String)> {
    let mut headers = vec![
        ("Accept".to_string(), "application/json".to_string()),
        ("User-Agent".to_string(), CODEBUDDY_USER_AGENT.to_string()),
        ("X-Requested-With".to_string(), "XMLHttpRequest".to_string()),
        ("X-Domain".to_string(), "www.codebuddy.ai".to_string()),
        ("X-No-Authorization".to_string(), "true".to_string()),
        ("X-No-User-Id".to_string(), "true".to_string()),
        ("X-Product".to_string(), "SaaS".to_string()),
    ];
    match request {
        PluginAuthRequest::State => {
            headers.insert(
                0,
                ("Content-Type".to_string(), "application/json".to_string()),
            );
        }
        PluginAuthRequest::TokenPoll => {
            headers.push(("X-No-Enterprise-Id".to_string(), "true".to_string()));
            headers.push(("X-No-Department-Info".to_string(), "true".to_string()));
        }
    }
    headers
}

async fn request_json(
    http: &dyn ProviderHttp,
    ui: &dyn OAuthLoginUi,
    request: ProviderHttpRequest,
    timeout_ms: u64,
) -> Result<CodeBuddyResponse, String> {
    if ui.is_cancelled() {
        return Err(LOGIN_CANCELLED.to_string());
    }
    let response = http
        .request(request, timeout_ms.min(REQUEST_TIMEOUT_MS))
        .await
        .map_err(|message| {
            if message.contains("timed out") {
                "CodeBuddy OAuth request timed out. Try signing in again.".to_string()
            } else {
                "CodeBuddy OAuth request failed. Check your connection and try again.".to_string()
            }
        })?;
    if ui.is_cancelled() {
        return Err(LOGIN_CANCELLED.to_string());
    }
    let parsed: serde_json::Value = serde_json::from_str(&response.body).map_err(|_| {
        format!(
            "CodeBuddy OAuth returned invalid JSON (HTTP {})",
            response.status
        )
    })?;
    let serde_json::Value::Object(body) = parsed else {
        return Err(format!(
            "CodeBuddy OAuth returned non-object JSON (HTTP {})",
            response.status
        ));
    };
    Ok(CodeBuddyResponse {
        ok: response.ok(),
        status: response.status,
        body,
    })
}

fn request_failure(action: &str, response: &CodeBuddyResponse) -> String {
    format!("CodeBuddy OAuth {action} failed (HTTP {})", response.status)
}

fn envelope_code(
    body: &serde_json::Map<String, serde_json::Value>,
    action: &str,
) -> Result<i64, String> {
    body.get("code")
        .and_then(serde_json::Value::as_i64)
        .ok_or_else(|| format!("Invalid CodeBuddy OAuth {action} response field: code"))
}

fn envelope_failure(
    action: &str,
    code: i64,
    body: &serde_json::Map<String, serde_json::Value>,
) -> String {
    let msg = body
        .get("msg")
        .and_then(serde_json::Value::as_str)
        .filter(|message| !message.trim().is_empty())
        .unwrap_or("unknown");
    let lower = msg.to_ascii_lowercase();
    if lower.contains("denied") || lower.contains("declined") || lower.contains("reject") {
        return "CodeBuddy device authorization was denied".to_string();
    }
    if lower.contains("expired") {
        return "CodeBuddy device code expired; sign in again".to_string();
    }
    format!("CodeBuddy OAuth {action} failed (code {code}): {msg}")
}

fn required_data<'a>(
    body: &'a serde_json::Map<String, serde_json::Value>,
    action: &str,
) -> Result<&'a serde_json::Map<String, serde_json::Value>, String> {
    body.get("data")
        .and_then(serde_json::Value::as_object)
        .ok_or_else(|| format!("Invalid CodeBuddy OAuth {action} response field: data"))
}

fn required_string(
    body: &serde_json::Map<String, serde_json::Value>,
    field: &str,
    action: &str,
) -> Result<String, String> {
    body.get(field)
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(str::to_string)
        .ok_or_else(|| format!("Invalid CodeBuddy OAuth {action} response field: {field}"))
}

fn credentials_from_login_data(
    data: &serde_json::Map<String, serde_json::Value>,
) -> Result<CodeBuddyCredentials, String> {
    let access = required_string(data, "accessToken", "device token polling")?;
    let refresh = required_string(data, "refreshToken", "device token polling")?;
    let expires_in = data
        .get("expiresIn")
        .and_then(serde_json::Value::as_i64)
        .filter(|seconds| *seconds > 0)
        .ok_or_else(|| {
            "Invalid CodeBuddy OAuth device token polling response field: expiresIn".to_string()
        })?;
    Ok(CodeBuddyCredentials {
        access,
        refresh: Some(refresh),
        expires: now_ms().saturating_add(expires_in.saturating_mul(1000)),
    })
}

fn response_interval(body: &serde_json::Map<String, serde_json::Value>) -> Option<u64> {
    poll_interval_ms(body.get("interval")).or_else(|| {
        body.get("data")
            .and_then(serde_json::Value::as_object)
            .and_then(|data| poll_interval_ms(data.get("interval")))
    })
}

fn poll_interval_ms(value: Option<&serde_json::Value>) -> Option<u64> {
    let seconds = value
        .and_then(serde_json::Value::as_f64)
        .filter(|seconds| seconds.is_finite() && *seconds > 0.0)?;
    // The wire's interval is a positive f64 second count; u64 ms is the poll's unit.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    Some(((seconds * 1000.0) as u64).max(1000))
}

fn token_url(state: &str) -> String {
    let query = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("state", state)
        .finish();
    format!("{CODEBUDDY_TOKEN_URL}?{query}")
}

fn validate_state(state: &str) -> Result<(), String> {
    let valid = state.len() == 36
        && state.chars().enumerate().all(|(index, character)| {
            if matches!(index, 8 | 13 | 18 | 23) {
                character == '-'
            } else {
                character.is_ascii_hexdigit()
            }
        });
    if valid {
        Ok(())
    } else {
        Err("Invalid CodeBuddy OAuth device authorization response field: state".to_string())
    }
}

/// A parsed `CodeBuddy` HTTPS URL without credentials or control characters.
fn trusted_auth_url(raw: &str, state: &str) -> Result<String, String> {
    let url = Url::parse(raw)
        .map_err(|_| "Untrusted auth URL in CodeBuddy OAuth response".to_string())?;
    let has_control = raw
        .chars()
        .any(|c| c <= '\u{20}' || ('\u{7f}'..='\u{9f}').contains(&c));
    if url.scheme() != "https"
        || url.host_str() != Some("www.codebuddy.ai")
        || url.path() != "/login"
        || !url.username().is_empty()
        || url.password().is_some()
        || has_control
    {
        return Err("Untrusted auth URL in CodeBuddy OAuth response".to_string());
    }
    let mut state_matches = false;
    let mut platform_matches = false;
    for (key, value) in url.query_pairs() {
        if key == "state" && value == state {
            state_matches = true;
        }
        if key == "platform" && value == "ide" {
            platform_matches = true;
        }
    }
    if state_matches && platform_matches {
        Ok(url.to_string())
    } else {
        Err("Untrusted auth URL in CodeBuddy OAuth response".to_string())
    }
}

/// The cooperative cancel flag ends the wait mid-sleep.
async fn cancel_aware_sleep(ui: &dyn OAuthLoginUi, total: Duration) -> Result<(), String> {
    let deadline = std::time::Instant::now() + total;
    loop {
        if ui.is_cancelled() {
            return Err(LOGIN_CANCELLED.to_string());
        }
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        if remaining.is_zero() {
            return Ok(());
        }
        tokio::time::sleep(remaining.min(CANCEL_POLL_INTERVAL)).await;
    }
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
    use std::collections::{HashMap, VecDeque};
    use std::future::Future;
    use std::pin::Pin;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};

    use super::*;
    use crate::oauth::{OAuthPrompt, ProviderHttpResponse};

    /// A scripted transport: queued responses per url (popped in order); unknown urls fail.
    struct ScriptedHttp {
        queued: Mutex<HashMap<String, VecDeque<ProviderHttpResponse>>>,
        requests: Mutex<Vec<ProviderHttpRequest>>,
    }

    impl ScriptedHttp {
        fn new() -> Self {
            ScriptedHttp {
                queued: Mutex::new(HashMap::new()),
                requests: Mutex::new(Vec::new()),
            }
        }

        fn entry(status: u16, body: &str) -> ProviderHttpResponse {
            ProviderHttpResponse {
                status,
                body: body.to_string(),
            }
        }

        fn queue(self, url: &str, responses: Vec<ProviderHttpResponse>) -> Self {
            self.queued.lock().unwrap().insert(
                url.to_string(),
                responses.into_iter().collect::<VecDeque<_>>(),
            );
            self
        }

        fn requests_for(&self, url: &str) -> Vec<ProviderHttpRequest> {
            self.requests
                .lock()
                .unwrap()
                .iter()
                .filter(|request| request.url == url)
                .cloned()
                .collect()
        }
    }

    impl ProviderHttp for ScriptedHttp {
        fn request(
            &self,
            request: ProviderHttpRequest,
            _timeout_ms: u64,
        ) -> Pin<Box<dyn Future<Output = Result<ProviderHttpResponse, String>> + Send + '_>>
        {
            self.requests.lock().unwrap().push(request.clone());
            let response = self
                .queued
                .lock()
                .unwrap()
                .get_mut(&request.url)
                .and_then(std::collections::VecDeque::pop_front);
            Box::pin(async move { response.ok_or_else(|| format!("{} not scripted", request.url)) })
        }
    }

    struct ScriptedUi {
        auth_url: Mutex<Option<String>>,
        auth_instructions: Mutex<Option<String>>,
        cancelled: Arc<AtomicBool>,
        cancel_on_auth: bool,
    }

    impl ScriptedUi {
        fn new() -> Self {
            ScriptedUi {
                auth_url: Mutex::new(None),
                auth_instructions: Mutex::new(None),
                cancelled: Arc::new(AtomicBool::new(false)),
                cancel_on_auth: false,
            }
        }

        fn cancel_on_auth(mut self) -> Self {
            self.cancel_on_auth = true;
            self
        }

        fn captured_auth(&self) -> (String, Option<String>) {
            (
                self.auth_url.lock().unwrap().clone().expect("the auth url"),
                self.auth_instructions.lock().unwrap().clone(),
            )
        }
    }

    impl OAuthLoginUi for ScriptedUi {
        fn on_auth(&self, url: &str, instructions: Option<&str>) {
            *self.auth_url.lock().unwrap() = Some(url.to_string());
            *self.auth_instructions.lock().unwrap() = instructions.map(str::to_string);
            if self.cancel_on_auth {
                self.cancelled.store(true, Ordering::Relaxed);
            }
        }

        fn on_prompt(
            &self,
            _prompt: &OAuthPrompt,
        ) -> Pin<Box<dyn Future<Output = Option<String>> + Send + '_>> {
            Box::pin(std::future::pending())
        }

        fn on_progress(&self, _message: &str) {}

        fn on_manual_code_input(
            &self,
        ) -> Option<Pin<Box<dyn Future<Output = Option<String>> + Send + '_>>> {
            None
        }

        fn is_cancelled(&self) -> bool {
            self.cancelled.load(Ordering::Relaxed)
        }
    }

    fn state_body() -> String {
        serde_json::json!({
            "code": 0,
            "msg": "OK",
            "data": {
                "state": "28117873-7c32-49dc-87ec-a161c92b209a",
                "authUrl": "https://www.codebuddy.ai/login?platform=ide&state=28117873-7c32-49dc-87ec-a161c92b209a",
            },
        })
        .to_string()
    }

    fn token_poll_url() -> String {
        token_url("28117873-7c32-49dc-87ec-a161c92b209a")
    }

    fn happy_http() -> ScriptedHttp {
        ScriptedHttp::new()
            .queue(CODEBUDDY_STATE_URL, vec![ScriptedHttp::entry(200, &state_body())])
            .queue(
                &token_poll_url(),
                vec![
                    ScriptedHttp::entry(200, r#"{"code":11217,"msg":"PENDING"}"#),
                    ScriptedHttp::entry(
                        200,
                        r#"{"code":0,"msg":"OK","data":{"accessToken":"codebuddy-access","refreshToken":"codebuddy-refresh","expiresIn":3600}}"#,
                    ),
                ],
            )
    }

    #[tokio::test(start_paused = true)]
    async fn login_presents_the_auth_url_and_reads_the_tokens() {
        let http = happy_http();
        let ui = ScriptedUi::new();
        let before_ms = now_ms();
        let credentials = login_codebuddy(&http, &ui).await.unwrap();
        assert_eq!(credentials.access, "codebuddy-access");
        assert_eq!(credentials.refresh.as_deref(), Some("codebuddy-refresh"));
        assert!(
            credentials.expires >= before_ms + 3_600_000,
            "the expiry is the provider lifetime without an OAuth skew"
        );
        let (url, instructions) = ui.captured_auth();
        assert_eq!(
            url,
            "https://www.codebuddy.ai/login?platform=ide&state=28117873-7c32-49dc-87ec-a161c92b209a"
        );
        assert_eq!(instructions, None);
        let state_request = &http.requests_for(CODEBUDDY_STATE_URL)[0];
        assert_eq!(state_request.method, ProviderHttpMethod::Post);
        assert_eq!(state_request.body.as_deref(), Some("{}"));
        assert!(state_request
            .headers
            .iter()
            .any(|(name, value)| { name == "X-No-Authorization" && value == "true" }));
        let poll_requests = http.requests_for(&token_poll_url());
        assert_eq!(poll_requests.len(), 2);
        assert_eq!(poll_requests[0].method, ProviderHttpMethod::Get);
        assert_eq!(poll_requests[0].body, None);
        assert!(poll_requests[0]
            .headers
            .iter()
            .any(|(name, value)| { name == "X-No-Enterprise-Id" && value == "true" }));
    }

    #[tokio::test]
    async fn a_cancelled_surface_ends_after_presenting_the_url() {
        let http = happy_http();
        let ui = ScriptedUi::new().cancel_on_auth();
        let error = login_codebuddy(&http, &ui).await.unwrap_err();
        assert_eq!(error, LOGIN_CANCELLED);
        assert_eq!(
            http.requests_for(&token_poll_url()),
            Vec::<ProviderHttpRequest>::new(),
            "a cancelled panel does not poll for tokens"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_denied_authorization_surfaces_the_device_message() {
        let http = ScriptedHttp::new()
            .queue(
                CODEBUDDY_STATE_URL,
                vec![ScriptedHttp::entry(200, &state_body())],
            )
            .queue(
                &token_poll_url(),
                vec![ScriptedHttp::entry(
                    200,
                    r#"{"code":14001,"msg":"authorization denied"}"#,
                )],
            );
        let error = login_codebuddy(&http, &ScriptedUi::new())
            .await
            .unwrap_err();
        assert_eq!(error, "CodeBuddy device authorization was denied");
    }

    #[tokio::test(start_paused = true)]
    async fn an_expired_authorization_surfaces_the_device_message() {
        let http = ScriptedHttp::new()
            .queue(
                CODEBUDDY_STATE_URL,
                vec![ScriptedHttp::entry(200, &state_body())],
            )
            .queue(
                &token_poll_url(),
                vec![ScriptedHttp::entry(
                    200,
                    r#"{"code":14002,"msg":"state expired"}"#,
                )],
            );
        let error = login_codebuddy(&http, &ScriptedUi::new())
            .await
            .unwrap_err();
        assert_eq!(error, "CodeBuddy device code expired; sign in again");
    }

    #[tokio::test]
    async fn an_http_failure_surfaces_the_status() {
        let http = ScriptedHttp::new().queue(
            CODEBUDDY_STATE_URL,
            vec![ScriptedHttp::entry(
                503,
                r#"{"code":14003,"msg":"too many requests"}"#,
            )],
        );
        let error = login_codebuddy(&http, &ScriptedUi::new())
            .await
            .unwrap_err();
        assert_eq!(
            error,
            "CodeBuddy OAuth device authorization failed (HTTP 503)"
        );
    }

    #[tokio::test]
    async fn an_untrusted_auth_url_fails_the_flow() {
        let body = serde_json::json!({
            "code": 0,
            "msg": "OK",
            "data": {
                "state": "28117873-7c32-49dc-87ec-a161c92b209a",
                "authUrl": "https://evil.example/login?platform=ide&state=28117873-7c32-49dc-87ec-a161c92b209a",
            },
        });
        let http = ScriptedHttp::new().queue(
            CODEBUDDY_STATE_URL,
            vec![ScriptedHttp::entry(200, &body.to_string())],
        );
        let error = login_codebuddy(&http, &ScriptedUi::new())
            .await
            .unwrap_err();
        assert_eq!(error, "Untrusted auth URL in CodeBuddy OAuth response");
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

    /// A scripted transport (url -> response); unknown urls fail.
    struct RefreshHttp(HashMap<String, ProviderHttpResponse>);

    impl ProviderHttp for RefreshHttp {
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

    fn http_with(status: u16, body: &str) -> RefreshHttp {
        RefreshHttp(
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
}
