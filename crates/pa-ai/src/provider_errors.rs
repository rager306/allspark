//! Fork-only provider error normalization.
//!
//! The fork's providers (`muse`, `codebuddy`, `genspark`) ship error envelopes
//! the shared OpenAI/Anthropic classifiers do not read:
//!
//! - `CodeBuddy`: `{"code":<int>,"msg":"...","displayMsg":{"en","zh"}}` over HTTP
//!   400 for *every* failure class — a 11128 security block, a 6004 frequency
//!   limit, a 11102 delisted model. The body text and the retry/reset
//!   information live in `msg`, and the numeric `code` never lands in
//!   [`StreamFailureInfo::provider_error_type`] (the shared extractor only reads
//!   *string* `type`/`code` fields).
//! - `Genspark` (`LiteLLM`): `{"detail":"..."}`.
//!
//! [`normalize_provider_http_error`] rewrites those envelopes into the canonical
//! `{"error":{"type","message","code"}}` shape (plus a `Retry-After` header when
//! the body carries a reset time), so one call site per fork provider keeps the
//! shared classification, retry and failover machinery intact.

use std::collections::HashMap;

use serde_json::{json, Value};

use crate::utils_inner::stream_failure::{
    classify_stream_failure, ProviderError, ProviderHttpError, StreamFailureError,
    StreamFailureInfo, StreamFailureKind,
};

/// The providers this module normalizes.
#[must_use]
pub fn is_fork_provider(provider: &str) -> bool {
    matches!(provider, "muse" | "codebuddy" | "genspark")
}

/// Rewrite a fork provider's HTTP error body into the canonical error envelope,
/// when it is one of the shapes the shared classifier cannot read. Every other
/// provider (and body) passes through untouched.
#[must_use]
pub fn normalize_provider_http_error(provider: &str, error: ProviderError) -> ProviderError {
    if !is_fork_provider(provider) {
        return error;
    }
    let ProviderError::Http(http) = error else {
        return error;
    };
    let Some(body) = http.body.as_deref() else {
        return ProviderError::Http(http);
    };
    let Ok(parsed) = serde_json::from_str::<Value>(body) else {
        return ProviderError::Http(http);
    };
    // Already canonical (`error.{type,message}`): leave the shared path alone.
    if parsed.get("error").is_some_and(Value::is_object) {
        return ProviderError::Http(http);
    }

    match provider {
        "codebuddy" => normalize_codebuddy(http, &parsed),
        "genspark" => normalize_genspark(http, &parsed),
        // Muse speaks the canonical Anthropic/Responses envelopes.
        _ => ProviderError::Http(http),
    }
}

/// `CodeBuddy`: `{"code":11102,"msg":"model [...] not found","displayMsg":{...}}`.
///
/// The gateway returns HTTP 400 for every class, so the numeric `code` decides
/// the canonical `type` the shared classifier reads; the message prefers `msg`
/// (carries the reset time for 6004) and falls back to `displayMsg.en`.
fn normalize_codebuddy(mut http: ProviderHttpError, parsed: &Value) -> ProviderError {
    let code = parsed.get("code").and_then(Value::as_i64);
    let msg = parsed
        .get("msg")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| {
            parsed
                .get("displayMsg")
                .and_then(|display| display.get("en"))
                .and_then(Value::as_str)
                .map(str::to_string)
        });
    let error_type = match code {
        Some(6004) => "rate_limit_exceeded",
        // 11128 is the gateway's security-policy block ("Illegal API invocation
        // from an unapproved channel"): a content/policy rejection, not a bad
        // request. The canonical type reuses the shared safety classifier's
        // `prohibited_content` marker.
        Some(11128) => "prohibited_content",
        // 11101 (non-stream), 11102 (delisted model), and friends are request
        // rejections.
        _ => "invalid_request_error",
    };

    if let Some(msg) = &msg {
        let canonical = json!({
            "error": {
                "type": error_type,
                "message": msg,
                "code": code,
            }
        })
        .to_string();
        http.message = format!("{} {}", http.status.unwrap_or(0), msg);
        http.body = Some(canonical);
    }
    // The 6004 body carries the reset time as a `YYYY-MM-DD HH:MM:SS UTC+8`
    // stamp; convert it into the `Retry-After` wait the shared retry policy
    // reads (it only looks at headers).
    if code == Some(6004) {
        if let Some(wait_ms) = msg.as_deref().and_then(parse_reset_stamp_ms) {
            let seconds = wait_ms.div_ceil(1000);
            http.headers
                .insert("retry-after".to_string(), seconds.to_string());
            http.retry_after_ms = Some(wait_ms);
        }
    }
    ProviderError::Http(http)
}

/// `Genspark` (`LiteLLM`): `{"detail":"Invalid or expired token"}`.
fn normalize_genspark(mut http: ProviderHttpError, parsed: &Value) -> ProviderError {
    let Some(detail) = parsed.get("detail").and_then(Value::as_str) else {
        return ProviderError::Http(http);
    };
    // LiteLLM reports auth failures as HTTP 401 with the plain detail string; the
    // shared classifier already maps 401 -> auth, so the type only adds the
    // human-readable text.
    let error_type = match http.status {
        Some(401) => "authentication_error",
        Some(403) => "permission_error",
        Some(402) => "insufficient_credits",
        Some(429) => "rate_limit_exceeded",
        _ => "invalid_request_error",
    };
    http.message = format!("{} {detail}", http.status.unwrap_or(0));
    http.body = Some(json!({ "error": { "type": error_type, "message": detail } }).to_string());
    ProviderError::Http(http)
}

/// Parse a `CodeBuddy` reset stamp (`2026-10-13 00:00:00 UTC+8`, also `+08:00`
/// or a bare `UTC`) into a millisecond wait from now. The gateway formats the
/// stamp in its own timezone, so the offset is part of the value.
fn parse_reset_stamp_ms(message: &str) -> Option<u64> {
    let bytes = message.as_bytes();
    let mut found = None;
    // The stamp is `YYYY-MM-DD HH:MM:SS` optionally followed by an offset.
    let mut index = 0;
    while index + 19 <= bytes.len() {
        if is_stamp_at(message, index) {
            found = Some(index);
            break;
        }
        index += 1;
    }
    let start = found?;
    let stamp = &message[start..start + 19];
    let (date, time) = stamp.split_once(' ')?;
    let (year, month, day) = parse_date(date)?;
    let (hour, minute, second) = parse_time(time)?;
    let offset_minutes = message[start + 19..]
        .trim_start()
        .strip_prefix("UTC")
        .and_then(parse_offset_minutes)
        .unwrap_or(0);
    let epoch_seconds =
        days_from_civil(year, month, day) * 86_400 + hour * 3600 + minute * 60 + second
            - offset_minutes * 60;
    let now_seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_secs();
    let wait_seconds = epoch_seconds.max(0) - now_seconds.cast_signed();
    u64::try_from(wait_seconds).ok().map(|wait| wait * 1000)
}

fn is_stamp_at(message: &str, index: usize) -> bool {
    let bytes = message.as_bytes();
    // `YYYY-MM-DD HH:MM:SS`: digits at every position except the separators
    // (4 `-`, 7 `-`, 10 ` `, 13 `:`, 16 `:`).
    let digits = [0, 1, 2, 3, 5, 6, 8, 9, 11, 12, 14, 15, 17, 18];
    if !digits
        .iter()
        .all(|offset| bytes[index + offset].is_ascii_digit())
    {
        return false;
    }
    bytes[index + 4] == b'-'
        && bytes[index + 7] == b'-'
        && bytes[index + 10] == b' '
        && bytes[index + 13] == b':'
        && bytes[index + 16] == b':'
}

fn parse_date(date: &str) -> Option<(i64, i64, i64)> {
    let mut parts = date.split('-');
    let year = parts.next()?.parse().ok()?;
    let month = parts.next()?.parse().ok()?;
    let day = parts.next()?.parse().ok()?;
    (parts.next().is_none()).then_some((year, month, day))
}

fn parse_time(time: &str) -> Option<(i64, i64, i64)> {
    let mut parts = time.split(':');
    let hour = parts.next()?.parse().ok()?;
    let minute = parts.next()?.parse().ok()?;
    let second = parts.next()?.parse().ok()?;
    (parts.next().is_none()).then_some((hour, minute, second))
}

/// `+8`, `+08`, `+08:00`, `-05:30` -> minutes east of UTC.
fn parse_offset_minutes(raw: &str) -> Option<i64> {
    let raw = raw.trim();
    let (sign, rest) = match raw.strip_prefix('-') {
        Some(rest) => (-1, rest),
        None => (1, raw.strip_prefix('+').unwrap_or(raw)),
    };
    let (hours, minutes): (i64, i64) = match rest.split_once(':') {
        Some((hours, minutes)) => (hours.parse().ok()?, minutes.parse().ok()?),
        None => (rest.parse().ok()?, 0),
    };
    Some(sign * (hours * 60 + minutes))
}

fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Classify a mid-stream `{"error":...}` SSE frame (an HTTP 200 stream that
/// fails *after* it starts: quota exhausted, overload, an upstream `LiteLLM`
/// error). Returns the structured failure the stream must terminate with.
///
/// The fork's `genspark` (`LiteLLM`) and `codebuddy` gateways both surface
/// quota/limit failures as in-stream frames, so a silent ignore would let the
/// turn settle as a `stream_drop` instead of a classified rate limit.
#[must_use]
pub fn stream_failure_from_error_frame(provider: &str, frame: &Value) -> StreamFailureError {
    // The frame may nest the error (`{"error":{...}}`) or carry it at the top
    // level with the provider's own fields (`{"code":6004,"msg":...}`).
    let normalized = if is_fork_provider(provider) && frame.get("error").is_none() {
        let http = ProviderHttpError {
            message: String::new(),
            status: None,
            body: Some(frame.to_string()),
            headers: HashMap::new(),
            request_id: None,
            sdk_name: None,
            retry_after_ms: None,
            provider_error_type: None,
        };
        match normalize_provider_http_error(provider, ProviderError::Http(http)) {
            ProviderError::Http(updated) => {
                serde_json::from_str::<Value>(updated.body.as_deref().unwrap_or(""))
                    .unwrap_or(Value::Null)
            }
            _ => frame.clone(),
        }
    } else {
        frame.clone()
    };
    let error = normalized.get("error").unwrap_or(&normalized);
    let provider_error_type = error
        .get("type")
        .and_then(Value::as_str)
        .or_else(|| error.get("code").and_then(Value::as_str))
        .map(str::to_string);
    let message = error
        .get("message")
        .and_then(Value::as_str)
        .or_else(|| normalized.get("detail").and_then(Value::as_str))
        .map_or_else(|| "Provider error frame".to_string(), str::to_string);
    let retry_after_ms = if is_fork_provider(provider) {
        retry_after_from_error_frame(provider, frame)
    } else {
        None
    };
    // A credits/balance exhaustion signal (LiteLLM and friends) with no HTTP
    // status: the shared classifier only maps these through a 402, so pin the
    // payment kind here — a drained wallet never refills inside a retry ladder.
    let kind = match provider_error_type.as_deref() {
        Some(kind) if is_payment_signal(kind) => StreamFailureKind::PaymentRequired,
        other => classify_stream_failure(other, None),
    };
    StreamFailureError {
        message,
        info: StreamFailureInfo {
            kind,
            provider_error_type,
            retry_after_ms,
            ..StreamFailureInfo::unknown()
        },
    }
}

/// Credit/balance exhaustion markers gateways use in `error.type`/`error.code`
/// without an HTTP 402 (`LiteLLM`, and the fork gateways that mirror it).
fn is_payment_signal(provider_error_type: &str) -> bool {
    let kind = provider_error_type.to_lowercase();
    kind.contains("insufficient_credits")
        || kind.contains("insufficient_balance")
        || kind.contains("insufficient_funds")
        || kind.contains("insufficient_quota")
        || kind.contains("quota_exceeded")
        || kind.contains("credit_limit")
        || kind.contains("billing_hard_limit")
        || kind.contains("out_of_credits")
        || kind.contains("payment_required")
}

/// The wait a mid-stream error frame carries, when the provider's envelope
/// embeds one (`CodeBuddy`'s 6004 reset stamp).
fn retry_after_from_error_frame(provider: &str, frame: &Value) -> Option<u64> {
    if provider != "codebuddy" {
        return None;
    }
    let msg = frame.get("msg").and_then(Value::as_str)?;
    parse_reset_stamp_ms(msg)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::utils_inner::stream_failure::{extract_stream_failure_info, StreamFailureKind};

    fn http_error(status: u16, body: &str) -> ProviderError {
        ProviderError::Http(ProviderHttpError {
            message: format!("{status} {body}"),
            status: Some(status),
            body: Some(body.to_string()),
            headers: HashMap::new(),
            request_id: None,
            sdk_name: None,
            retry_after_ms: None,
            provider_error_type: None,
        })
    }

    fn body_of(error: &ProviderError) -> String {
        match error {
            ProviderError::Http(http) => http.body.clone().unwrap_or_default(),
            other => panic!("expected http error, got {other:?}"),
        }
    }

    #[test]
    fn codebuddy_delisted_model_becomes_a_readable_invalid_request() {
        let raw =
            r#"{"code":11102,"msg":"model [no-such] service info not found","requestId":"r1"}"#;
        let error = normalize_provider_http_error("codebuddy", http_error(400, raw));
        let info = extract_stream_failure_info(&error);
        assert_eq!(info.kind, StreamFailureKind::InvalidRequest);
        let body = body_of(&error);
        assert!(
            body.contains("model [no-such] service info not found"),
            "{body}"
        );
        assert!(
            crate::utils_inner::stream_failure::format_stream_failure_message(&error)
                .contains("model [no-such] service info not found")
        );
    }

    #[test]
    fn codebuddy_security_block_classifies_as_safety() {
        let raw = r#"{"code":11128,"msg":"Illegal API invocation from an unapproved channel"}"#;
        let error = normalize_provider_http_error("codebuddy", http_error(400, raw));
        assert_eq!(
            extract_stream_failure_info(&error).kind,
            StreamFailureKind::Safety
        );
    }

    #[test]
    fn codebuddy_frequency_limit_classifies_as_rate_limit_with_a_reset_wait() {
        let raw = r#"{"code":6004,"msg":"超出频率限制 frequency limit, resets 2035-10-13 00:00:00 UTC+8"}"#;
        let error = normalize_provider_http_error("codebuddy", http_error(400, raw));
        let info = extract_stream_failure_info(&error);
        assert_eq!(info.kind, StreamFailureKind::RateLimit);
        // A far-future stamp yields a positive wait, not a negative/zero one.
        assert!(info.retry_after_ms.is_some_and(|wait| wait > 0));
    }

    #[test]
    fn genspark_detail_body_keeps_its_text() {
        let error = normalize_provider_http_error(
            "genspark",
            http_error(401, r#"{"detail":"Invalid or expired token"}"#),
        );
        let info = extract_stream_failure_info(&error);
        assert_eq!(info.kind, StreamFailureKind::Auth);
        assert!(
            crate::utils_inner::stream_failure::format_stream_failure_message(&error)
                .contains("Invalid or expired token")
        );
    }

    #[test]
    fn genspark_insufficient_credits_is_a_payment_failure() {
        let error = normalize_provider_http_error(
            "genspark",
            http_error(402, r#"{"detail":"Insufficient credits"}"#),
        );
        assert_eq!(
            extract_stream_failure_info(&error).kind,
            StreamFailureKind::PaymentRequired
        );
    }

    #[test]
    fn canonical_and_foreign_bodies_pass_through_untouched() {
        let canonical = r#"{"error":{"type":"rate_limit_error","message":"slow down"}}"#;
        let error = normalize_provider_http_error("codebuddy", http_error(429, canonical));
        assert_eq!(body_of(&error), canonical);
        // A non-fork provider is never rewritten, even with a fork-shaped body.
        let fork_body = r#"{"code":11102,"msg":"x"}"#;
        let error = normalize_provider_http_error("openai", http_error(400, fork_body));
        assert_eq!(body_of(&error), fork_body);
    }

    #[test]
    fn mid_stream_error_frame_classifies_genspark_litellm_failures() {
        // A LiteLLM in-stream error frame (HTTP 200 already sent).
        let frame = serde_json::json!({
            "error": {"message": "Rate limit exceeded", "type": "rate_limit_error"}
        });
        let failure = stream_failure_from_error_frame("genspark", &frame);
        assert_eq!(failure.info.kind, StreamFailureKind::RateLimit);
        assert_eq!(
            failure.info.provider_error_type.as_deref(),
            Some("rate_limit_error")
        );
        assert_eq!(failure.message, "Rate limit exceeded");
    }

    #[test]
    fn mid_stream_error_frame_reads_a_codebuddy_failure_envelope() {
        // CodeBuddy's own envelope as an in-stream frame: numeric `code` plus
        // `msg`, with the 6004 reset stamp.
        let frame = serde_json::json!({
            "code": 6004,
            "msg": "超出频率限制 frequency limit, resets 2035-10-13 00:00:00 UTC+8"
        });
        let failure = stream_failure_from_error_frame("codebuddy", &frame);
        assert_eq!(failure.info.kind, StreamFailureKind::RateLimit);
        assert!(failure.info.retry_after_ms.is_some_and(|wait| wait > 0));
    }

    #[test]
    fn mid_stream_credit_exhaustion_is_a_payment_failure() {
        let frame = serde_json::json!({
            "error": {"message": "Budget exceeded for this account", "type": "insufficient_credits"}
        });
        let failure = stream_failure_from_error_frame("genspark", &frame);
        assert_eq!(failure.info.kind, StreamFailureKind::PaymentRequired);
    }

    #[test]
    fn mid_stream_error_frame_for_other_providers_stays_generic() {
        let frame = serde_json::json!({
            "error": {"message": "boom", "type": "server_error"}
        });
        let failure = stream_failure_from_error_frame("openai", &frame);
        assert_eq!(failure.info.kind, StreamFailureKind::ServerError);
        assert_eq!(failure.info.retry_after_ms, None);
    }

    #[test]
    fn parses_the_reset_stamp_with_offsets() {
        let stamp = "2026-10-13 00:00:00 UTC+8";
        let wait = parse_reset_stamp_ms(stamp).expect("stamp parses");
        // 2026-10-13T00:00:00+08:00 == 2026-10-12T16:00:00Z.
        assert!(wait > 0);
        assert_eq!(parse_offset_minutes("+8"), Some(480));
        assert_eq!(parse_offset_minutes("+08:00"), Some(480));
        assert_eq!(parse_offset_minutes("-05:30"), Some(-330));
    }
}
