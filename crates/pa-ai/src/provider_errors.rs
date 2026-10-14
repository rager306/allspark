//! Fork-only provider error normalization.
// FORK-ONLY EXTENSIONS: normalize_genspark/normalize_codebuddy/is_fork_provider
// are fork-specific. Upstream has no Genspark/CodeBuddy/Muse providers.
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
// FORK-ONLY: not in upstream.
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
    // Already canonical (`error.{type,message}`): leave the shared path alone —
    // except a relayed 5xx whose only clue is the message text (see
    // `recover_relayed_upstream_status`).
    if parsed.get("error").is_some_and(Value::is_object) {
        return recover_relayed_upstream_status(ProviderError::Http(http));
    }

    let normalized = match provider {
        "codebuddy" => normalize_codebuddy(http, &parsed),
        "genspark" => normalize_genspark(http, &parsed),
        // Muse speaks the canonical Anthropic/Responses envelopes.
        _ => ProviderError::Http(http),
    };
    recover_relayed_upstream_status(normalized)
}

/// A relayed 5xx (a router in front of the provider turning an upstream 429
/// into a synthetic 503) keeps the real status only in the message text. The
/// shared classifier reads `error.type`/`error.code`, so without this the
/// failure lands as a server error and never reaches the quota-park seam.
/// Re-derive the canonical `type` from the embedded status/phrasing.
fn recover_relayed_upstream_status(error: ProviderError) -> ProviderError {
    let ProviderError::Http(mut http) = error else {
        return error;
    };
    let Some(status) = http.status else {
        return ProviderError::Http(http);
    };
    if status < 500 {
        return ProviderError::Http(http);
    }
    let Some(message) = http
        .body
        .as_deref()
        .and_then(extract_message_text)
        .or_else(|| Some(http.message.clone()))
    else {
        return ProviderError::Http(http);
    };
    let embedded = embedded_upstream_status(&message);
    let is_limit = embedded == Some(429) || (embedded.is_none() && is_rate_limit_text(&message));
    if !is_limit {
        return ProviderError::Http(http);
    }
    let canonical = json!({
        "error": {
            "type": "rate_limit_exceeded",
            "message": message,
        }
    })
    .to_string();
    http.body = Some(canonical);
    ProviderError::Http(http)
}

/// The human text of a canonical error body (`error.message`, else `message`,
/// else `detail`).
fn extract_message_text(body: &str) -> Option<String> {
    let parsed = serde_json::from_str::<Value>(body).ok()?;
    let node = parsed.get("error").unwrap_or(&parsed);
    node.get("message")
        .or_else(|| parsed.get("message"))
        .or_else(|| parsed.get("detail"))
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// `CodeBuddy`: `{"code":11102,"msg":"model [...] not found","displayMsg":{...}}`.
///
/// The gateway returns HTTP 400 for every class, so the numeric `code` decides
/// the canonical `type` the shared classifier reads; the message prefers `msg`
/// (carries the reset time for 6004) and falls back to `displayMsg.en`.
// FORK-ONLY: not in upstream.
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
    // The text markers are checked before the code default: the gateway
    // wraps upstream failures in its own envelope, and a credit/quota
    // message is a payment failure even when the numeric code is one we
    // do not enumerate. A 5xx status is a server fault, never a request
    // rejection (an invalid_request_error there would never be retried).
    let msg_lower = msg.as_deref().unwrap_or("").to_lowercase();
    let credits_exhausted = [
        "insufficient credit",
        "insufficient_balance",
        "quota exceeded",
    ]
    .iter()
    .any(|marker| msg_lower.contains(marker));
    let error_type = match code {
        Some(6004) => "rate_limit_exceeded",
        // 11128 is the gateway's security-policy block ("Illegal API invocation
        // from an unapproved channel"): a content/policy rejection, not a bad
        // request. The canonical type reuses the shared safety classifier's
        // `prohibited_content` marker.
        Some(11128) => "prohibited_content",
        // 11101 (non-stream), 11102 (delisted model), and friends are request
        // rejections.
        _ if credits_exhausted => "insufficient_credits",
        _ if http.status.is_some_and(|status| status >= 500) => "server_error",
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
// FORK-ONLY: not in upstream.
fn normalize_genspark(mut http: ProviderHttpError, parsed: &Value) -> ProviderError {
    let Some(detail) = parsed.get("detail").and_then(Value::as_str) else {
        return ProviderError::Http(http);
    };
    // LiteLLM reports auth failures as HTTP 401 with the plain detail string; the
    // shared classifier already maps 401 -> auth, so the type only adds the
    // human-readable text.
    // A 5xx carries a server-side failure even when the JSON body is a
    // plain detail string: classifying it as invalid_request_error would
    // make the retry ladder treat a transient gateway fault as permanent.
    // The credit markers in the text still win (a drained wallet is a
    // payment failure at any status).
    let detail_lower = detail.to_lowercase();
    let credits_exhausted = [
        "insufficient credit",
        "insufficient_balance",
        "quota exceeded",
    ]
    .iter()
    .any(|marker| detail_lower.contains(marker));
    let error_type = match http.status {
        Some(401) => "authentication_error",
        Some(403) => "permission_error",
        Some(402) => "insufficient_credits",
        Some(429) => "rate_limit_exceeded",
        _ if credits_exhausted => "insufficient_credits",
        Some(status) if status >= 500 => "server_error",
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

/// Recover a server-requested wait from an error *message* when the response
/// carried no `Retry-After` header. Some providers report the reset only in the
/// body text (`zai` `1308`: "Usage limit reached for 5 hour. Your limit will
/// reset at 2026-10-12 19:10:00"), which the shared classifier reads for the
/// kind but not for the wait — so the quota-park seam never sees a reset time
/// and the session gives up instead of parking until the window rolls over.
///
/// Applies to every provider (a non-2xx error body is the only input), rewriting
/// the wait onto the error when a reset stamp parses. A body with no stamp is
/// untouched.
#[must_use]
pub fn recover_reset_wait_from_message(error: ProviderError) -> ProviderError {
    let ProviderError::Http(mut http) = error else {
        return error;
    };
    if http.retry_after_ms.is_some() {
        return ProviderError::Http(http);
    }
    // Only a rate-limit-class failure parks; do not invent a wait elsewhere.
    let is_rate_limit = http.status == Some(429)
        || http.body.as_deref().is_some_and(|body| {
            let lowered = body.to_lowercase();
            lowered.contains("rate limit")
                || lowered.contains("usage limit")
                || lowered.contains("too many requests")
        });
    if !is_rate_limit {
        return ProviderError::Http(http);
    }
    let Some(message) = http
        .body
        .as_deref()
        .and_then(extract_message_text)
        .or_else(|| Some(http.message.clone()))
    else {
        return ProviderError::Http(http);
    };
    let Some(wait_ms) = reset_wait_ms(&message) else {
        return ProviderError::Http(http);
    };
    let seconds = wait_ms.div_ceil(1000);
    http.headers
        .insert("retry-after".to_string(), seconds.to_string());
    http.retry_after_ms = Some(wait_ms);
    ProviderError::Http(http)
}

/// Parse a "reset at `YYYY-MM-DD HH:MM:SS`" stamp from an error message into a
/// millisecond wait from now.
///
/// The stamp carries no timezone. `zai` reports it in Beijing time (UTC+8): the
/// message states a five-hour window, and the observed stamp sat ~1h ahead of
/// UTC+8 (and ~9h ahead of UTC — impossible inside a five-hour window). So the
/// UTC+8 reading is tried first, and the candidate whose wait fits a plausible
/// window (positive and at most `MAX_PLAUSIBLE_WINDOW_MS`) wins; a stamp that
/// only reads as a sane wait in UTC falls back to UTC.
fn reset_wait_ms(message: &str) -> Option<u64> {
    const MAX_PLAUSIBLE_WINDOW_MS: u64 = 6 * 60 * 60 * 1000;
    let (year, month, day, hour, minute, second) = find_stamp(message)?;
    let epoch_seconds =
        days_from_civil(year, month, day) * 86_400 + hour * 3600 + minute * 60 + second;
    #[allow(clippy::cast_possible_wrap)]
    let now_seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_secs() as i64;
    // UTC+8 first (zai's reported zone), then UTC.
    let candidates = [epoch_seconds - 8 * 3600, epoch_seconds];
    let mut best: Option<u64> = None;
    for candidate in candidates {
        let wait_seconds = candidate - now_seconds;
        if wait_seconds <= 0 {
            continue;
        }
        #[allow(clippy::cast_sign_loss)]
        let wait_ms = (wait_seconds as u64) * 1000;
        if wait_ms <= MAX_PLAUSIBLE_WINDOW_MS {
            return Some(wait_ms);
        }
        best = Some(best.map_or(wait_ms, |current: u64| current.min(wait_ms)));
    }
    best
}

/// Find a `YYYY-MM-DD HH:MM:SS` stamp in `message`, returning its parts.
fn find_stamp(message: &str) -> Option<(i64, i64, i64, i64, i64, i64)> {
    let bytes = message.as_bytes();
    let mut index = 0;
    while index + 19 <= bytes.len() {
        if is_stamp_at(message, index) {
            let stamp = &message[index..index + 19];
            let (date, time) = stamp.split_once(' ')?;
            let (year, month, day) = parse_date(date)?;
            let (hour, minute, second) = parse_time(time)?;
            return Some((year, month, day, hour, minute, second));
        }
        index += 1;
    }
    None
}

/// A gateway relaying an upstream failure inside a 5xx envelope: 9router turns
/// a real upstream 429 into a synthetic 503 with `[429]` (or `[provider/model]
/// [429]`) inside the message text and a `Retry-After` header, so the shared
/// classifier — which reads only `error.type`/`error.code` — would call it a
/// server error and skip the quota-park seam (9router #4663). Recover the
/// embedded upstream status from the message.
fn embedded_upstream_status(message: &str) -> Option<u16> {
    // The relayed shape: `[provider/model] [429]: ...`. Take the last bracket
    // group that is a 3-digit HTTP status so a model id like `[1m]` is skipped.
    let mut status = None;
    let bytes = message.as_bytes();
    let mut index = 0;
    while index + 3 <= bytes.len() {
        if bytes[index] == b'[' {
            let rest = &message[index + 1..];
            if let Some(close) = rest.find(']') {
                if let Ok(code) = rest[..close].parse::<u16>() {
                    if (400..=599).contains(&code) {
                        status = Some(code);
                    }
                }
                index += close + 2;
                continue;
            }
        }
        index += 1;
    }
    status
}

/// Rate-limit phrasings a relayed envelope carries when it does not embed a
/// numeric status (`too many requests`, `rate limit`).
fn is_rate_limit_text(message: &str) -> bool {
    let lower = message.to_lowercase();
    lower.contains("too many requests") || lower.contains("rate limit")
}

/// Credit/balance exhaustion markers gateways use in `error.type`/`error.code`
/// without an HTTP 402 (`LiteLLM`, and the fork gateways that mirror it).
pub(crate) fn is_payment_signal(provider_error_type: &str) -> bool {
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
    fn a_relayed_upstream_429_in_a_5xx_envelope_classifies_as_rate_limit() {
        // 9router relays a real upstream 429 as a synthetic 503, keeping the
        // status only in the message text (9router #4663).
        let body = serde_json::json!({
            "error": {
                "message": "[codebuddy-intl/deepseek-v4.1-flash] [429]: {\"code\":14003,\"msg\":\"too many requests\"} (reset after 2s)"
            }
        })
        .to_string();
        let error = normalize_provider_http_error("codebuddy", http_error(503, &body));
        let info = extract_stream_failure_info(&error);
        assert_eq!(info.kind, StreamFailureKind::RateLimit);
    }

    #[test]
    fn a_relayed_rate_limit_phrase_without_a_status_classifies_as_rate_limit() {
        let body = serde_json::json!({
            "error": { "message": "upstream said too many requests, slow down" }
        })
        .to_string();
        let error = normalize_provider_http_error("genspark", http_error(502, &body));
        assert_eq!(
            extract_stream_failure_info(&error).kind,
            StreamFailureKind::RateLimit
        );
    }

    #[test]
    fn a_genuine_5xx_without_a_rate_limit_marker_stays_a_server_error() {
        let body = serde_json::json!({
            "error": { "message": "upstream connection reset by peer" }
        })
        .to_string();
        let error = normalize_provider_http_error("codebuddy", http_error(502, &body));
        assert_eq!(
            extract_stream_failure_info(&error).kind,
            StreamFailureKind::ServerError
        );
    }

    #[test]
    fn a_zai_reset_stamp_becomes_a_retry_wait() {
        // zai reports the reset only in the body text, in Beijing time.
        let body = r#"{"error":{"code":"1308","message":"Usage limit reached for 5 hour. Your limit will reset at 2035-10-12 19:10:00"}}"#;
        let error = recover_reset_wait_from_message(http_error(429, body));
        let info = extract_stream_failure_info(&error);
        assert_eq!(info.kind, StreamFailureKind::RateLimit);
        assert!(
            info.retry_after_ms.is_some_and(|wait| wait > 0),
            "the reset stamp must yield a wait: {info:?}"
        );
    }

    #[test]
    fn a_non_rate_limit_error_gets_no_invented_wait() {
        // A 400 invalid-request body mentioning a timestamp must not park.
        let body = r#"{"error":{"message":"bad request at 2035-10-12 19:10:00"}}"#;
        let error = recover_reset_wait_from_message(http_error(400, body));
        assert_eq!(extract_stream_failure_info(&error).retry_after_ms, None);
    }

    #[test]
    fn an_existing_retry_after_header_wins() {
        let body = r#"{"error":{"message":"rate limit; reset at 2035-10-12 19:10:00"}}"#;
        let mut error = http_error(429, body);
        if let ProviderError::Http(http) = &mut error {
            http.retry_after_ms = Some(1234);
        }
        let error = recover_reset_wait_from_message(error);
        assert_eq!(
            extract_stream_failure_info(&error).retry_after_ms,
            Some(1234)
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
        // A stamp far in the future: the wait is measured from `now`, so a
        // near-date literal would turn this into a time bomb.
        let stamp = "2099-01-01 00:00:00 UTC+8";
        let wait = parse_reset_stamp_ms(stamp).expect("stamp parses");
        assert!(wait > 0);
        assert_eq!(parse_offset_minutes("+8"), Some(480));
        assert_eq!(parse_offset_minutes("+08:00"), Some(480));
        assert_eq!(parse_offset_minutes("-05:30"), Some(-330));
    }
    #[test]
    fn genspark_5xx_detail_is_a_server_fault_not_a_request_rejection() {
        // A gateway fault carrying a plain detail string must classify as a
        // server error so the retry ladder retries it; before the fix the
        // `_` arm made it invalid_request_error (never retried).
        let error = normalize_provider_http_error(
            "genspark",
            http_error(500, r#"{"detail":"upstream connect error"}"#),
        );
        assert_eq!(
            extract_stream_failure_info(&error).kind,
            StreamFailureKind::ServerError
        );
    }

    #[test]
    fn genspark_5xx_with_credit_text_still_classifies_as_payment() {
        // The credit markers win over the status: a drained wallet is a
        // payment failure at any status, so failover reroutes it.
        let error = normalize_provider_http_error(
            "genspark",
            http_error(500, r#"{"detail":"Insufficient credits"}"#),
        );
        assert_eq!(
            extract_stream_failure_info(&error).kind,
            StreamFailureKind::PaymentRequired
        );
    }

    #[test]
    fn codebuddy_5xx_is_a_server_fault_not_a_request_rejection() {
        let error = normalize_provider_http_error(
            "codebuddy",
            http_error(500, r#"{"code":9999,"msg":"internal error"}"#),
        );
        assert_eq!(
            extract_stream_failure_info(&error).kind,
            StreamFailureKind::ServerError
        );
    }

    #[test]
    fn codebuddy_credit_text_classifies_as_payment_at_any_status() {
        let error = normalize_provider_http_error(
            "codebuddy",
            http_error(500, r#"{"code":9999,"msg":"Insufficient credits"}"#),
        );
        assert_eq!(
            extract_stream_failure_info(&error).kind,
            StreamFailureKind::PaymentRequired
        );
    }
}
