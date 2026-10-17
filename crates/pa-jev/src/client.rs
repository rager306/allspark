//! The judgment client: one System One endpoint behind one wire dialect,
//! with bounded retry, per-attempt timeout, and question chunking.
//!
//! Constants mirror the omp reference exactly: 10s timeout, 3 attempts,
//! backoff 500ms doubling to a 5s ceiling (a `retry-after` hint wins, capped
//! at the same ceiling), at most 4 chunks of one split judgment in flight.
//! A split judgment's first failing chunk aborts its in-flight siblings and
//! stops dispatch; the usage of chunks that already succeeded rides the
//! error ([`JevError::ChunkFailed`]) so a doomed judgment stops spending.
//!
//! Cancellation is drop-based: no in-flight signal handle exists, so dropping
//! the `judge` future cancels the request (the socket closes) - callers that
//! need a typed abort race the future against their own signal.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use tokio::sync::watch;

use crate::error::JevError;
use crate::types::{Answer, JudgmentRequest, JudgmentState, Question, Usage};
use crate::wire::{self, WireDialect};

/// Per-attempt timeout (the omp `DEFAULT_TIMEOUT_MS`).
pub const DEFAULT_TIMEOUT_MS: u64 = 10_000;
/// Attempts per judgment, transport failures and transient statuses included.
pub const MAX_ATTEMPTS: u32 = 3;
/// Backoff of the first retry; doubles each retry.
pub const BACKOFF_BASE_MS: u64 = 500;
/// Backoff ceiling (also caps a `retry-after` hint).
pub const BACKOFF_MAX_MS: u64 = 5_000;
/// Chunks of one split judgment in flight at once.
pub const CHUNK_PARALLEL: usize = 4;

/// Everything one endpoint needs to serve judgments.
#[derive(Debug, Clone)]
pub struct JevClientConfig {
    /// The wire dialect (route + body codec).
    pub dialect: WireDialect,
    /// API root; the dialect appends its path (`.../v1/systemone`).
    pub base_url: String,
    /// The wire model id sent with every request (pin a version).
    pub model: String,
    /// The catalog provider reported on results (e.g. `typesafe`).
    pub provider: String,
    /// Bearer credential.
    pub api_key: String,
    /// Per-attempt timeout.
    pub timeout: Duration,
    /// Most questions per request this endpoint accepts; larger requests
    /// split. `None`: no cap.
    pub max_questions: Option<usize>,
    /// Extra static headers (catalog per-model headers).
    pub extra_headers: BTreeMap<String, String>,
}

impl JevClientConfig {
    /// The config with defaults filled in (`timeout` 10s, no cap, no extra
    /// headers).
    #[must_use]
    pub fn new(
        dialect: WireDialect,
        base_url: impl Into<String>,
        model: impl Into<String>,
        provider: impl Into<String>,
        api_key: impl Into<String>,
    ) -> Self {
        JevClientConfig {
            dialect,
            base_url: base_url.into(),
            model: model.into(),
            provider: provider.into(),
            api_key: api_key.into(),
            timeout: Duration::from_millis(DEFAULT_TIMEOUT_MS),
            max_questions: None,
            extra_headers: BTreeMap::new(),
        }
    }
}

/// One judgment outcome: what answered and what it cost.
#[derive(Debug, Clone, PartialEq)]
pub struct JudgmentResult {
    /// The dialect's `api` spelling (e.g. `typesafe`).
    pub api: String,
    pub provider: String,
    /// The model that answered - the response's exact version when it names
    /// one, else the requested id.
    pub model: String,
    pub answers: BTreeMap<String, Answer>,
    pub usage: Usage,
}

/// The judgment client for one endpoint. Stateless per call; share freely.
pub struct JevClient {
    http: reqwest::Client,
    config: JevClientConfig,
}

impl JevClient {
    /// Build the client over `config`.
    ///
    /// # Panics
    ///
    /// Panics when the underlying reqwest client cannot be built (a TLS
    /// backend failure - never observed on the workspace's rustls builds).
    #[must_use]
    pub fn new(config: JevClientConfig) -> Self {
        JevClient {
            http: reqwest::Client::builder().build().expect("reqwest client"),
            config,
        }
    }

    /// The backend description for logs, e.g. `typesafe/jev-1.13.0`.
    #[must_use]
    pub fn label(&self) -> String {
        format!("{}/{}", self.config.provider, self.config.model)
    }

    /// The dialect's `api` spelling (result metadata).
    #[must_use]
    pub fn api(&self) -> &'static str {
        self.config.dialect.api()
    }

    /// The catalog provider reported on results.
    #[must_use]
    pub fn provider(&self) -> &str {
        &self.config.provider
    }

    /// Answer `request`. Splits over [`JevClientConfig::max_questions`],
    /// merging answers and summing usage across chunks.
    ///
    /// # Errors
    ///
    /// Every failure mode is [`JevError`]; see its variants for the retry,
    /// cooldown, and abort semantics.
    pub async fn judge(&self, request: &JudgmentRequest) -> Result<JudgmentResult, JevError> {
        request
            .validate()
            .map_err(|detail| JevError::Malformed { detail })?;
        let ids: Vec<String> = request.questions.keys().cloned().collect();
        if let Some(cap) = self.config.max_questions.filter(|cap| ids.len() > *cap) {
            return self.judge_chunked(request, ids, cap).await;
        }
        // The single-shot path shares the fail-channel plumbing so both
        // paths race the same cancellation shape.
        let (_fail_tx, fail_rx) = watch::channel(false);
        self.judge_once(&request.state, &request.questions, fail_rx)
            .await
    }

    /// Answer `request` in chunks of at most `cap` questions,
    /// [`CHUNK_PARALLEL`] at a time. The first failing chunk aborts its
    /// in-flight siblings and stops dispatch; answers merge and usage sums.
    async fn judge_chunked(
        &self,
        request: &JudgmentRequest,
        ids: Vec<String>,
        cap: usize,
    ) -> Result<JudgmentResult, JevError> {
        let chunks: Vec<Vec<String>> = ids.chunks(cap).map(<[String]>::to_vec).collect();
        let in_flight = CHUNK_PARALLEL.min(chunks.len());
        let (fail_tx, fail_rx) = watch::channel(false);
        let fail_tx = Arc::new(fail_tx);
        let first_failure: std::sync::Arc<std::sync::Mutex<Option<JevError>>> =
            std::sync::Arc::new(std::sync::Mutex::new(None));
        let mut results: Vec<Option<JudgmentResult>> = vec![None; chunks.len()];
        let mut outcomes = futures::stream::iter(chunks.into_iter().enumerate())
            .map(|(index, chunk)| {
                let fail_rx = fail_rx.clone();
                let first_failure = Arc::clone(&first_failure);
                let fail_tx = Arc::clone(&fail_tx);
                async move {
                    // The worker's dispatch check: a chunk pulled after a
                    // sibling failed never reaches the endpoint.
                    if *fail_rx.borrow() {
                        return (index, Err(JevError::Aborted));
                    }
                    let questions: BTreeMap<String, Question> = chunk
                        .iter()
                        .filter_map(|id| {
                            request
                                .questions
                                .get(id)
                                .map(|question| (id.clone(), question.clone()))
                        })
                        .collect();
                    let outcome = self
                        .judge_once(&request.state, &questions, fail_rx.clone())
                        .await;
                    if outcome.is_err() {
                        // First failure wins the report; later ones only
                        // reinforce the flag.
                        let mut slot = first_failure.lock().expect("jev chunk failure slot");
                        if slot.is_none() {
                            *slot = Some(outcome.as_ref().err().cloned().expect("checked is_err"));
                        }
                        let _ = fail_tx.send(true);
                    }
                    (index, outcome)
                }
            })
            .buffer_unordered(in_flight);
        while let Some((index, outcome)) = outcomes.next().await {
            if let Ok(result) = outcome {
                results[index] = Some(result);
            }
        }
        if let Some(failure) = first_failure.lock().expect("jev chunk failure slot").take() {
            let partial_usage = results
                .iter()
                .flatten()
                .fold(Usage::default(), |sum, result| sum.sum(result.usage));
            return Err(JevError::ChunkFailed {
                partial_cost_usd: partial_usage.cost_usd,
                source: Box::new(failure),
            });
        }
        let mut answers = BTreeMap::new();
        let mut usage = Usage::default();
        let mut model = self.config.model.clone();
        for result in results.into_iter().flatten() {
            if usage.input_tokens == 0 && usage.cost_usd == 0.0 {
                model = result.model;
            }
            answers.extend(result.answers);
            usage = usage.sum(result.usage);
        }
        Ok(JudgmentResult {
            api: self.config.dialect.api().to_string(),
            provider: self.config.provider.clone(),
            model,
            answers,
            usage,
        })
    }

    /// One request (one chunk) through the retry loop.
    async fn judge_once(
        &self,
        state: &JudgmentState,
        questions: &BTreeMap<String, Question>,
        mut fail_rx: watch::Receiver<bool>,
    ) -> Result<JudgmentResult, JevError> {
        let request = JudgmentRequest {
            state: state.clone(),
            questions: questions.clone(),
        };
        let body = wire::encode_request(&request, &self.config.model);
        let url = format!("{}{}", self.config.base_url, self.config.dialect.path());
        let mut attempt: u32 = 0;
        loop {
            if *fail_rx.borrow() {
                return Err(JevError::Aborted);
            }
            attempt += 1;
            let mut builder = self
                .http
                .post(&url)
                .timeout(self.config.timeout)
                .bearer_auth(&self.config.api_key)
                .header(reqwest::header::ACCEPT, "application/json");
            for (name, value) in &self.config.extra_headers {
                builder = builder.header(name.as_str(), value.as_str());
            }
            let send = builder.json(&body).send();
            let response = tokio::select! {
                biased;
                _ = fail_rx.changed() => return Err(JevError::Aborted),
                result = send => match result {
                    Ok(response) => response,
                    Err(error) => {
                        if *fail_rx.borrow() {
                            return Err(JevError::Aborted);
                        }
                        if attempt >= MAX_ATTEMPTS {
                            return Err(transport_error(&error, attempt));
                        }
                        self.sleep_backoff(attempt - 1, None, &mut fail_rx).await?;
                        continue;
                    }
                },
            };
            let status = response.status().as_u16();
            if response.status().is_success() {
                let payload = match response.json::<serde_json::Value>().await {
                    Ok(payload) => payload,
                    Err(error) => return Err(transport_error(&error, attempt)),
                };
                return self.decode_result(&payload, questions);
            }
            let retry_after = parse_retry_after_ms(response.headers());
            let body_text = response.text().await.unwrap_or_default();
            if !is_transient(status) || attempt >= MAX_ATTEMPTS {
                return Err(if is_transient(status) {
                    JevError::RetriesExhausted {
                        status,
                        attempts: attempt,
                        body: body_text,
                    }
                } else {
                    JevError::Api {
                        status,
                        body: body_text,
                    }
                });
            }
            self.sleep_backoff(attempt - 1, retry_after, &mut fail_rx)
                .await?;
        }
    }

    /// Decode a success payload and enforce the per-question postconditions.
    fn decode_result(
        &self,
        payload: &serde_json::Value,
        questions: &BTreeMap<String, Question>,
    ) -> Result<JudgmentResult, JevError> {
        let decoded = wire::decode_response(payload).ok_or_else(|| JevError::Malformed {
            detail: format!("not a judgment response: {payload}"),
        })?;
        for (id, question) in questions {
            if decoded.refused.iter().any(|refused| refused == id) {
                return Err(JevError::Malformed {
                    detail: format!("the endpoint refused to answer question {id:?}"),
                });
            }
            match decoded.answers.get(id) {
                None => {
                    return Err(JevError::Malformed {
                        detail: format!("no answer for question {id:?}"),
                    });
                }
                Some(answer) if answer.kind() != question.kind() => {
                    return Err(JevError::Malformed {
                        detail: format!(
                            "question {id:?} is a {} question but the answer is a {}",
                            question.kind().as_str(),
                            answer.kind().as_str()
                        ),
                    });
                }
                Some(_) => {}
            }
        }
        let mut answers = BTreeMap::new();
        for id in questions.keys() {
            answers.insert(
                id.clone(),
                decoded
                    .answers
                    .get(id)
                    .cloned()
                    .expect("postcondition checked above"),
            );
        }
        Ok(JudgmentResult {
            api: self.config.dialect.api().to_string(),
            provider: self.config.provider.clone(),
            model: decoded.model.unwrap_or_else(|| self.config.model.clone()),
            answers,
            usage: decoded.usage,
        })
    }

    /// Backoff before the next attempt: a server hint wins (capped at
    /// [`BACKOFF_MAX_MS`]), otherwise `BACKOFF_BASE_MS << attempt` capped
    /// the same way.
    async fn sleep_backoff(
        &self,
        attempt: u32,
        hint_ms: Option<u64>,
        fail_rx: &mut watch::Receiver<bool>,
    ) -> Result<(), JevError> {
        let delay_ms = match hint_ms {
            Some(hint) => hint.min(BACKOFF_MAX_MS),
            None => (BACKOFF_BASE_MS << attempt).min(BACKOFF_MAX_MS),
        };
        tokio::select! {
            biased;
            _ = fail_rx.changed() => Err(JevError::Aborted),
            () = tokio::time::sleep(Duration::from_millis(delay_ms)) => Ok(()),
        }
    }
}

/// Map a reqwest failure to the typed error (timeouts stay distinguishable:
/// the chain rethrows them without fallback).
fn transport_error(error: &reqwest::Error, attempts: u32) -> JevError {
    if error.is_timeout() {
        JevError::Timeout { attempts }
    } else {
        JevError::Connection {
            message: error.to_string(),
        }
    }
}

/// Whether an HTTP status is retryable (408, 429, and every 5xx - the
/// measured contract's transient class, 529 included).
fn is_transient(status: u16) -> bool {
    status == 408 || status == 429 || status >= 500
}

/// The server's retry hint in milliseconds: `retry-after-ms` (milliseconds)
/// or `retry-after` (seconds); `None` when absent or unparsable.
// The header parses as f64 then clamps before the u64 cast: truncation
// shaves a fraction of a millisecond off a sleep, never adds time.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn parse_retry_after_ms(headers: &reqwest::header::HeaderMap) -> Option<u64> {
    let direct = headers
        .get("retry-after-ms")
        .and_then(|value| value.to_str().ok())
        .and_then(|text| text.trim().parse::<f64>().ok())
        .map(|millis| millis.max(0.0) as u64);
    let seconds = headers
        .get("retry-after")
        .and_then(|value| value.to_str().ok())
        .and_then(|text| text.trim().parse::<f64>().ok())
        .map(|seconds| (seconds * 1000.0).max(0.0) as u64);
    direct.or(seconds)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transient_statuses_retry_and_others_do_not() {
        for status in [408u16, 429, 500, 502, 503, 504, 529] {
            assert!(is_transient(status), "{status} is transient");
        }
        for status in [200u16, 400, 401, 402, 403, 404, 413, 415, 422] {
            assert!(!is_transient(status), "{status} is not transient");
        }
    }

    #[test]
    fn retry_hints_parse_from_both_headers() {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert("retry-after", "1".parse().expect("header value"));
        assert_eq!(parse_retry_after_ms(&headers), Some(1000));
        headers.insert("retry-after-ms", "250".parse().expect("header value"));
        assert_eq!(parse_retry_after_ms(&headers), Some(250), "ms wins");
        headers.insert("retry-after", "garbage".parse().expect("header value"));
        assert_eq!(parse_retry_after_ms(&headers), Some(250));
        assert_eq!(
            parse_retry_after_ms(&reqwest::header::HeaderMap::new()),
            None
        );
    }
}
