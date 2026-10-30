//! Client registration: a pre-registered client skips DCR, a pinned callback
//! port is what gets advertised, and an absent registration endpoint fails
//! with the actionable message.

use url::Url;

use super::fixtures::*;
use crate::auth::types::AuthCredential;
use crate::mcp::oauth::mcp_login;

#[tokio::test]
async fn zero_callback_port_fails_before_any_oauth_request() {
    for client_id in [None, Some("fixture-client".to_string())] {
        let http = ScriptedHttp::new(vec![]);
        let ui = test_ui();
        let mut login_config = config("fixture", ORIGIN_URL);
        login_config.client_id = client_id;
        login_config.callback_port = Some(0);
        let error = mcp_login(&http, &login_config, &ui).await.unwrap_err();
        assert!(error.to_string().contains("callbackPort"));
        assert!(http.seen.lock().unwrap().is_empty());
    }
}
#[tokio::test]
async fn pre_registered_client_skips_registration_and_pins_the_redirect() {
    let probe = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = probe.local_addr().unwrap().port();
    drop(probe);
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
            &json_response(&serde_json::json!({ "access_token": "root-access" })),
        ),
    ]);
    let ui = test_ui();
    let mut login_config = config("root", ORIGIN_URL);
    login_config.client_id = Some("root-client".to_string());
    login_config.callback_port = Some(port);
    let credentials = mcp_login(&http, &login_config, &ui).await.unwrap();
    match &credentials {
        AuthCredential::Oauth {
            access, client_id, ..
        } => {
            assert_eq!(access, "root-access");
            assert_eq!(client_id.as_deref(), Some("root-client"));
        }
        other => panic!("oauth credential expected, got {other:?}"),
    }
    let urls: Vec<String> = http
        .seen
        .lock()
        .unwrap()
        .iter()
        .map(|(url, _)| url.clone())
        .collect();
    assert!(!urls.contains(&ORIGIN_REGISTER.to_string()));
    let redirect_uri = format!("http://localhost:{port}/callback");
    let auth_url = Url::parse(&ui.auth_url.lock().unwrap()).unwrap();
    let redirect = auth_url
        .query_pairs()
        .find(|(key, _)| key == "redirect_uri")
        .map(|(_, value)| value.to_string())
        .unwrap();
    assert_eq!(redirect, redirect_uri);
    let token_request = http
        .seen
        .lock()
        .unwrap()
        .iter()
        .find(|(url, _)| url == ORIGIN_TOKEN)
        .cloned()
        .unwrap();
    let body = token_request.1.unwrap();
    let encoded_redirect = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("redirect_uri", &redirect_uri)
        .finish();
    assert!(body.contains(&encoded_redirect));
}
#[tokio::test]
async fn dynamic_registration_advertises_the_pinned_redirect() {
    let probe = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = probe.local_addr().unwrap().port();
    drop(probe);
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
            &json_response(&serde_json::json!({ "client_id": "range-client" })),
        ),
        (
            ORIGIN_TOKEN,
            200,
            None,
            &json_response(&serde_json::json!({ "access_token": "range-access" })),
        ),
    ]);
    let ui = test_ui();
    let mut login_config = config("origin", ORIGIN_URL);
    login_config.callback_port = Some(port);
    let credentials = mcp_login(&http, &login_config, &ui).await.unwrap();
    match &credentials {
        AuthCredential::Oauth { access, .. } => assert_eq!(access, "range-access"),
        other => panic!("oauth credential expected, got {other:?}"),
    }
    let registration_request = http
        .seen
        .lock()
        .unwrap()
        .iter()
        .find(|(url, _)| url == ORIGIN_REGISTER)
        .cloned()
        .unwrap();
    let registration_body =
        serde_json::from_str::<serde_json::Value>(&registration_request.1.unwrap()).unwrap();
    assert_eq!(
        registration_body.get("redirect_uris"),
        Some(&serde_json::json!([format!(
            "http://localhost:{port}/callback"
        )]))
    );
}
