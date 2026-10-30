//! Discovery: which metadata a server advertises, and what must be refused
//! when the advertised resource, issuer, or document shape disagrees.

use url::Url;

use super::fixtures::*;
use crate::auth::types::AuthCredential;
use crate::mcp::oauth::mcp_login;

/// The full Plane-style login through a manual paste: PRM, issuer
/// metadata, DCR, and the token endpoint.
#[tokio::test]
async fn discovers_protected_resource_metadata_and_external_issuer() {
    let http = ScriptedHttp::new(vec![
        (RESOURCE, 401, None, ""),
        (
            PLANE_PRM_URL,
            200,
            None,
            &json_response(&serde_json::json!({
                "resource": RESOURCE,
                "authorization_servers": [PLANE_ISSUER],
                "scopes_supported": ["read", "write"],
            })),
        ),
        (PLANE_META_URL, 200, None, &json_response(&plane_meta())),
        (
            PLANE_REGISTER,
            200,
            None,
            &json_response(&serde_json::json!({ "client_id": "plane-client" })),
        ),
        (
            PLANE_TOKEN,
            200,
            None,
            &json_response(&serde_json::json!({
                "access_token": "access-1",
                "refresh_token": "refresh-1",
                "expires_in": 3600,
            })),
        ),
    ]);
    let ui = test_ui();
    let credentials = mcp_login(&http, &config("plane", RESOURCE), &ui)
        .await
        .unwrap();
    match &credentials {
        AuthCredential::Oauth {
            access,
            refresh,
            endpoint,
            token_endpoint,
            client_id,
            resource,
            issuer,
            audience_mode,
            ..
        } => {
            assert_eq!(access, "access-1");
            assert_eq!(refresh.as_deref(), Some("refresh-1"));
            assert_eq!(endpoint.as_deref(), Some(RESOURCE));
            assert_eq!(token_endpoint.as_deref(), Some(PLANE_TOKEN));
            assert_eq!(client_id.as_deref(), Some("plane-client"));
            assert_eq!(resource.as_deref(), Some(RESOURCE));
            assert_eq!(issuer.as_deref(), Some(PLANE_ISSUER));
            assert_eq!(audience_mode.as_deref(), Some("exact"));
        }
        other => panic!("oauth credential expected, got {other:?}"),
    }
    // The registration request carried the resolved scope.
    let registration_request = http
        .seen
        .lock()
        .unwrap()
        .iter()
        .find(|(url, _)| url == PLANE_REGISTER)
        .cloned()
        .unwrap();
    let registration_body =
        serde_json::from_str::<serde_json::Value>(&registration_request.1.unwrap()).unwrap();
    assert_eq!(
        registration_body.get("scope").and_then(|v| v.as_str()),
        Some("read write")
    );
    // The authorization URL carries the client, the resource, and the
    // server's advertised scopes.
    let auth_url = Url::parse(&ui.auth_url.lock().unwrap()).unwrap();
    let param = |name: &str| {
        auth_url
            .query_pairs()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.to_string())
    };
    assert_eq!(param("client_id").as_deref(), Some("plane-client"));
    assert_eq!(param("resource").as_deref(), Some(RESOURCE));
    assert_eq!(param("scope").as_deref(), Some("read write"));
    assert_eq!(param("code_challenge_method").as_deref(), Some("S256"));
    // The token request carried the resource indicator.
    let token_request = http
        .seen
        .lock()
        .unwrap()
        .iter()
        .find(|(url, _)| url == PLANE_TOKEN)
        .cloned()
        .unwrap();
    let body = token_request.1.unwrap();
    assert!(body.contains("grant_type=authorization_code"));
    let encoded_resource = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("resource", RESOURCE)
        .finish();
    assert!(body.contains(&encoded_resource));
}
#[tokio::test]
async fn origin_level_metadata_without_protected_resource() {
    let origin_prm = "https://srv.test/.well-known/oauth-protected-resource/mcp";
    let http = ScriptedHttp::new(vec![
        (ORIGIN_URL, 404, None, ""),
        (origin_prm, 404, None, ""),
        (ORIGIN_META_URL, 200, None, &json_response(&origin_meta())),
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
            &json_response(&serde_json::json!({
                "access_token": "origin-access",
                "refresh_token": "origin-refresh",
                "expires_in": 3600,
            })),
        ),
    ]);
    let ui = test_ui();
    let credentials = mcp_login(&http, &config("origin", ORIGIN_URL), &ui)
        .await
        .unwrap();
    match &credentials {
        AuthCredential::Oauth {
            access,
            endpoint,
            resource,
            issuer,
            ..
        } => {
            assert_eq!(access, "origin-access");
            assert_eq!(endpoint.as_deref(), Some(ORIGIN_URL));
            assert_eq!(*resource, None);
            assert_eq!(*issuer, None);
        }
        other => panic!("oauth credential expected, got {other:?}"),
    }
    // No resource parameter on the authorization URL.
    let auth_url = Url::parse(&ui.auth_url.lock().unwrap()).unwrap();
    assert!(!auth_url.query_pairs().any(|(key, _)| key == "resource"));
    // The root PRM location is never probed for a pathful resource.
    let urls: Vec<String> = http
        .seen
        .lock()
        .unwrap()
        .iter()
        .map(|(url, _)| url.clone())
        .collect();
    assert!(!urls.contains(&"https://srv.test/.well-known/oauth-protected-resource".to_string()));
}
#[tokio::test]
async fn www_authenticate_pointer_preferred() {
    let pointer = "https://metadata.example/resources/plane";
    let http = ScriptedHttp::new(vec![
        (
            RESOURCE,
            401,
            Some(
                r#"Bearer realm="mcp", resource_metadata="https://metadata.example/resources/plane""#,
            ),
            "",
        ),
        (
            pointer,
            200,
            None,
            &json_response(&serde_json::json!({
                "resource": RESOURCE,
                "authorization_servers": [PLANE_ISSUER],
            })),
        ),
        (PLANE_META_URL, 200, None, &json_response(&plane_meta())),
        (
            PLANE_REGISTER,
            200,
            None,
            &json_response(&serde_json::json!({ "client_id": "pointer-client" })),
        ),
        (
            PLANE_TOKEN,
            200,
            None,
            &json_response(&serde_json::json!({ "access_token": "pointer-access" })),
        ),
    ]);
    let ui = test_ui();
    let credentials = mcp_login(&http, &config("plane", RESOURCE), &ui)
        .await
        .unwrap();
    match &credentials {
        AuthCredential::Oauth {
            access,
            resource,
            issuer,
            ..
        } => {
            assert_eq!(access, "pointer-access");
            assert_eq!(resource.as_deref(), Some(RESOURCE));
            assert_eq!(issuer.as_deref(), Some(PLANE_ISSUER));
        }
        other => panic!("oauth credential expected, got {other:?}"),
    }
    // The derived PRM URL was never fetched.
    let urls: Vec<String> = http
        .seen
        .lock()
        .unwrap()
        .iter()
        .map(|(url, _)| url.clone())
        .collect();
    assert!(!urls.contains(&PLANE_PRM_URL.to_string()));
}
#[tokio::test]
async fn protected_resource_mismatch_fails() {
    let http = ScriptedHttp::new(vec![
        (RESOURCE, 401, None, ""),
        (
            PLANE_PRM_URL,
            200,
            None,
            &json_response(&serde_json::json!({
                "resource": "https://attacker.example/mcp",
                "authorization_servers": [PLANE_ISSUER],
            })),
        ),
    ]);
    let error = mcp_login(&http, &config("plane", RESOURCE), &test_ui())
        .await
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("resource does not match the configured endpoint"),
        "{error}"
    );
}
#[tokio::test]
async fn prm_selected_issuer_must_match_exactly() {
    let http = ScriptedHttp::new(vec![
        (RESOURCE, 401, None, ""),
        (
            PLANE_PRM_URL,
            200,
            None,
            &json_response(&serde_json::json!({
                "resource": RESOURCE,
                "authorization_servers": [PLANE_ISSUER],
            })),
        ),
        (
            PLANE_META_URL,
            200,
            None,
            &json_response(&serde_json::json!({
                "issuer": "https://wrong.example",
                "authorization_endpoint": PLANE_AUTHORIZE,
                "token_endpoint": PLANE_TOKEN,
            })),
        ),
        (
            "https://mcp.plane.so/http/.well-known/openid-configuration",
            404,
            None,
            "",
        ),
    ]);
    let error = mcp_login(&http, &config("plane", RESOURCE), &test_ui())
        .await
        .unwrap_err()
        .to_string();
    assert!(error.contains("issuer does not exactly match"), "{error}");
}
/// Pathful OIDC metadata when RFC 8414 serves a non-metadata document.
#[tokio::test]
async fn pathful_oidc_fallback() {
    let issuer = "https://login.example/tenant";
    let oidc_meta = "https://login.example/tenant/.well-known/openid-configuration";
    let oidc = serde_json::json!({
        "issuer": issuer,
        "authorization_endpoint": "https://login.example/tenant/authorize",
        "token_endpoint": "https://login.example/tenant/token",
        "registration_endpoint": "https://login.example/tenant/register",
        "scopes_supported": ["read", "write"],
    });
    let http = ScriptedHttp::new(vec![
        (RESOURCE, 404, None, ""),
        (
            PLANE_PRM_URL,
            200,
            None,
            &json_response(&serde_json::json!({
                "resource": RESOURCE,
                "authorization_servers": [issuer],
            })),
        ),
        (
            "https://login.example/.well-known/oauth-authorization-server/tenant",
            200,
            None,
            "<html>not metadata</html>",
        ),
        (oidc_meta, 200, None, &json_response(&oidc)),
        (
            "https://login.example/tenant/register",
            200,
            None,
            &json_response(&serde_json::json!({ "client_id": "c" })),
        ),
        (
            "https://login.example/tenant/token",
            200,
            None,
            &json_response(&serde_json::json!({ "access_token": "a", "expires_in": 60 })),
        ),
    ]);
    let credentials = mcp_login(&http, &config("plane", RESOURCE), &test_ui())
        .await
        .unwrap();
    match &credentials {
        AuthCredential::Oauth {
            resource, issuer, ..
        } => {
            assert_eq!(resource.as_deref(), Some(RESOURCE));
            assert_eq!(issuer.as_deref(), Some("https://login.example/tenant"));
        }
        other => panic!("oauth credential expected, got {other:?}"),
    }
}
#[tokio::test]
async fn dynamic_registration_unavailable_fails_clearly() {
    let meta = serde_json::json!({
        "issuer": "https://srv.test/tenant",
        "authorization_endpoint": ORIGIN_AUTHORIZE,
        "token_endpoint": ORIGIN_TOKEN,
    });
    let origin_prm = "https://srv.test/.well-known/oauth-protected-resource/mcp";
    let http = ScriptedHttp::new(vec![
        (ORIGIN_URL, 404, None, ""),
        (origin_prm, 404, None, ""),
        (ORIGIN_META_URL, 200, None, &json_response(&meta)),
    ]);
    let error = mcp_login(&http, &config("slackish", ORIGIN_URL), &test_ui())
        .await
        .unwrap_err()
        .to_string();
    assert!(error.contains("dynamic client registration"), "{error}");
}
