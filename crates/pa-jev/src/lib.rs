//! `pa-jev`: the typed-judgment (System One) leaf crate.
//!
//! Scope: the three question primitives' wire types (choice / score / noul,
//! field-for-field with the `{model, state, questions} -> {model, answers,
//! usage}` contract), the judgment wire dialects (`typesafe`,
//! `openrouter-decisions`), and one judgment client with bounded retry,
//! per-attempt timeout, and question chunking. FORK-ONLY: not in upstream.
//!
//! Non-goals: candidate chains and cooldowns, the answer cache, the agent
//! tool, settings, and every consumer - those live in `pa-core::jev` and
//! above. No provider-abstraction, model-registry, or session knowledge
//! lives here; a candidate is a URL, a key, and a wire dialect.
//!
//! Public API: [`types`] (the wire vocabulary), [`wire`] (dialects and the
//! shared codec), [`client`] ([`JevClient`]), and [`error`] ([`JevError`]).
//!
//! Cancellation is drop-based: dropping the `judge` future cancels the
//! in-flight request. Callers that need a typed abort race the future
//! against their own signal.

pub mod client;
pub mod error;
pub mod types;
pub mod wire;

pub use client::{
    JevClient, JevClientConfig, JudgmentResult, BACKOFF_BASE_MS, BACKOFF_MAX_MS, CHUNK_PARALLEL,
    DEFAULT_TIMEOUT_MS, MAX_ATTEMPTS,
};
pub use error::JevError;
pub use types::{
    Answer, DecodedJudgment, JudgmentRequest, JudgmentState, NoulCriteria, Question, QuestionKind,
    Usage, JEV_INPUT_PRICE_PER_MTOK, MAX_CHOICE_OPTIONS, MAX_SCORE_LEVELS, MIN_SCORE_LEVELS,
};
pub use wire::{is_judgment_api, WireDialect};
