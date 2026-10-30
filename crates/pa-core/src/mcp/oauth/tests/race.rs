//! The registry races: a login dropped mid-paste must release its callback
//! port, and a settled browser redirect must survive a paste that cancels or
//! fails validation.

use std::future::Future;
use std::pin::Pin;
use std::sync::Mutex;

use anyhow::{anyhow, Result};
use tokio::time::Duration;
use url::Url;

use super::fixtures::*;
use crate::mcp::oauth::mcp_login;
use crate::mcp::oauth::McpLoginUi;

#[tokio::test]
async fn dropping_a_login_with_a_pending_paste_releases_the_registered_port() {
    struct PendingUi(Mutex<String>);
    impl McpLoginUi for PendingUi {
        fn on_progress(&self, _message: &str) {}
        fn on_auth(&self, url: &str, _instructions: &str) {
            *self.0.lock().unwrap() = url.to_string();
        }
        fn on_prompt(
            &self,
            _message: &str,
            _placeholder: &str,
        ) -> Pin<Box<dyn Future<Output = Result<String>> + Send>> {
            Box::pin(std::future::pending())
        }
        fn on_manual_code_input(
            &self,
        ) -> Option<Pin<Box<dyn Future<Output = Option<String>> + Send>>> {
            Some(Box::pin(std::future::pending()))
        }
    }
    let http = ScriptedHttp::new(vec![
        (ORIGIN_URL, 404, None, ""),
        (
            "https://srv.test/.well-known/oauth-protected-resource/mcp",
            404,
            None,
            "",
        ),
        (ORIGIN_META_URL, 200, None, &json_response(&origin_meta())),
    ]);
    let reservation = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = reservation.local_addr().unwrap().port();
    drop(reservation);
    let mut oauth_config = config("Slack", ORIGIN_URL);
    oauth_config.client_id = Some("registered-client".to_string());
    oauth_config.callback_port = Some(port);
    let ui = PendingUi(Mutex::default());
    let mut login = Box::pin(mcp_login(&http, &oauth_config, &ui));
    assert!(futures::poll!(login.as_mut()).is_pending());
    assert!(
        !ui.0.lock().unwrap().is_empty(),
        "login reached browser authorization"
    );
    drop(login);
    let rebound = std::net::TcpListener::bind(("127.0.0.1", port))
        .expect("cancelling login must release its callback port before the next attempt");
    drop(rebound);
}
#[tokio::test(start_paused = true)]
async fn settled_browser_code_wins_over_paste_cancel_and_validation_errors() {
    struct GraceUi {
        auth_url: Mutex<String>,
        input: Option<String>,
    }
    impl McpLoginUi for GraceUi {
        fn on_progress(&self, _message: &str) {}
        fn on_auth(&self, url: &str, _instructions: &str) {
            *self.auth_url.lock().unwrap() = url.to_string();
        }
        fn on_prompt(
            &self,
            _message: &str,
            _placeholder: &str,
        ) -> Pin<Box<dyn Future<Output = Result<String>> + Send>> {
            Box::pin(async { Err(anyhow!("unexpected fallback prompt")) })
        }
        fn on_manual_code_input(
            &self,
        ) -> Option<Pin<Box<dyn Future<Output = Option<String>> + Send>>> {
            let input = self.input.clone();
            Some(Box::pin(async move { input }))
        }
    }
    let http = ScriptedHttp::new(vec![
        (ORIGIN_URL, 404, None, ""),
        (
            "https://srv.test/.well-known/oauth-protected-resource/mcp",
            404,
            None,
            "",
        ),
        (ORIGIN_META_URL, 200, None, &json_response(&origin_meta())),
        (
            ORIGIN_TOKEN,
            200,
            None,
            r#"{"access_token":"browser-access"}"#,
        ),
    ]);
    let client = reqwest::Client::new();
    for input in [
        None,
        Some("http://localhost/callback?code=paste&state=wrong".to_string()),
    ] {
        // Every schedule must succeed; repetition exercises select's fair branch ordering,
        // rather than retrying a failed login until it happens to succeed.
        for _ in 0..32 {
            let reservation = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
            let port = reservation.local_addr().unwrap().port();
            drop(reservation);
            let mut oauth_config = config("Slack", ORIGIN_URL);
            oauth_config.client_id = Some("registered-client".to_string());
            oauth_config.callback_port = Some(port);
            let ui = GraceUi {
                auth_url: Mutex::default(),
                input: input.clone(),
            };
            let mut login = Box::pin(mcp_login(&http, &oauth_config, &ui));
            assert!(futures::poll!(login.as_mut()).is_pending());
            let auth_url = Url::parse(&ui.auth_url.lock().unwrap()).unwrap();
            let state = auth_url
                .query_pairs()
                .find(|(key, _)| key == "state")
                .map(|(_, value)| value.into_owned())
                .unwrap();
            let response = client
                .get(format!(
                    "http://127.0.0.1:{port}/callback?code=browser-code&state={state}"
                ))
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), reqwest::StatusCode::OK);
            // Both the browser result and the expired paste grace are now ready before
            // the login future is polled again; no wall-clock sleeps or timing guesses.
            tokio::time::advance(Duration::from_secs(1)).await;
            login
                .await
                .expect("the settled browser code must survive the paste outcome");
            let requests = http.seen.lock().unwrap();
            let (url, body) = requests.last().unwrap();
            assert_eq!(url, ORIGIN_TOKEN);
            assert!(body.as_ref().unwrap().contains("code=browser-code"));
        }
    }
}
/// A genuine paste validation error surfaces; a cancelled paste keeps the browser path alive,
/// then reports a missing code.
#[tokio::test]
async fn manual_paste_state_mismatch_surfaces() {
    let http = ScriptedHttp::new(vec![
        (ORIGIN_URL, 404, None, ""),
        (
            "https://srv.test/.well-known/oauth-protected-resource/mcp",
            404,
            None,
            "",
        ),
        (ORIGIN_META_URL, 200, None, &json_response(&origin_meta())),
        (
            ORIGIN_REGISTER,
            200,
            None,
            &json_response(&serde_json::json!({ "client_id": "c" })),
        ),
    ]);
    let ui = std::sync::Arc::new(TestUi::with_manual_input(
        "http://localhost:53700/callback?code=stolen&state=other",
    ));
    let error = mcp_login(&http, &config("origin", ORIGIN_URL), &ui)
        .await
        .unwrap_err()
        .to_string();
    assert!(error.contains("OAuth state mismatch"), "{error}");
}
