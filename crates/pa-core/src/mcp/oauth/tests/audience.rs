//! Resource audiences: the exact, trailing-slash, and origin-level shapes a
//! server may declare, on both the login and the refresh path.

use url::Url;

use super::fixtures::*;
use crate::auth::types::AuthCredential;
use crate::mcp::oauth::{mcp_login, mcp_refresh_token};

/// The Vercel shape (TS parity): a root endpoint whose declared
/// resource keeps the trailing slash is component-equal, never a
/// string-equal, match — the declared resource is the stored and sent
/// audience.
#[tokio::test]
async fn root_endpoint_trailing_slash_resource_audience() {
    const ROOT: &str = "https://root.example";
    const ROOT_PRM: &str = "https://root.example/.well-known/oauth-protected-resource";
    const ROOT_AS: &str = "https://root.example/.well-known/oauth-authorization-server";
    let http = ScriptedHttp::new(vec![
        (ROOT, 401, None, ""),
        (
            ROOT_PRM,
            200,
            None,
            &json_response(&serde_json::json!({
                "resource": "https://root.example/",
                "authorization_servers": [ROOT],
                "scopes_supported": ["openid"],
            })),
        ),
        (
            ROOT_AS,
            200,
            None,
            &json_response(&serde_json::json!({
                "issuer": ROOT,
                "authorization_endpoint": "https://root.example/authorize",
                "token_endpoint": "https://root.example/token",
                "scopes_supported": ["openid"],
            })),
        ),
        (
            "https://root.example/token",
            200,
            None,
            &json_response(&serde_json::json!({
                "access_token": "root-access",
                "refresh_token": "root-refresh",
            })),
        ),
    ]);
    let ui = test_ui();
    let mut login_config = config("root", ROOT);
    login_config.client_id = Some("root-client".to_string());
    let credentials = mcp_login(&http, &login_config, &ui).await.unwrap();
    match &credentials {
        AuthCredential::Oauth {
            access,
            endpoint,
            resource,
            issuer,
            audience_mode,
            ..
        } => {
            assert_eq!(access, "root-access");
            assert_eq!(endpoint.as_deref(), Some(ROOT));
            assert_eq!(resource.as_deref(), Some("https://root.example/"));
            assert_eq!(issuer.as_deref(), Some(ROOT));
            assert_eq!(audience_mode.as_deref(), Some("exact"));
        }
        other => panic!("oauth credential expected, got {other:?}"),
    }
    // The declared resource and the PRM-advertised scope went out on
    // the authorization URL.
    let auth_url = Url::parse(&ui.auth_url.lock().unwrap()).unwrap();
    let param = |name: &str| {
        auth_url
            .query_pairs()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.to_string())
    };
    assert_eq!(param("resource").as_deref(), Some("https://root.example/"));
    assert_eq!(param("scope").as_deref(), Some("openid"));
    // The token request carried the declared resource.
    let token_request = http
        .seen
        .lock()
        .unwrap()
        .iter()
        .find(|(url, _)| url == "https://root.example/token")
        .cloned()
        .unwrap();
    let body = token_request.1.unwrap();
    let encoded_resource = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("resource", "https://root.example/")
        .finish();
    assert!(body.contains(&encoded_resource));
}
/// The Notion/Slack shape (TS parity): an origin-level declared
/// resource serves a pathful endpoint as an "origin" audience.
#[tokio::test]
async fn origin_level_resource_audience_serves_pathful_endpoint() {
    const NOTION_PRM: &str = "https://srv.test/.well-known/oauth-protected-resource/mcp";
    let http = ScriptedHttp::new(vec![
        (ORIGIN_URL, 401, None, ""),
        (
            NOTION_PRM,
            200,
            None,
            &json_response(&serde_json::json!({
                "resource": "https://srv.test",
                "authorization_servers": ["https://srv.test/tenant"],
                "scopes_supported": ["default"],
            })),
        ),
        (
            "https://srv.test/.well-known/oauth-authorization-server/tenant",
            200,
            None,
            &json_response(&serde_json::json!({
                "issuer": "https://srv.test/tenant",
                "authorization_endpoint": ORIGIN_AUTHORIZE,
                "token_endpoint": ORIGIN_TOKEN,
                "registration_endpoint": ORIGIN_REGISTER,
            })),
        ),
        (
            ORIGIN_REGISTER,
            200,
            None,
            &json_response(&serde_json::json!({ "client_id": "origin-client" })),
        ),
        (
            ORIGIN_TOKEN,
            200,
            None,
            &json_response(&serde_json::json!({ "access_token": "origin-access" })),
        ),
    ]);
    let ui = test_ui();
    let credentials = mcp_login(&http, &config("origin", ORIGIN_URL), &ui)
        .await
        .unwrap();
    match &credentials {
        AuthCredential::Oauth {
            access,
            resource,
            issuer,
            audience_mode,
            ..
        } => {
            assert_eq!(access, "origin-access");
            assert_eq!(resource.as_deref(), Some("https://srv.test"));
            assert_eq!(issuer.as_deref(), Some("https://srv.test/tenant"));
            assert_eq!(audience_mode.as_deref(), Some("origin"));
        }
        other => panic!("oauth credential expected, got {other:?}"),
    }
    // The declared origin-level audience went out on the authorization
    // URL.
    let auth_url = Url::parse(&ui.auth_url.lock().unwrap()).unwrap();
    let resource = auth_url
        .query_pairs()
        .find(|(key, _)| key == "resource")
        .map(|(_, value)| value.to_string());
    assert_eq!(resource.as_deref(), Some("https://srv.test"));
}
/// Origin-mode credentials refresh against the origin-level binding;
/// an audience-mode drift against current discovery requires re-login.
#[tokio::test]
async fn refresh_validates_audience_modes() {
    const NOTION_PRM: &str = "https://srv.test/.well-known/oauth-protected-resource/mcp";
    const DECLARED: &str = "https://srv.test";
    // A credential factory: the enum is not a struct, so variant
    // rebuilds spell every field.
    let reissued_oauth =
        |resource: Option<&str>, audience_mode: Option<&str>| AuthCredential::Oauth {
            access: "origin-access".to_string(),
            refresh: Some("origin-refresh".to_string()),
            expires: 0,
            account_id: None,
            endpoint: Some(ORIGIN_URL.to_string()),
            token_endpoint: Some(ORIGIN_TOKEN.to_string()),
            client_id: Some("origin-client".to_string()),
            resource: resource.map(str::to_string),
            issuer: Some("https://srv.test/tenant".to_string()),
            audience_mode: audience_mode.map(str::to_string),
            enterprise_url: None,
        };
    let credentials = reissued_oauth(Some(DECLARED), Some("origin"));
    let http = ScriptedHttp::new(vec![
        (ORIGIN_URL, 401, None, ""),
        (
            NOTION_PRM,
            200,
            None,
            &json_response(&serde_json::json!({
                "resource": DECLARED,
                "authorization_servers": ["https://srv.test/tenant"],
            })),
        ),
        (
            "https://srv.test/.well-known/oauth-authorization-server/tenant",
            200,
            None,
            &json_response(&serde_json::json!({
                "issuer": "https://srv.test/tenant",
                "authorization_endpoint": ORIGIN_AUTHORIZE,
                "token_endpoint": ORIGIN_TOKEN,
            })),
        ),
        (
            ORIGIN_TOKEN,
            200,
            None,
            &json_response(&serde_json::json!({
                "access_token": "origin-access-2",
                "expires_in": 1800,
            })),
        ),
    ]);
    // The origin-mode grant refreshes through the origin-level binding.
    let refreshed = mcp_refresh_token(&http, &config("origin", ORIGIN_URL), &credentials)
        .await
        .unwrap();
    match &refreshed {
        AuthCredential::Oauth {
            access,
            resource,
            audience_mode,
            ..
        } => {
            assert_eq!(access, "origin-access-2");
            assert_eq!(resource.as_deref(), Some(DECLARED));
            assert_eq!(audience_mode.as_deref(), Some("origin"));
        }
        other => panic!("oauth credential expected, got {other:?}"),
    }
    // The refresh request carried the stored origin-level audience.
    let token_request = http
        .seen
        .lock()
        .unwrap()
        .iter()
        .find(|(url, _)| url == ORIGIN_TOKEN)
        .cloned()
        .unwrap();
    assert!(token_request
        .1
        .unwrap()
        .contains("resource=https%3A%2F%2Fsrv.test"));

    // (The TS audience-mode equality fences on re-discovery — legacy
    // must re-discover as "exact", non-legacy modes must match — are
    // migration fences: classification is a pure function of the
    // endpoint and the declared resource, so a well-formed discovery
    // can never drift modes for the same stored binding. They stay
    // ported, unreachable by construction.)

    // Legacy credentials (no stored audience mode) pin to the canonical
    // endpoint: an origin-level stored resource is refused at the
    // binding check before any request leaves.
    let legacy = reissued_oauth(Some(DECLARED), None);
    let error = mcp_refresh_token(&http, &config("origin", ORIGIN_URL), &legacy)
        .await
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("not bound to https://srv.test/mcp"),
        "{error}"
    );

    // Legacy credentials whose stored resource is the canonical
    // endpoint refresh under "exact" re-discovery.
    let http = ScriptedHttp::new(vec![
        (ORIGIN_URL, 401, None, ""),
        (
            NOTION_PRM,
            200,
            None,
            &json_response(&serde_json::json!({
                "resource": ORIGIN_URL,
                "authorization_servers": ["https://srv.test/tenant"],
            })),
        ),
        (
            "https://srv.test/.well-known/oauth-authorization-server/tenant",
            200,
            None,
            &json_response(&serde_json::json!({
                "issuer": "https://srv.test/tenant",
                "authorization_endpoint": ORIGIN_AUTHORIZE,
                "token_endpoint": ORIGIN_TOKEN,
            })),
        ),
        (
            ORIGIN_TOKEN,
            200,
            None,
            &json_response(&serde_json::json!({ "access_token": "exact-access" })),
        ),
    ]);
    let legacy_exact = reissued_oauth(Some(ORIGIN_URL), None);
    let refreshed = mcp_refresh_token(&http, &config("origin", ORIGIN_URL), &legacy_exact)
        .await
        .unwrap();
    match &refreshed {
        AuthCredential::Oauth {
            access,
            audience_mode,
            ..
        } => {
            assert_eq!(access, "exact-access");
            assert!(audience_mode.is_none());
        }
        other => panic!("oauth credential expected, got {other:?}"),
    }
}
