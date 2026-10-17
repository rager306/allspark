//! One `pa-jev` client behind the answer cache, as a [`Judge`]: the native
//! judgment backend (fork port of the omp `nativeJudge` wrapper).
//!
//! Questions already answered about the same state under the same model come
//! from the cache; only the pending questions reach the endpoint (none when
//! every answer is cached - a full cache hit bills nothing and never reaches
//! the ledger). The cache model key is the catalog identity
//! (`provider/model-id`), so different candidates in one chain never share
//! answers.

use std::collections::BTreeMap;
use std::sync::Arc;

use pa_jev::{JevClient, JevError, JudgmentRequest, JudgmentResult, Usage};

use crate::jev::cache::{AnswerCache, CachedAnswers};
use crate::jev::{Judge, JudgeError};

/// The native judgment backend for one endpoint.
pub struct NativeJudge {
    client: JevClient,
    identity: String,
    cache: Option<Arc<AnswerCache>>,
}

impl NativeJudge {
    /// Build the backend over `client`; `identity` is the catalog identity
    /// (cooldown and cache key); `cache` absent runs uncached.
    #[must_use]
    pub fn new(client: JevClient, identity: String, cache: Option<Arc<AnswerCache>>) -> Self {
        NativeJudge {
            client,
            identity,
            cache,
        }
    }

    /// The client's error mapped into the chain's vocabulary.
    fn map_error(&self, error: JevError) -> JudgeError {
        match error {
            JevError::Aborted => JudgeError::Aborted,
            JevError::Timeout { attempts } => JudgeError::Timeout { attempts },
            other => JudgeError::Candidate {
                identity: self.identity.clone(),
                message: other.to_string(),
                status: other.status(),
            },
        }
    }
}

#[async_trait::async_trait]
impl Judge for NativeJudge {
    fn label(&self) -> &str {
        &self.identity
    }

    async fn judge(&self, request: &JudgmentRequest) -> Result<JudgmentResult, JudgeError> {
        let cached = self
            .cache
            .as_ref()
            .map(|cache| cache.lookup(&self.identity, request));
        let pending: BTreeMap<String, pa_jev::Question> = request
            .questions
            .iter()
            .filter(|(id, _)| {
                cached
                    .as_ref()
                    .is_none_or(|cached| !cached.answers.contains_key(*id))
            })
            .map(|(id, question)| (id.clone(), question.clone()))
            .collect();
        let Some(fresh) = (if pending.is_empty() {
            None
        } else {
            let pending_request = JudgmentRequest {
                state: request.state.clone(),
                questions: pending,
            };
            let result = self
                .client
                .judge(&pending_request)
                .await
                .map_err(|error| self.map_error(error))?;
            if let Some(cache) = &self.cache {
                cache.record(
                    &self.identity,
                    cached.as_ref().unwrap_or(&CachedAnswers::default()),
                    &pending_request.questions,
                    &result,
                );
            }
            Some(result)
        }) else {
            // A wholly-cached judgment: zero usage, every answer local.
            let answers = cached.map_or_else(BTreeMap::new, |cached| cached.answers);
            return Ok(JudgmentResult {
                api: self.client.api().to_string(),
                provider: self.client.provider().to_string(),
                model: self.identity.clone(),
                answers,
                usage: Usage::default(),
            });
        };
        let pa_jev::JudgmentResult {
            api,
            provider,
            model,
            answers: fresh_answers,
            usage: fresh_usage,
        } = fresh;
        let mut answers = cached.map_or_else(BTreeMap::new, |cached| cached.answers);
        answers.extend(fresh_answers);
        Ok(JudgmentResult {
            api,
            provider,
            model,
            answers,
            usage: fresh_usage,
        })
    }
}
