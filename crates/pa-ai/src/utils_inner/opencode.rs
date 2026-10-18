//! `OpenCode` free-tier client: the v2 protocol shape the free endpoint
//! requires (HTTP/2, stream:true, session-id triplet, fingerprint tools).
//!
//! Three hard requirements discovered by reverse-engineering the `OpenCode`
//! CLI (`Go`) and the `9router` proxy (`Node.js`) — any one missing = 403:
//! 1. HTTP/2 via ALPN (HTTP/1.1 is rejected);
//! 2. `stream: true` on every request (non-streaming is rejected);
//! 3. The v2 header shape: UA `opencode/<channel>/<version>/cli`,
//!    `x-opencode-client: cli`, session-id on three headers, no v1
//!    `x-opencode-request` header.
//!
//! Auth is `Bearer public` — no registration, no key.

use std::time::{SystemTime, UNIX_EPOCH};

/// The v2 User-Agent the upstream validates (major >= 2 required).
#[allow(dead_code)] // wired in the transport integration
pub const OPENCODE_UA: &str = "opencode/latest/2.0.26/cli";

/// Base62 alphabet for session-id random suffix.
const BASE62: &[u8; 62] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";

/// The fingerprint tools the free tier requires on every request.
#[allow(dead_code)] // wired in the transport integration
pub const FINGERPRINT_TOOLS: &[&str] = &["bash", "glob", "grep", "read"];

/// Generate a session id in the v2 format: `ses_` + 12 hex chars of the
/// bitwise-NOT timestamp (counter must be 0-16) + 14 base62 random chars.
#[must_use]
#[allow(dead_code)] // wired in the transport integration
pub fn generate_session_id() -> String {
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0u64, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX));
    let encoded = !(now_ms.wrapping_mul(0x1000).wrapping_add(1)) & 0xFFFF_FFFF_FFFF;
    let hex = format!("{encoded:012x}");
    let mut state = now_ms ^ u64::from(std::process::id());
    let mut random = String::with_capacity(14);
    for _ in 0..14 {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        let index = ((state >> 33) % 62).try_into().unwrap_or(0);
        random.push(BASE62[index] as char);
    }
    format!("ses_{hex}{random}")
}

/// The v2 headers for one request (the session rides three headers).
#[must_use]
#[allow(dead_code)] // wired in the transport integration
pub fn v2_headers(session: &str) -> Vec<(String, String)> {
    vec![
        ("User-Agent".to_string(), OPENCODE_UA.to_string()),
        ("x-opencode-client".to_string(), "cli".to_string()),
        ("x-opencode-session".to_string(), session.to_string()),
        ("x-opencode-session-id".to_string(), session.to_string()),
        ("x-session-affinity".to_string(), session.to_string()),
        ("x-session-id".to_string(), session.to_string()),
        ("x-opencode-project".to_string(), "global".to_string()),
    ]
}

/// Ensure the request body carries the fingerprint tools and `stream: true`.
#[allow(dead_code)] // wired in the transport integration
pub fn prepare_body(body: &mut serde_json::Value) {
    let Some(obj) = body.as_object_mut() else {
        return;
    };
    obj.insert("stream".to_string(), serde_json::json!(true));
    let tools = obj.entry("tools").or_insert_with(|| serde_json::json!([]));
    let Some(arr) = tools.as_array_mut() else {
        return;
    };
    let present: Vec<String> = arr
        .iter()
        .filter_map(|t| {
            t.get("function")
                .and_then(|f| f.get("name"))
                .and_then(|n| n.as_str())
                .map(String::from)
        })
        .collect();
    for name in FINGERPRINT_TOOLS {
        if !present.iter().any(|p| p == name) {
            arr.push(serde_json::json!({
                "type": "function",
                "function": {
                    "name": name,
                    "description": format!("OpenCode built-in {name} tool"),
                    "parameters": {"type": "object", "properties": {}}
                }
            }));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_id_matches_the_v2_shape() {
        let id = generate_session_id();
        assert!(id.starts_with("ses_"), "prefix: {id}");
        assert_eq!(id.len(), 4 + 12 + 14, "length: {id}");
        let hex = &id[4..16];
        let decoded = u64::from_str_radix(hex, 16).expect("hex prefix");
        assert_ne!(decoded, 0, "timestamp must be real");
    }

    #[test]
    fn fingerprint_tools_are_injected_when_missing() {
        let mut body = serde_json::json!({"model": "test", "messages": []});
        prepare_body(&mut body);
        assert_eq!(body["stream"], serde_json::json!(true));
        let tools = body["tools"].as_array().expect("tools");
        assert_eq!(tools.len(), 4, "all four fingerprint tools");
    }

    #[test]
    fn existing_tools_are_not_duplicated() {
        let mut body = serde_json::json!({
            "tools": [{"type":"function","function":{"name":"bash","description":"real","parameters":{}}}]
        });
        prepare_body(&mut body);
        let tools = body["tools"].as_array().expect("tools");
        assert_eq!(tools.len(), 4, "3 added, 1 existing, no dupes");
    }
}
