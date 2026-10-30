//! The fixtures shared by the OAuth test subjects: a scripted transport, a
//! login UI with a manual-paste surface, and the server shapes the flow is
//! exercised against.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Mutex;

use anyhow::{anyhow, Result};
use url::Url;

use crate::mcp::oauth::{McpLoginUi, McpOAuthConfig};
use crate::mcp::oauth_http::{OAuthHttp, OAuthHttpRequest, OAuthHttpResponse};

/// A scripted transport: url -> response. Unknown urls fail the
/// request (the TS suite throws on unexpected fetches).
pub(super) struct ScriptedHttp {
    pub(super) responses: HashMap<String, OAuthHttpResponse>,
    pub(super) seen: Mutex<Vec<(String, Option<String>)>>,
}

impl ScriptedHttp {
    pub(super) fn new(responses: Vec<(&str, u16, Option<&str>, &str)>) -> Self {
        let responses = responses
            .into_iter()
            .map(|(url, status, header, body)| {
                let mut headers =
                    vec![("content-type".to_string(), "application/json".to_string())];
                if let Some(header) = header {
                    headers.push(("www-authenticate".to_string(), header.to_string()));
                }
                (
                    url.to_string(),
                    OAuthHttpResponse {
                        status,
                        headers,
                        body: body.to_string(),
                    },
                )
            })
            .collect();
        ScriptedHttp {
            responses,
            seen: Mutex::new(Vec::new()),
        }
    }
}

impl OAuthHttp for ScriptedHttp {
    fn request(
        &self,
        request: OAuthHttpRequest,
    ) -> futures::future::BoxFuture<'_, Result<OAuthHttpResponse>> {
        Box::pin(async move {
            self.seen
                .lock()
                .unwrap()
                .push((request.url.clone(), request.body.clone()));
            match self.responses.get(&request.url) {
                Some(response) => Ok(response.clone()),
                None => Err(anyhow!("unexpected request: {}", request.url)),
            }
        })
    }
}

pub(super) fn json_response(body: &serde_json::Value) -> String {
    body.to_string()
}

/// A login UI with a manual-paste surface: the paste is derived from the authorization URL the
/// way a user would, or a fixed input when set.
pub(super) struct TestUi {
    pub(super) auth_url: Mutex<String>,
    pub(super) manual: Mutex<Option<String>>,
    pub(super) progress: Mutex<Vec<String>>,
}

impl TestUi {
    pub(super) fn with_manual_input(input: &str) -> Self {
        TestUi {
            auth_url: Mutex::new(String::new()),
            manual: Mutex::new(Some(input.to_string())),
            progress: Mutex::new(Vec::new()),
        }
    }
    pub(super) fn from_auth_url() -> Self {
        TestUi {
            auth_url: Mutex::new(String::new()),
            manual: Mutex::new(None),
            progress: Mutex::new(Vec::new()),
        }
    }
}

impl McpLoginUi for std::sync::Arc<TestUi> {
    fn on_progress(&self, message: &str) {
        self.progress.lock().unwrap().push(message.to_string());
    }
    fn on_auth(&self, url: &str, _instructions: &str) {
        *self.auth_url.lock().unwrap() = url.to_string();
    }
    fn on_prompt(
        &self,
        _message: &str,
        _placeholder: &str,
    ) -> Pin<Box<dyn Future<Output = Result<String>> + Send>> {
        Box::pin(async { Err(anyhow!("prompt surface unavailable in tests")) })
    }
    fn on_manual_code_input(&self) -> Option<Pin<Box<dyn Future<Output = Option<String>> + Send>>> {
        // The paste surface exists once a login is live (the surface
        // resolves from the authorization URL like a user would).
        let ui = std::sync::Arc::clone(self);
        Some(Box::pin(async move {
            if let Some(input) = ui.manual.lock().unwrap().take() {
                return Some(input);
            }
            let auth_url = ui.auth_url.lock().unwrap().clone();
            let url = Url::parse(&auth_url).ok()?;
            let redirect = url
                .query_pairs()
                .find(|(key, _)| key == "redirect_uri")
                .map(|(_, value)| value.to_string())?;
            let state = url
                .query_pairs()
                .find(|(key, _)| key == "state")
                .map(|(_, value)| value.to_string())?;
            Some(format!("{redirect}?code=the-code&state={state}"))
        }))
    }
}

pub(super) fn test_ui() -> std::sync::Arc<TestUi> {
    std::sync::Arc::new(TestUi::from_auth_url())
}

pub(super) const RESOURCE: &str = "https://mcp.plane.so/http/mcp";
pub(super) const PLANE_ISSUER: &str = "https://mcp.plane.so/http";
pub(super) const PLANE_PRM_URL: &str =
    "https://mcp.plane.so/.well-known/oauth-protected-resource/http/mcp";
pub(super) const PLANE_META_URL: &str =
    "https://mcp.plane.so/.well-known/oauth-authorization-server/http";
pub(super) const PLANE_REGISTER: &str = "https://mcp.plane.so/http/register";
pub(super) const PLANE_TOKEN: &str = "https://mcp.plane.so/http/token";
pub(super) const PLANE_AUTHORIZE: &str = "https://mcp.plane.so/http/authorize";
pub(super) const ORIGIN_URL: &str = "https://srv.test/mcp";
pub(super) const ORIGIN_META_URL: &str = "https://srv.test/.well-known/oauth-authorization-server";
pub(super) const ORIGIN_AUTHORIZE: &str = "https://srv.test/authorize";
pub(super) const ORIGIN_REGISTER: &str = "https://srv.test/register";
pub(super) const ORIGIN_TOKEN: &str = "https://srv.test/token";

pub(super) fn plane_meta() -> serde_json::Value {
    serde_json::json!({
        "issuer": PLANE_ISSUER,
        "authorization_endpoint": PLANE_AUTHORIZE,
        "token_endpoint": PLANE_TOKEN,
        "registration_endpoint": PLANE_REGISTER,
        "scopes_supported": ["read", "write"],
    })
}

pub(super) fn origin_meta() -> serde_json::Value {
    serde_json::json!({
        "issuer": "https://srv.test/tenant",
        "authorization_endpoint": ORIGIN_AUTHORIZE,
        "token_endpoint": ORIGIN_TOKEN,
        "registration_endpoint": ORIGIN_REGISTER,
        "scopes_supported": ["read", "write"],
    })
}

pub(super) fn config(server: &str, url: &str) -> McpOAuthConfig {
    McpOAuthConfig {
        server: server.to_string(),
        label: server.to_string(),
        url: url.to_string(),
        client_id: None,
        callback_port: None,
        scopes: None,
    }
}
