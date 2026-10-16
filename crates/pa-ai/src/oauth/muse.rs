//! The Muse Code login: Meta's device flow mints a long-lived Muse API key.
//!
//! Meta returns a short-lived OIDC access token from `auth.meta.com`; that token
//! is only an intermediate credential. Prime Agent stores the durable key minted
//! by `api.meta.ai/muse-code/key`, so there is no refresh path for this provider.

use std::time::Duration;

use url::Url;

use super::provider_http::{
    ProviderHttp, ProviderHttpMethod, ProviderHttpRequest, ProviderHttpResponse,
};
use super::types::OAuthLoginUi;

pub const MUSE_CLIENT_ID: &str = "1031625952748946";
const DEVICE_CODE_URL: &str = "https://auth.meta.com/oidc/device/authorization/";
const TOKEN_URL: &str = "https://auth.meta.com/oidc/device/token/";
const KEY_MINT_URL: &str = "https://api.meta.ai/muse-code/key";
const API_VERSION: &str = "1.0.0";
const REQUEST_TIMEOUT_MS: u64 = 30_000;
const DEFAULT_POLL_INTERVAL_MS: u64 = 5000;
const MAX_DEVICE_CODE_LIFETIME_SECONDS: f64 = 24.0 * 60.0 * 60.0;
const MINT_MAX_ATTEMPTS: u8 = 3;
/// The cancel error the driving surface maps to the silent cancelled outcome.
pub const LOGIN_CANCELLED: &str = "Login cancelled";
const CANCEL_POLL_INTERVAL: Duration = Duration::from_millis(250);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MuseCredentials {
    /// The long-lived Muse Code key (`LLM|...`) stored as an API key.
    pub api_key: String,
}

/// The status plus the parsed JSON object (an empty object when the body is not one).
struct MuseResponse {
    ok: bool,
    status: u16,
    body: serde_json::Map<String, serde_json::Value>,
}

/// Run Meta's device flow and mint the durable Muse Code API key.
///
/// # Errors
///
/// Returns an error when the login is cancelled ([`LOGIN_CANCELLED`]), the device code expires, the
/// user denies the request, Meta returns an invalid response, or the Muse Code subscription cannot
/// mint an API key.
pub async fn login_muse(
    http: &dyn ProviderHttp,
    ui: &dyn OAuthLoginUi,
) -> Result<MuseCredentials, String> {
    if ui.is_cancelled() {
        return Err(LOGIN_CANCELLED.to_string());
    }
    let response = post_form(
        http,
        ui,
        DEVICE_CODE_URL,
        &[("client_id", MUSE_CLIENT_ID)],
        REQUEST_TIMEOUT_MS,
    )
    .await?;
    if !response.ok {
        return Err(request_failure("device authorization", &response));
    }
    let device_code = required_string(&response.body, "device_code")?;
    let user_code = required_string(&response.body, "user_code")?;
    if user_code.is_empty()
        || !user_code
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-')
    {
        return Err("Invalid Muse OAuth response field: user_code".to_string());
    }
    let verification_uri_raw = required_string(&response.body, "verification_uri")?;
    let url = verification_uri(&verification_uri_raw)?;
    let expires_in = positive_seconds(response.body.get("expires_in"))?;
    let deadline = std::time::Instant::now()
        .checked_add(Duration::from_secs(expires_in))
        .ok_or_else(|| "Invalid Muse OAuth response field: expires_in".to_string())?;
    let mut interval_ms = poll_interval_ms(response.body.get("interval"));
    ui.on_auth(&url, Some(&format!("Enter code: {user_code}")));

    while std::time::Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        cancel_aware_sleep(ui, remaining.min(Duration::from_millis(interval_ms))).await?;
        if std::time::Instant::now() >= deadline {
            break;
        }
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        // The remaining deadline (u128 millis) fits u64 for a bounded device lifetime.
        #[allow(clippy::cast_possible_truncation)]
        let token = post_form(
            http,
            ui,
            TOKEN_URL,
            &[
                ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
                ("device_code", device_code.as_str()),
                ("client_id", MUSE_CLIENT_ID),
            ],
            remaining.as_millis() as u64,
        )
        .await?;
        if token.ok {
            let access_token = required_string(&token.body, "access_token")?;
            return mint_api_key(http, ui, &access_token).await;
        }
        match token.body.get("error").and_then(serde_json::Value::as_str) {
            Some("authorization_pending") => {}
            Some("slow_down") => {
                interval_ms = interval_ms
                    .saturating_add(5000)
                    .max(poll_interval_ms(token.body.get("interval")));
            }
            Some("access_denied" | "authorization_denied") => {
                return Err("Muse device authorization was denied".to_string());
            }
            Some("expired_token") => {
                return Err("Muse device code expired; sign in again".to_string());
            }
            Some(_) | None => return Err(request_failure("device token polling", &token)),
        }
    }
    Err("Muse device code expired; sign in again".to_string())
}

async fn mint_api_key(
    http: &dyn ProviderHttp,
    ui: &dyn OAuthLoginUi,
    access_token: &str,
) -> Result<MuseCredentials, String> {
    for attempt in 1..=MINT_MAX_ATTEMPTS {
        if ui.is_cancelled() {
            return Err(LOGIN_CANCELLED.to_string());
        }
        let response = post_json_raw(http, ui, access_token).await?;
        if response.status == 429 && attempt < MINT_MAX_ATTEMPTS {
            // The key mint endpoint is account-scoped and may briefly rate-limit after device auth;
            // retrying here keeps the UI flow intact without storing the short-lived access token.
            cancel_aware_sleep(ui, Duration::from_secs(5 * u64::from(attempt))).await?;
            continue;
        }
        let response = parse_response(&response)?;
        if !response.ok {
            return Err(request_failure("key mint", &response));
        }
        return credentials_from_mint_response(&response.body);
    }
    Err("Muse Code key mint failed after retrying rate limits".to_string())
}

async fn post_form(
    http: &dyn ProviderHttp,
    ui: &dyn OAuthLoginUi,
    url: &str,
    fields: &[(&str, &str)],
    timeout_ms: u64,
) -> Result<MuseResponse, String> {
    let body = url::form_urlencoded::Serializer::new(String::new())
        .extend_pairs(fields.iter().map(|(key, value)| (*key, *value)))
        .finish();
    let response = send_request(
        http,
        ui,
        ProviderHttpRequest {
            method: ProviderHttpMethod::Post,
            url: url.to_string(),
            headers: form_headers(),
            body: Some(body),
            // The Meta auth edge intermittently 302s the device endpoints; following
            // lands on the same JSON API (verified live against auth.meta.com).
            follow_redirects: true,
        },
        timeout_ms.min(REQUEST_TIMEOUT_MS),
    )
    .await?;
    parse_response(&response)
}

async fn post_json_raw(
    http: &dyn ProviderHttp,
    ui: &dyn OAuthLoginUi,
    access_token: &str,
) -> Result<ProviderHttpResponse, String> {
    send_request(
        http,
        ui,
        ProviderHttpRequest {
            method: ProviderHttpMethod::Post,
            url: KEY_MINT_URL.to_string(),
            headers: vec![
                (
                    "Authorization".to_string(),
                    format!("Bearer {access_token}"),
                ),
                ("Accept".to_string(), "application/json".to_string()),
                ("Content-Type".to_string(), "application/json".to_string()),
                ("x-api-version".to_string(), API_VERSION.to_string()),
            ],
            body: Some(serde_json::json!({ "onboard": true }).to_string()),
            // The Meta auth edge intermittently 302s the device endpoints; following
            // lands on the same JSON API (verified live against auth.meta.com).
            follow_redirects: true,
        },
        REQUEST_TIMEOUT_MS,
    )
    .await
}

async fn send_request(
    http: &dyn ProviderHttp,
    ui: &dyn OAuthLoginUi,
    request: ProviderHttpRequest,
    timeout_ms: u64,
) -> Result<ProviderHttpResponse, String> {
    if ui.is_cancelled() {
        return Err(LOGIN_CANCELLED.to_string());
    }
    let response = http.request(request, timeout_ms).await.map_err(|message| {
        if message.contains("timed out") {
            "Muse OAuth request timed out. Try signing in again.".to_string()
        } else {
            // The underlying transport error (DNS, TLS, connection reset) is
            // the only clue to WHY Meta was unreachable - surface it instead
            // of the catch-all that hid the cause.
            format!("Muse OAuth request failed ({message}). Check your connection and try again.")
        }
    })?;
    if ui.is_cancelled() {
        return Err(LOGIN_CANCELLED.to_string());
    }
    Ok(response)
}

fn form_headers() -> Vec<(String, String)> {
    vec![
        ("Accept".to_string(), "application/json".to_string()),
        (
            "Content-Type".to_string(),
            "application/x-www-form-urlencoded".to_string(),
        ),
        ("x-api-version".to_string(), API_VERSION.to_string()),
    ]
}

fn parse_response(response: &ProviderHttpResponse) -> Result<MuseResponse, String> {
    let parsed: serde_json::Value = serde_json::from_str(&response.body).map_err(|_| {
        format!(
            "Muse OAuth returned invalid JSON (HTTP {})",
            response.status
        )
    })?;
    let body = match parsed {
        serde_json::Value::Object(map) => map,
        serde_json::Value::Null
        | serde_json::Value::Bool(_)
        | serde_json::Value::Number(_)
        | serde_json::Value::String(_)
        | serde_json::Value::Array(_) => serde_json::Map::new(),
    };
    Ok(MuseResponse {
        ok: response.ok(),
        status: response.status,
        body,
    })
}

fn request_failure(action: &str, response: &MuseResponse) -> String {
    format!("Muse OAuth {action} failed (HTTP {})", response.status)
}

fn credentials_from_mint_response(
    body: &serde_json::Map<String, serde_json::Value>,
) -> Result<MuseCredentials, String> {
    match body.get("is_subs_active") {
        Some(serde_json::Value::Bool(false)) => {
            return Err(
                "Muse Code subscription is inactive — activate it on muse.ai first".to_string(),
            );
        }
        Some(serde_json::Value::Bool(true)) => {}
        None
        | Some(
            serde_json::Value::Null
            | serde_json::Value::Number(_)
            | serde_json::Value::String(_)
            | serde_json::Value::Array(_)
            | serde_json::Value::Object(_),
        ) => return Err("Invalid Muse Code key response field: is_subs_active".to_string()),
    }
    if let Some(api_key) = optional_string(body, "api_key")? {
        if !api_key.starts_with("LLM|") {
            return Err("Invalid Muse Code key response field: api_key".to_string());
        }
        return Ok(MuseCredentials { api_key });
    }
    let action_url = optional_string(body, "action_url")?;
    let require_payment = match body.get("require_payment") {
        Some(serde_json::Value::Bool(value)) => *value,
        Some(
            serde_json::Value::Null
            | serde_json::Value::Number(_)
            | serde_json::Value::String(_)
            | serde_json::Value::Array(_)
            | serde_json::Value::Object(_),
        ) => return Err("Invalid Muse Code key response field: require_payment".to_string()),
        None => false,
    };
    if require_payment || action_url.is_some() {
        return match action_url {
            Some(url) => Err(format!(
                "Muse Code subscription requires payment — open {url}"
            )),
            None => Err("Muse Code subscription requires payment".to_string()),
        };
    }
    Err("Invalid Muse Code key response field: api_key".to_string())
}

fn required_string(
    body: &serde_json::Map<String, serde_json::Value>,
    field: &str,
) -> Result<String, String> {
    optional_string(body, field)?
        .ok_or_else(|| format!("Invalid Muse OAuth response field: {field}"))
}

fn optional_string(
    body: &serde_json::Map<String, serde_json::Value>,
    field: &str,
) -> Result<Option<String>, String> {
    match body.get(field) {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(serde_json::Value::String(value)) if !value.trim().is_empty() => {
            Ok(Some(value.clone()))
        }
        Some(
            serde_json::Value::String(_)
            | serde_json::Value::Bool(_)
            | serde_json::Value::Number(_)
            | serde_json::Value::Array(_)
            | serde_json::Value::Object(_),
        ) => Err(format!("Invalid Muse OAuth response field: {field}")),
    }
}

// The wire's expires_in is an integer second count read through JSON f64; u64 is Duration's unit.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn positive_seconds(value: Option<&serde_json::Value>) -> Result<u64, String> {
    let seconds = value
        .and_then(serde_json::Value::as_f64)
        .filter(|seconds| {
            seconds.is_finite() && *seconds > 0.0 && *seconds <= MAX_DEVICE_CODE_LIFETIME_SECONDS
        })
        .ok_or_else(|| "Invalid Muse OAuth response field: expires_in".to_string())?;
    Ok(seconds as u64)
}

// The device-flow interval is seconds on the wire; zero is accepted so tests and unusual providers
// can request an immediate first poll without sleeping.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn poll_interval_ms(value: Option<&serde_json::Value>) -> u64 {
    value
        .and_then(serde_json::Value::as_f64)
        .filter(|seconds| seconds.is_finite() && *seconds >= 0.0)
        .map_or(DEFAULT_POLL_INTERVAL_MS, |seconds| {
            (seconds * 1000.0) as u64
        })
}

/// A parsed https URL without credentials or control characters.
fn verification_uri(raw: &str) -> Result<String, String> {
    let url = Url::parse(raw)
        .map_err(|_| "Untrusted verification URI in Muse OAuth response".to_string())?;
    let untrusted = url.scheme() != "https"
        || !url.username().is_empty()
        || url.password().is_some()
        || raw
            .chars()
            .any(|c| c <= '\u{20}' || ('\u{7f}'..='\u{9f}').contains(&c));
    if untrusted {
        return Err("Untrusted verification URI in Muse OAuth response".to_string());
    }
    Ok(url.to_string())
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{HashMap, VecDeque};
    use std::future::Future;
    use std::pin::Pin;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};

    /// Queued responses per url (popped in order).
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

        fn request_count(&self) -> usize {
            self.requests.lock().unwrap().len()
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
            Box::pin(
                async move { response.ok_or_else(|| format!("{} was not scripted", request.url)) },
            )
        }
    }

    struct ScriptedUi {
        auth_url: Mutex<Option<String>>,
        auth_instructions: Mutex<Option<String>>,
        cancelled: Arc<AtomicBool>,
    }

    impl ScriptedUi {
        fn new() -> Self {
            ScriptedUi {
                auth_url: Mutex::new(None),
                auth_instructions: Mutex::new(None),
                cancelled: Arc::new(AtomicBool::new(false)),
            }
        }

        fn cancelled() -> Self {
            let ui = ScriptedUi::new();
            ui.cancelled.store(true, Ordering::Relaxed);
            ui
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
        }

        fn on_prompt(
            &self,
            _prompt: &super::super::types::OAuthPrompt,
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

    fn device_response() -> ProviderHttpResponse {
        ScriptedHttp::entry(
            200,
            r#"{"device_code":"dev-1","user_code":"META-1234","verification_uri":"https://auth.meta.com/oauth/device/","interval":0,"expires_in":600}"#,
        )
    }

    fn key_response(api_key: &str) -> ProviderHttpResponse {
        ScriptedHttp::entry(
            200,
            &serde_json::json!({
                "api_key": api_key,
                "is_subs_active": true,
                "subs_tier_name": "Muse Code",
                "user_email": "user@example.com",
                "require_payment": false,
            })
            .to_string(),
        )
    }

    fn happy_http() -> ScriptedHttp {
        ScriptedHttp::new()
            .queue(DEVICE_CODE_URL, vec![device_response()])
            .queue(
                TOKEN_URL,
                vec![
                    ScriptedHttp::entry(400, r#"{"error":"authorization_pending"}"#),
                    ScriptedHttp::entry(200, r#"{"access_token":"meta-access","expires_in":600}"#),
                ],
            )
            .queue(KEY_MINT_URL, vec![key_response("LLM|muse-key")])
    }

    #[tokio::test]
    async fn the_happy_flow_mints_the_long_lived_key() {
        let http = happy_http();
        let ui = ScriptedUi::new();
        let credentials = login_muse(&http, &ui).await.unwrap();
        assert_eq!(credentials.api_key, "LLM|muse-key");

        let (url, instructions) = ui.captured_auth();
        assert_eq!(url, "https://auth.meta.com/oauth/device/");
        assert_eq!(instructions.as_deref(), Some("Enter code: META-1234"));

        let device = &http.requests_for(DEVICE_CODE_URL)[0];
        assert_eq!(device.method, ProviderHttpMethod::Post);
        assert_eq!(device.body.as_deref(), Some("client_id=1031625952748946"));
        assert!(device
            .headers
            .iter()
            .any(|(name, value)| name == "x-api-version" && value == API_VERSION));

        let polls = http.requests_for(TOKEN_URL);
        assert_eq!(polls.len(), 2);
        let poll_body = polls[1].body.as_deref().unwrap();
        assert!(poll_body.contains("device_code=dev-1"));
        assert!(poll_body.contains("client_id=1031625952748946"));
        assert!(
            poll_body.contains("grant_type=urn%3Aietf%3Aparams%3Aoauth%3Agrant-type%3Adevice_code")
        );

        let mint = &http.requests_for(KEY_MINT_URL)[0];
        assert!(mint
            .headers
            .iter()
            .any(|(name, value)| name == "Authorization" && value == "Bearer meta-access"));
        assert_eq!(mint.body.as_deref(), Some(r#"{"onboard":true}"#));
    }

    #[tokio::test]
    async fn a_cancelled_surface_stops_before_the_first_request() {
        let http = happy_http();
        let ui = ScriptedUi::cancelled();
        let error = login_muse(&http, &ui).await.unwrap_err();
        assert_eq!(error, LOGIN_CANCELLED);
        assert_eq!(http.request_count(), 0);
    }

    #[tokio::test]
    async fn a_denied_authorization_surfaces_the_message() {
        let http = ScriptedHttp::new()
            .queue(DEVICE_CODE_URL, vec![device_response()])
            .queue(
                TOKEN_URL,
                vec![ScriptedHttp::entry(400, r#"{"error":"access_denied"}"#)],
            );
        let error = login_muse(&http, &ScriptedUi::new()).await.unwrap_err();
        assert_eq!(error, "Muse device authorization was denied");
    }

    #[tokio::test]
    async fn an_expired_authorization_surfaces_the_message() {
        let http = ScriptedHttp::new()
            .queue(DEVICE_CODE_URL, vec![device_response()])
            .queue(
                TOKEN_URL,
                vec![ScriptedHttp::entry(400, r#"{"error":"expired_token"}"#)],
            );
        let error = login_muse(&http, &ScriptedUi::new()).await.unwrap_err();
        assert_eq!(error, "Muse device code expired; sign in again");
    }

    #[tokio::test]
    async fn an_http_failure_surfaces_the_status_line() {
        let http = ScriptedHttp::new().queue(
            DEVICE_CODE_URL,
            vec![ScriptedHttp::entry(500, r#"{"error":"server_error"}"#)],
        );
        let error = login_muse(&http, &ScriptedUi::new()).await.unwrap_err();
        assert_eq!(error, "Muse OAuth device authorization failed (HTTP 500)");
    }

    #[tokio::test(start_paused = true)]
    async fn a_rate_limited_key_mint_retries_before_succeeding() {
        let http = ScriptedHttp::new()
            .queue(DEVICE_CODE_URL, vec![device_response()])
            .queue(
                TOKEN_URL,
                vec![ScriptedHttp::entry(
                    200,
                    r#"{"access_token":"meta-access"}"#,
                )],
            )
            .queue(
                KEY_MINT_URL,
                vec![
                    ScriptedHttp::entry(429, r#"{"error":"rate_limited"}"#),
                    key_response("LLM|after-retry"),
                ],
            );
        let credentials = login_muse(&http, &ScriptedUi::new()).await.unwrap();
        assert_eq!(credentials.api_key, "LLM|after-retry");
        assert_eq!(http.requests_for(KEY_MINT_URL).len(), 2);
    }

    #[tokio::test]
    async fn an_inactive_subscription_stops_before_storing_a_key() {
        let http = ScriptedHttp::new()
            .queue(DEVICE_CODE_URL, vec![device_response()])
            .queue(
                TOKEN_URL,
                vec![ScriptedHttp::entry(
                    200,
                    r#"{"access_token":"meta-access"}"#,
                )],
            )
            .queue(
                KEY_MINT_URL,
                vec![ScriptedHttp::entry(
                    200,
                    r#"{"is_subs_active":false,"api_key":"LLM|blocked"}"#,
                )],
            );
        let error = login_muse(&http, &ScriptedUi::new()).await.unwrap_err();
        assert_eq!(
            error,
            "Muse Code subscription is inactive — activate it on muse.ai first"
        );
    }

    #[tokio::test]
    async fn a_payment_response_names_the_action_url() {
        let http = ScriptedHttp::new()
            .queue(DEVICE_CODE_URL, vec![device_response()])
            .queue(
                TOKEN_URL,
                vec![ScriptedHttp::entry(
                    200,
                    r#"{"access_token":"meta-access"}"#,
                )],
            )
            .queue(
                KEY_MINT_URL,
                vec![ScriptedHttp::entry(
                    200,
                    r#"{"is_subs_active":true,"require_payment":true,"action_url":"https://muse.ai/subscribe"}"#,
                )],
            );
        let error = login_muse(&http, &ScriptedUi::new()).await.unwrap_err();
        assert_eq!(
            error,
            "Muse Code subscription requires payment — open https://muse.ai/subscribe"
        );
    }

    #[test]
    fn a_transport_failure_names_the_underlying_error() {
        // The catch-all must keep the transport's own clue (DNS vs TLS vs
        // reset): the ScriptedHttp "not scripted" error stands in for it.
        let http = ScriptedHttp::new();
        let outcome = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime")
            .block_on(login_muse(&http, &ScriptedUi::new()));
        let error = outcome.expect_err("an unscripted flow must fail");
        assert!(
            error.contains("Muse OAuth request failed") && error.contains("was not scripted"),
            "the transport clue must survive: {error}"
        );
    }
}
