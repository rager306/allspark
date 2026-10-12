//! The Genspark CLI device login.
//!
//! Genspark's CLI returns a long-lived `gsk` API key instead of a refreshable
//! OAuth token. The flow still behaves like the other device-code logins: get a
//! browser URL, present it through the auth panel, then poll until the browser
//! approval yields the key. Keeping it in this OAuth module lets `/login` share
//! the same transport seam and cancel handling as xAI and Copilot.

use std::time::Duration;

use url::Url;

use super::provider_http::{ProviderHttp, ProviderHttpMethod, ProviderHttpRequest};
use super::types::OAuthLoginUi;

const DEVICE_CODE_URL: &str = "https://www.genspark.ai/api/cli_auth/device_code";
const TOKEN_URL: &str = "https://www.genspark.ai/api/cli_auth/token";
const USER_AGENT: &str = "gsk/1.18.34";
pub const REQUEST_TIMEOUT_MS: u64 = 30_000;
/// The cancel error the driving surface maps to the silent cancelled outcome.
pub const LOGIN_CANCELLED: &str = "Login cancelled";
const CANCEL_POLL_INTERVAL: Duration = Duration::from_millis(250);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GensparkCredentials {
    pub api_key: String,
}

/// The status plus the parsed body (an empty object when the body is not one).
struct GensparkResponse {
    ok: bool,
    status: u16,
    body: serde_json::Map<String, serde_json::Value>,
}

/// Run the Genspark CLI device flow and return the approved API key.
///
/// # Errors
///
/// Returns an error when the login is cancelled ([`LOGIN_CANCELLED`]), the
/// device authorization response is invalid, the code expires or is denied, or
/// token polling fails.
pub async fn login_genspark(
    http: &dyn ProviderHttp,
    ui: &dyn OAuthLoginUi,
) -> Result<GensparkCredentials, String> {
    if ui.is_cancelled() {
        return Err(LOGIN_CANCELLED.to_string());
    }
    let response = send_json_request(
        http,
        ui,
        ProviderHttpMethod::Post,
        DEVICE_CODE_URL.to_string(),
        Some("{}".to_string()),
        REQUEST_TIMEOUT_MS,
    )
    .await?;
    if !response.ok {
        return Err(request_failure("device authorization", &response));
    }
    let device_code = required_string(&response.body, "device_code")?;
    let auth_url = trusted_url(&required_string(&response.body, "auth_url")?)?;
    let expires_in = positive_duration(response.body.get("expires_in"), "expires_in")?;
    let poll_interval = positive_duration(response.body.get("poll_interval"), "poll_interval")?;
    let deadline = std::time::Instant::now()
        .checked_add(expires_in)
        .ok_or_else(|| "Invalid Genspark login response field: expires_in".to_string())?;
    ui.on_auth(&auth_url, None);

    while std::time::Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        cancel_aware_sleep(ui, remaining.min(poll_interval)).await?;
        if std::time::Instant::now() >= deadline {
            break;
        }
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        let response = send_json_request(
            http,
            ui,
            ProviderHttpMethod::Get,
            token_url(&device_code)?,
            None,
            duration_millis_u64(remaining),
        )
        .await?;
        if !response.ok {
            return Err(request_failure("token polling", &response));
        }
        match response
            .body
            .get("status")
            .and_then(serde_json::Value::as_str)
        {
            Some("pending") => {}
            Some("approved") => {
                return Ok(GensparkCredentials {
                    api_key: required_string(&response.body, "api_key")?,
                });
            }
            Some("denied") => {
                return Err("Genspark device authorization was denied".to_string());
            }
            Some("expired") => {
                return Err("Genspark device code expired; sign in again".to_string());
            }
            Some(status) => {
                return Err(format!("Genspark login stopped with status: {status}"));
            }
            None => {
                if let Some(error) = response
                    .body
                    .get("error")
                    .and_then(serde_json::Value::as_str)
                {
                    return Err(format!("Genspark token polling failed: {error}"));
                }
                return Err("Invalid Genspark login response field: status".to_string());
            }
        }
    }
    Err("Genspark device code expired; sign in again".to_string())
}

async fn send_json_request(
    http: &dyn ProviderHttp,
    ui: &dyn OAuthLoginUi,
    method: ProviderHttpMethod,
    url: String,
    body: Option<String>,
    timeout_ms: u64,
) -> Result<GensparkResponse, String> {
    if ui.is_cancelled() {
        return Err(LOGIN_CANCELLED.to_string());
    }
    let mut headers = vec![
        ("Accept".to_string(), "application/json".to_string()),
        ("User-Agent".to_string(), USER_AGENT.to_string()),
    ];
    if body.is_some() {
        headers.push(("Content-Type".to_string(), "application/json".to_string()));
    }
    let response = http
        .request(
            ProviderHttpRequest {
                method,
                url,
                headers,
                body,
                // The CLI auth endpoints answer in place; a redirect would be a different flow.
                follow_redirects: false,
            },
            timeout_ms.min(REQUEST_TIMEOUT_MS),
        )
        .await
        .map_err(|message| {
            if message.contains("timed out") {
                "Genspark login request timed out. Try signing in again.".to_string()
            } else {
                "Genspark login request failed. Check your connection and try again.".to_string()
            }
        })?;
    if ui.is_cancelled() {
        return Err(LOGIN_CANCELLED.to_string());
    }
    let parsed: serde_json::Value = serde_json::from_str(&response.body).map_err(|_| {
        format!(
            "Genspark login returned invalid JSON (HTTP {})",
            response.status
        )
    })?;
    let body = match parsed {
        serde_json::Value::Object(map) => map,
        _ => serde_json::Map::new(),
    };
    Ok(GensparkResponse {
        ok: response.ok(),
        status: response.status,
        body,
    })
}

fn request_failure(action: &str, response: &GensparkResponse) -> String {
    format!("Genspark {action} failed (HTTP {})", response.status)
}

fn token_url(device_code: &str) -> Result<String, String> {
    let mut url =
        Url::parse(TOKEN_URL).map_err(|_| "Invalid Genspark token endpoint".to_string())?;
    url.query_pairs_mut().append_pair("code", device_code);
    Ok(url.to_string())
}

fn required_string(
    body: &serde_json::Map<String, serde_json::Value>,
    field: &str,
) -> Result<String, String> {
    body.get(field)
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(str::to_string)
        .ok_or_else(|| format!("Invalid Genspark login response field: {field}"))
}

fn positive_duration(value: Option<&serde_json::Value>, field: &str) -> Result<Duration, String> {
    let seconds = value
        .and_then(serde_json::Value::as_f64)
        .filter(|seconds| seconds.is_finite() && *seconds > 0.0)
        .ok_or_else(|| format!("Invalid Genspark login response field: {field}"))?;
    Duration::try_from_secs_f64(seconds)
        .map_err(|_| format!("Invalid Genspark login response field: {field}"))
}

fn duration_millis_u64(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

/// A parsed https URL without credentials or control characters.
fn trusted_url(raw: &str) -> Result<String, String> {
    let url =
        Url::parse(raw).map_err(|_| "Untrusted auth URL in Genspark login response".to_string())?;
    let untrusted = url.scheme() != "https"
        || !url.username().is_empty()
        || url.password().is_some()
        || raw
            .chars()
            .any(|c| c <= '\u{20}' || ('\u{7f}'..='\u{9f}').contains(&c));
    if untrusted {
        return Err("Untrusted auth URL in Genspark login response".to_string());
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

    type ScriptedResponse = super::super::provider_http::ProviderHttpResponse;

    /// Queued responses per URL (popped in order).
    struct ScriptedHttp {
        queued: Mutex<HashMap<String, VecDeque<ScriptedResponse>>>,
        requests: Mutex<Vec<ProviderHttpRequest>>,
    }

    impl ScriptedHttp {
        fn new() -> Self {
            ScriptedHttp {
                queued: Mutex::new(HashMap::new()),
                requests: Mutex::new(Vec::new()),
            }
        }

        fn entry(status: u16, body: &str) -> ScriptedResponse {
            ScriptedResponse {
                status,
                body: body.to_string(),
            }
        }

        fn queue(self, url: &str, responses: Vec<ScriptedResponse>) -> Self {
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
        ) -> Pin<Box<dyn Future<Output = Result<ScriptedResponse, String>> + Send + '_>> {
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

    fn device_response() -> ScriptedResponse {
        ScriptedHttp::entry(
            200,
            r#"{"device_code":"dev-1","auth_url":"https://www.genspark.ai/api/cli_auth/verify?code=abc123","expires_in":600,"poll_interval":0.001}"#,
        )
    }

    fn token_poll_url() -> String {
        token_url("dev-1").unwrap()
    }

    #[tokio::test]
    async fn the_happy_flow_resolves_the_api_key() {
        let poll_url = token_poll_url();
        let http = ScriptedHttp::new()
            .queue(DEVICE_CODE_URL, vec![device_response()])
            .queue(
                &poll_url,
                vec![
                    ScriptedHttp::entry(200, r#"{"status":"pending"}"#),
                    ScriptedHttp::entry(200, r#"{"status":"approved","api_key":"gsk-test"}"#),
                ],
            );
        let ui = ScriptedUi::new();
        let credentials = login_genspark(&http, &ui).await.unwrap();
        assert_eq!(credentials.api_key, "gsk-test");
        let (url, instructions) = ui.captured_auth();
        assert_eq!(
            url,
            "https://www.genspark.ai/api/cli_auth/verify?code=abc123"
        );
        assert_eq!(instructions, None);
        let device = &http.requests_for(DEVICE_CODE_URL)[0];
        assert_eq!(device.method, ProviderHttpMethod::Post);
        assert_eq!(device.body.as_deref(), Some("{}"));
        assert!(device
            .headers
            .iter()
            .any(|(name, value)| name == "Content-Type" && value == "application/json"));
        assert!(device
            .headers
            .iter()
            .any(|(name, value)| name == "User-Agent" && value == USER_AGENT));
        let polls = http.requests_for(&poll_url);
        assert_eq!(polls.len(), 2);
        assert_eq!(polls[0].method, ProviderHttpMethod::Get);
        assert_eq!(polls[0].body, None);
    }

    #[tokio::test]
    async fn a_cancelled_surface_stops_between_steps() {
        let poll_url = token_poll_url();
        let http = ScriptedHttp::new()
            .queue(DEVICE_CODE_URL, vec![device_response()])
            .queue(
                &poll_url,
                vec![ScriptedHttp::entry(
                    200,
                    r#"{"status":"approved","api_key":"gsk-test"}"#,
                )],
            );
        let ui = ScriptedUi::new().cancel_on_auth();
        let error = login_genspark(&http, &ui).await.unwrap_err();
        assert_eq!(error, LOGIN_CANCELLED);
        assert_eq!(
            http.requests_for(&poll_url),
            Vec::<ProviderHttpRequest>::new(),
            "the cancelled pane does not poll for a token"
        );
    }

    #[tokio::test]
    async fn a_denied_authorization_surfaces_the_message() {
        let poll_url = token_poll_url();
        let http = ScriptedHttp::new()
            .queue(DEVICE_CODE_URL, vec![device_response()])
            .queue(
                &poll_url,
                vec![ScriptedHttp::entry(200, r#"{"status":"denied"}"#)],
            );
        let error = login_genspark(&http, &ScriptedUi::new()).await.unwrap_err();
        assert_eq!(error, "Genspark device authorization was denied");
    }

    #[tokio::test]
    async fn an_expired_device_code_surfaces_the_message() {
        let poll_url = token_poll_url();
        let http = ScriptedHttp::new()
            .queue(DEVICE_CODE_URL, vec![device_response()])
            .queue(
                &poll_url,
                vec![ScriptedHttp::entry(200, r#"{"status":"expired"}"#)],
            );
        let error = login_genspark(&http, &ScriptedUi::new()).await.unwrap_err();
        assert_eq!(error, "Genspark device code expired; sign in again");
    }

    #[tokio::test]
    async fn an_http_error_stops_the_poll() {
        let poll_url = token_poll_url();
        let http = ScriptedHttp::new()
            .queue(DEVICE_CODE_URL, vec![device_response()])
            .queue(
                &poll_url,
                vec![ScriptedHttp::entry(500, r#"{"status":"pending"}"#)],
            );
        let error = login_genspark(&http, &ScriptedUi::new()).await.unwrap_err();
        assert_eq!(error, "Genspark token polling failed (HTTP 500)");
    }

    #[tokio::test]
    async fn an_untrusted_auth_url_fails_the_flow() {
        let http = ScriptedHttp::new().queue(
            DEVICE_CODE_URL,
            vec![ScriptedHttp::entry(
                200,
                r#"{"device_code":"dev-1","auth_url":"http://www.genspark.ai/verify","expires_in":600,"poll_interval":0.001}"#,
            )],
        );
        let error = login_genspark(&http, &ScriptedUi::new()).await.unwrap_err();
        assert_eq!(error, "Untrusted auth URL in Genspark login response");
    }
}
