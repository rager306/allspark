//! Refresh: the stored bindings are re-checked against current discovery
//! before one request leaves, and the prior refresh token survives a server
//! that omits a new one; an undeliverable token post is refused.

use super::fixtures::*;
use crate::auth::types::AuthCredential;
use crate::mcp::oauth::mcp_refresh_token;

#[tokio::test]
async fn redirected_token_post_rejected() {
    let credentials = AuthCredential::Oauth {
        access: "a".to_string(),
        refresh: Some("r".to_string()),
        expires: 0,
        account_id: None,
        endpoint: Some(ORIGIN_URL.to_string()),
        token_endpoint: Some(ORIGIN_TOKEN.to_string()),
        client_id: Some("c".to_string()),
        resource: None,
        issuer: None,
        audience_mode: None,
        enterprise_url: None,
    };
    let origin_prm = "https://srv.test/.well-known/oauth-protected-resource/mcp";
    let http = ScriptedHttp::new(vec![
        (ORIGIN_URL, 404, None, ""),
        (origin_prm, 404, None, ""),
        (ORIGIN_META_URL, 200, None, &json_response(&origin_meta())),
        (ORIGIN_TOKEN, 302, None, "redirect"),
    ]);
    let error = mcp_refresh_token(&http, &config("origin", ORIGIN_URL), &credentials)
        .await
        .unwrap_err()
        .to_string();
    assert!(error.contains("Token request to"), "{error}");
}
/// Refresh keeps the stored bindings, the resource indicator, and the
/// prior refresh token when the server omits a new one.
#[tokio::test]
async fn refresh_validates_bindings() {
    let credentials = AuthCredential::Oauth {
        access: "access-1".to_string(),
        refresh: Some("old-refresh".to_string()),
        expires: 0,
        account_id: None,
        endpoint: Some(RESOURCE.to_string()),
        token_endpoint: Some(PLANE_TOKEN.to_string()),
        client_id: Some("client-xyz".to_string()),
        resource: Some(RESOURCE.to_string()),
        issuer: Some(PLANE_ISSUER.to_string()),
        audience_mode: None,
        enterprise_url: None,
    };
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
        (PLANE_META_URL, 200, None, &json_response(&plane_meta())),
        (
            PLANE_TOKEN,
            200,
            None,
            &json_response(&serde_json::json!({
                "access_token": "access-2",
                "expires_in": 1800,
            })),
        ),
    ]);
    let refreshed = mcp_refresh_token(&http, &config("plane", RESOURCE), &credentials)
        .await
        .unwrap();
    match &refreshed {
        AuthCredential::Oauth {
            access,
            refresh,
            endpoint,
            resource,
            issuer,
            token_endpoint,
            ..
        } => {
            assert_eq!(access, "access-2");
            // Some servers omit refresh_token on refresh; the prior one
            // stays.
            assert_eq!(refresh.as_deref(), Some("old-refresh"));
            assert_eq!(endpoint.as_deref(), Some(RESOURCE));
            assert_eq!(resource.as_deref(), Some(RESOURCE));
            assert_eq!(issuer.as_deref(), Some(PLANE_ISSUER));
            assert_eq!(token_endpoint.as_deref(), Some(PLANE_TOKEN));
        }
        other => panic!("oauth credential expected, got {other:?}"),
    }
    // The refresh request carried the resource and the stored client.
    let token_request = http
        .seen
        .lock()
        .unwrap()
        .iter()
        .find(|(url, _)| url == PLANE_TOKEN)
        .cloned()
        .unwrap();
    let body = token_request.1.unwrap();
    assert!(body.contains("grant_type=refresh_token"));
    assert!(body.contains("refresh_token=old-refresh"));
    assert!(body.contains("client_id=client-xyz"));

    // A different endpoint requires a fresh login.
    let retargeted = AuthCredential::Oauth {
        access: "a".to_string(),
        refresh: Some("r".to_string()),
        expires: 0,
        account_id: None,
        endpoint: Some("https://other.test/mcp".to_string()),
        token_endpoint: None,
        client_id: None,
        resource: None,
        issuer: None,
        audience_mode: None,
        enterprise_url: None,
    };
    let error = mcp_refresh_token(&http, &config("plane", RESOURCE), &retargeted)
        .await
        .unwrap_err()
        .to_string();
    assert!(error.contains("not bound"), "{error}");

    // A stored token endpoint that drifted from current metadata is
    // refused without ever being contacted.
    let drifted = AuthCredential::Oauth {
        access: "a".to_string(),
        refresh: Some("r".to_string()),
        expires: 0,
        account_id: None,
        endpoint: Some(RESOURCE.to_string()),
        token_endpoint: Some("https://attacker.example/token".to_string()),
        client_id: Some("client-xyz".to_string()),
        resource: Some(RESOURCE.to_string()),
        issuer: Some(PLANE_ISSUER.to_string()),
        audience_mode: None,
        enterprise_url: None,
    };
    let error = mcp_refresh_token(&http, &config("plane", RESOURCE), &drifted)
        .await
        .unwrap_err()
        .to_string();
    assert!(error.contains("token endpoint does not match"), "{error}");
    let urls: Vec<String> = http
        .seen
        .lock()
        .unwrap()
        .iter()
        .map(|(url, _)| url.clone())
        .collect();
    assert!(!urls.contains(&"https://attacker.example/token".to_string()));
}
/// A discovery-mode change (origin-only credentials against a
/// resource-bound server) requires a fresh login.
#[tokio::test]
async fn discovery_mode_change_requires_relogin() {
    let credentials = AuthCredential::Oauth {
        access: "origin-access".to_string(),
        refresh: Some("origin-refresh".to_string()),
        expires: 0,
        account_id: None,
        endpoint: Some(RESOURCE.to_string()),
        token_endpoint: Some(PLANE_TOKEN.to_string()),
        client_id: Some("origin-client".to_string()),
        resource: None,
        issuer: None,
        audience_mode: None,
        enterprise_url: None,
    };
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
        (PLANE_META_URL, 200, None, &json_response(&plane_meta())),
    ]);
    let error = mcp_refresh_token(&http, &config("plane", RESOURCE), &credentials)
        .await
        .unwrap_err()
        .to_string();
    assert!(error.contains("discovery mode changed"), "{error}");
    let urls: Vec<String> = http
        .seen
        .lock()
        .unwrap()
        .iter()
        .map(|(url, _)| url.clone())
        .collect();
    assert!(!urls.contains(&PLANE_TOKEN.to_string()));
}
