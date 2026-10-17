//! Fork-only built-in provider catalog.
// FORK-ONLY: this file does not exist upstream. See docs/fork-identity.md.
//!
//! The upstream catalog chain (remote fetch -> disk cache -> bundled snapshot ->
//! [`crate::models_generated`]) never carries this fork's providers, so their
//! entries live here and `pa-core`'s registry merges them into the built-in set
//! after the chain resolves. [`crate::models_generated`] stays a byte-faithful
//! mirror of the TS fallback file; fork additions never touch it.
//!
//! Providers:
//! - `muse` — Meta Model API (`api.meta.ai`). Muse Code subscription keys are
//!   minted per account (device flow at `auth.meta.com`, `POST
//!   api.meta.ai/muse-code/key`); pay-as-you-go keys come from `dev.meta.ai`.
//!   Muse Spark reasoning (with encrypted-content replay) only round-trips on
//!   the Responses wire format, so the models pin `openai-responses`.
//! - `codebuddy` — the `codebuddy.ai` IDE-plan chat gateway
//!   (`www.codebuddy.ai/v2/chat/completions`). OpenAI-shaped, stream-only,
//!   requires its IDE headers and a leading system message; thinking goes
//!   through `reasoning_effort` ("none" accepted, verified live).
//! - `genspark` — the `genspark.ai` LLM proxy
//!   (`www.genspark.ai/api/llm_proxy/v1`, `LiteLLM` backend), Bearer `gsk_...`
//!   keys from the browser device flow. Server-driven model list; the entries
//!   are a curated snapshot with credit prices converted at the Plus rate.

use std::sync::LazyLock;

use crate::types::{
    CompatKind, JsNumber, Model, ModelCompat, ModelCost, ModelInput, ModelThinkingLevel,
    OpenAiCompletionsCompat, ThinkingLevelMap,
};

/// One fork catalog entry (the [`crate::models_generated`] layout, minus the
/// fields no fork provider uses).
struct ForkModel {
    provider: &'static str,
    id: &'static str,
    name: &'static str,
    api: &'static str,
    base_url: &'static str,
    reasoning: bool,
    thinking_levels: &'static [(ModelThinkingLevel, Option<&'static str>)],
    input: &'static [ModelInput],
    /// (`input`, `output`, `cacheRead`, `cacheWrite`) in $/million tokens.
    cost: (f64, f64, f64, f64),
    context_window: u64,
    max_tokens: u64,
    headers: &'static [(&'static str, &'static str)],
    /// The OpenAI-completions compat overrides for this entry, `None` for the
    /// detected defaults.
    supports_developer_role: Option<bool>,
    /// `Some(MaxTokens)` when the gateway only reads the legacy `max_tokens`
    /// field and ignores `max_completion_tokens` (the `CodeBuddy` gateways
    /// silently drop the completion-tokens form).
    max_tokens_field: Option<crate::types::MaxTokensField>,
}

/// The IDE headers the `CodeBuddy` gateway requires on every chat request.
const CODEBUDDY_HEADERS: &[(&str, &str)] = &[
    ("User-Agent", "IDE/2.108.1 CodeBuddy/2.108.1"),
    ("X-Product", "SaaS"),
    ("X-IDE-Type", "IDE"),
    ("X-IDE-Name", "IDE"),
    ("x-requested-with", "XMLHttpRequest"),
    ("x-codebuddy-request", "1"),
];

/// Genspark effort variants: `low`..=`xhigh`, no `off`/`minimal`/`max`.
const EFFORT_NO_OFF: &[(ModelThinkingLevel, Option<&str>)] = &[
    (ModelThinkingLevel::Off, None),
    (ModelThinkingLevel::Minimal, None),
    (ModelThinkingLevel::Low, Some("low")),
    (ModelThinkingLevel::Medium, Some("medium")),
    (ModelThinkingLevel::High, Some("high")),
    (ModelThinkingLevel::Xhigh, Some("xhigh")),
    (ModelThinkingLevel::Max, None),
];

/// Genspark effort variants that accept `none` (the `OpenAI` off value) and
/// top out at `xhigh`.
const EFFORT_WITH_NONE: &[(ModelThinkingLevel, Option<&str>)] = &[
    (ModelThinkingLevel::Off, Some("none")),
    (ModelThinkingLevel::Minimal, None),
    (ModelThinkingLevel::Low, Some("low")),
    (ModelThinkingLevel::Medium, Some("medium")),
    (ModelThinkingLevel::High, Some("high")),
    (ModelThinkingLevel::Xhigh, Some("xhigh")),
    (ModelThinkingLevel::Max, None),
];

/// Genspark effort variants `low`..=`max`, no `off`/`minimal`/`xhigh`.
const EFFORT_NO_OFF_WITH_MAX: &[(ModelThinkingLevel, Option<&str>)] = &[
    (ModelThinkingLevel::Off, None),
    (ModelThinkingLevel::Minimal, None),
    (ModelThinkingLevel::Low, Some("low")),
    (ModelThinkingLevel::Medium, Some("medium")),
    (ModelThinkingLevel::High, Some("high")),
    (ModelThinkingLevel::Xhigh, None),
    (ModelThinkingLevel::Max, Some("max")),
];

/// Genspark effort variants `low`..=`high` only.
const EFFORT_LOW_MEDIUM_HIGH: &[(ModelThinkingLevel, Option<&str>)] = &[
    (ModelThinkingLevel::Off, None),
    (ModelThinkingLevel::Minimal, None),
    (ModelThinkingLevel::Low, Some("low")),
    (ModelThinkingLevel::Medium, Some("medium")),
    (ModelThinkingLevel::High, Some("high")),
    (ModelThinkingLevel::Xhigh, None),
    (ModelThinkingLevel::Max, None),
];

/// Muse Spark thinking is always on: `off` is unsupported (`none` effort
/// returns 400). The range is `minimal`..=`max` (verified live against
/// `api.meta.ai`).
const MUSE_THINKING_LEVELS: &[(ModelThinkingLevel, Option<&str>)] = &[
    (ModelThinkingLevel::Off, None),
    (ModelThinkingLevel::Minimal, Some("minimal")),
    (ModelThinkingLevel::Low, Some("low")),
    (ModelThinkingLevel::Medium, Some("medium")),
    (ModelThinkingLevel::High, Some("high")),
    (ModelThinkingLevel::Xhigh, Some("xhigh")),
    (ModelThinkingLevel::Max, Some("max")),
];

static FORK_MODELS: LazyLock<Vec<Model>> = LazyLock::new(|| {
    FORK_RAW_MODELS
        .iter()
        .map(|raw| {
            let thinking_level_map: Option<ThinkingLevelMap> = (!raw.thinking_levels.is_empty())
                .then(|| {
                    raw.thinking_levels
                        .iter()
                        .copied()
                        .map(|(level, value)| (level, value.map(String::from)))
                        .collect()
                });
            let headers = (!raw.headers.is_empty()).then(|| {
                raw.headers
                    .iter()
                    .copied()
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    .collect()
            });
            Model {
                compat: raw.supports_developer_role.map(|flag| {
                    ModelCompat::from_kind(CompatKind::OpenAiCompletions(Box::new(
                        OpenAiCompletionsCompat {
                            supports_developer_role: Some(flag),
                            max_tokens_field: raw.max_tokens_field,
                            ..OpenAiCompletionsCompat::default()
                        },
                    )))
                }),
                id: raw.id.to_string(),
                name: raw.name.to_string(),
                api: raw.api.to_string(),
                provider: raw.provider.to_string(),
                base_url: raw.base_url.to_string(),
                reasoning: raw.reasoning,
                thinking_level_map,
                input: raw.input.to_vec(),
                cost: ModelCost {
                    input: JsNumber::from(raw.cost.0),
                    output: JsNumber::from(raw.cost.1),
                    cache_read: JsNumber::from(raw.cost.2),
                    cache_write: JsNumber::from(raw.cost.3),
                },
                context_window: raw.context_window,
                max_tokens: raw.max_tokens,
                max_tokens_explicit: false,
                featured: None,
                headers,
            }
        })
        .collect()
});

static FORK_RAW_MODELS: &[ForkModel] = &[
    // --- Subquadratic (SubQ Snap) ----------------------------------------------
    // A decision model speaking the TypeSafe System One wire byte-for-byte
    // (`POST {base}/v1/systemone`, same {model, state, questions} request and
    // {model, answers, usage} response, verified live 2026-10-17): it joins
    // the judge chain through the existing `typesafe` dialect with no Rust of
    // its own. Judgments only - the chat endpoints reject this model - so it
    // is never a session model. Caps 16 questions per request (17 answers
    // 422; batches above that must chunk, `pa-jev` does when configured);
    // output is not billed. `SUBQ_API_KEY` authenticates (Bearer).
    ForkModel {
        provider: "subquadratic",
        id: "subq-snap-preview",
        name: "SubQ Snap (preview)",
        api: "typesafe",
        base_url: "https://api.subq.ai",
        reasoning: false,
        thinking_levels: &[],
        input: &[ModelInput::Text],
        cost: (0.05, 0.0, 0.0, 0.0),
        context_window: 64_000,
        max_tokens: 4_096,
        headers: &[],
        supports_developer_role: None,
        max_tokens_field: None,
    },
    // --- Muse (Meta Model API) -------------------------------------------------
    // PAYG Standard pricing; context/output per the Meta guide.
    ForkModel {
        provider: "muse",
        id: "muse-spark-1.3",
        name: "Muse Spark 1.3",
        api: "openai-responses",
        base_url: "https://api.meta.ai/v1",
        reasoning: true,
        thinking_levels: MUSE_THINKING_LEVELS,
        input: &[ModelInput::Text, ModelInput::Image],
        cost: (1.25, 4.25, 0.15, 0.0),
        context_window: 1_048_576,
        max_tokens: 131_072,
        headers: &[("x-api-version", "1.0.0")],
        supports_developer_role: None,
        max_tokens_field: None,
    },
    ForkModel {
        provider: "muse",
        id: "muse-spark-1.3-contributor",
        name: "Muse Spark 1.3 Contributor",
        api: "openai-responses",
        base_url: "https://api.meta.ai/v1",
        reasoning: true,
        thinking_levels: MUSE_THINKING_LEVELS,
        input: &[ModelInput::Text, ModelInput::Image],
        cost: (0.10, 0.20, 0.0, 0.0),
        context_window: 1_048_576,
        max_tokens: 131_072,
        headers: &[("x-api-version", "1.0.0")],
        supports_developer_role: None,
        max_tokens_field: None,
    },
    ForkModel {
        provider: "muse",
        id: "muse-spark-1.2",
        name: "Muse Spark 1.2",
        api: "openai-responses",
        base_url: "https://api.meta.ai/v1",
        reasoning: true,
        thinking_levels: MUSE_THINKING_LEVELS,
        input: &[ModelInput::Text, ModelInput::Image],
        cost: (1.25, 4.25, 0.15, 0.0),
        context_window: 1_048_576,
        max_tokens: 131_072,
        headers: &[("x-api-version", "1.0.0")],
        supports_developer_role: None,
        max_tokens_field: None,
    },
    ForkModel {
        provider: "muse",
        id: "muse-spark-1.2-contributor",
        name: "Muse Spark 1.2 Contributor",
        api: "openai-responses",
        base_url: "https://api.meta.ai/v1",
        reasoning: true,
        thinking_levels: MUSE_THINKING_LEVELS,
        input: &[ModelInput::Text, ModelInput::Image],
        cost: (0.10, 0.20, 0.0, 0.0),
        context_window: 1_048_576,
        max_tokens: 131_072,
        headers: &[("x-api-version", "1.0.0")],
        supports_developer_role: None,
        max_tokens_field: None,
    },
    ForkModel {
        provider: "muse",
        id: "muse-spark-1.1",
        name: "Muse Spark 1.1",
        api: "openai-responses",
        base_url: "https://api.meta.ai/v1",
        reasoning: true,
        thinking_levels: MUSE_THINKING_LEVELS,
        input: &[ModelInput::Text, ModelInput::Image],
        cost: (1.25, 4.25, 0.15, 0.0),
        context_window: 1_048_576,
        max_tokens: 131_072,
        headers: &[("x-api-version", "1.0.0")],
        supports_developer_role: None,
        max_tokens_field: None,
    },
    // --- CodeBuddy (codebuddy.ai IDE plan) -------------------------------------
    // Subscription credits: no per-token list price, so cost stays zero.
    // Context/output windows mirror the gateway's model metadata.
    ForkModel {
        provider: "codebuddy",
        id: "glm-5.2",
        name: "GLM-5.2",
        api: "openai-completions",
        base_url: "https://www.codebuddy.ai/v2",
        reasoning: true,
        thinking_levels: &[],
        input: &[ModelInput::Text],
        cost: (0.0, 0.0, 0.0, 0.0),
        context_window: 1_000_000,
        max_tokens: 131_072,
        headers: CODEBUDDY_HEADERS,
        supports_developer_role: Some(false),
        max_tokens_field: Some(crate::types::MaxTokensField::MaxTokens),
    },
    ForkModel {
        provider: "codebuddy",
        id: "glm-5.1",
        name: "GLM-5.1",
        api: "openai-completions",
        base_url: "https://www.codebuddy.ai/v2",
        reasoning: true,
        thinking_levels: &[],
        input: &[ModelInput::Text],
        cost: (0.0, 0.0, 0.0, 0.0),
        context_window: 200_000,
        max_tokens: 128_000,
        headers: CODEBUDDY_HEADERS,
        supports_developer_role: Some(false),
        max_tokens_field: Some(crate::types::MaxTokensField::MaxTokens),
    },
    ForkModel {
        provider: "codebuddy",
        id: "glm-5.0",
        name: "GLM-5.0",
        api: "openai-completions",
        base_url: "https://www.codebuddy.ai/v2",
        reasoning: true,
        thinking_levels: &[],
        input: &[ModelInput::Text],
        cost: (0.0, 0.0, 0.0, 0.0),
        context_window: 200_000,
        max_tokens: 128_000,
        headers: CODEBUDDY_HEADERS,
        supports_developer_role: Some(false),
        max_tokens_field: Some(crate::types::MaxTokensField::MaxTokens),
    },
    ForkModel {
        provider: "codebuddy",
        id: "glm-5.0-turbo",
        name: "GLM-5.0-Turbo",
        api: "openai-completions",
        base_url: "https://www.codebuddy.ai/v2",
        reasoning: true,
        thinking_levels: &[],
        input: &[ModelInput::Text],
        cost: (0.0, 0.0, 0.0, 0.0),
        context_window: 200_000,
        max_tokens: 128_000,
        headers: CODEBUDDY_HEADERS,
        supports_developer_role: Some(false),
        max_tokens_field: Some(crate::types::MaxTokensField::MaxTokens),
    },
    ForkModel {
        provider: "codebuddy",
        id: "glm-5v-turbo",
        name: "GLM-5v-Turbo",
        api: "openai-completions",
        base_url: "https://www.codebuddy.ai/v2",
        reasoning: true,
        thinking_levels: &[],
        input: &[ModelInput::Text, ModelInput::Image],
        cost: (0.0, 0.0, 0.0, 0.0),
        context_window: 200_000,
        max_tokens: 128_000,
        headers: CODEBUDDY_HEADERS,
        supports_developer_role: Some(false),
        max_tokens_field: Some(crate::types::MaxTokensField::MaxTokens),
    },
    ForkModel {
        provider: "codebuddy",
        id: "glm-4.7",
        name: "GLM-4.7",
        api: "openai-completions",
        base_url: "https://www.codebuddy.ai/v2",
        reasoning: true,
        thinking_levels: &[],
        input: &[ModelInput::Text],
        cost: (0.0, 0.0, 0.0, 0.0),
        context_window: 200_000,
        max_tokens: 128_000,
        headers: CODEBUDDY_HEADERS,
        supports_developer_role: Some(false),
        max_tokens_field: Some(crate::types::MaxTokensField::MaxTokens),
    },
    ForkModel {
        provider: "codebuddy",
        id: "minimax-m3",
        name: "MiniMax-M3",
        api: "openai-completions",
        base_url: "https://www.codebuddy.ai/v2",
        reasoning: true,
        thinking_levels: &[],
        input: &[ModelInput::Text, ModelInput::Image],
        cost: (0.0, 0.0, 0.0, 0.0),
        context_window: 1_000_000,
        max_tokens: 131_072,
        headers: CODEBUDDY_HEADERS,
        supports_developer_role: Some(false),
        max_tokens_field: Some(crate::types::MaxTokensField::MaxTokens),
    },
    ForkModel {
        provider: "codebuddy",
        id: "minimax-m2.7",
        name: "MiniMax-M2.7",
        api: "openai-completions",
        base_url: "https://www.codebuddy.ai/v2",
        reasoning: true,
        thinking_levels: &[],
        input: &[ModelInput::Text, ModelInput::Image],
        cost: (0.0, 0.0, 0.0, 0.0),
        context_window: 204_800,
        max_tokens: 131_072,
        headers: CODEBUDDY_HEADERS,
        supports_developer_role: Some(false),
        max_tokens_field: Some(crate::types::MaxTokensField::MaxTokens),
    },
    ForkModel {
        provider: "codebuddy",
        id: "kimi-k2.7",
        name: "Kimi-K2.7-Code",
        api: "openai-completions",
        base_url: "https://www.codebuddy.ai/v2",
        reasoning: true,
        thinking_levels: &[],
        input: &[ModelInput::Text, ModelInput::Image],
        cost: (0.0, 0.0, 0.0, 0.0),
        context_window: 262_144,
        max_tokens: 262_144,
        headers: CODEBUDDY_HEADERS,
        supports_developer_role: Some(false),
        max_tokens_field: Some(crate::types::MaxTokensField::MaxTokens),
    },
    ForkModel {
        provider: "codebuddy",
        id: "kimi-k2.6",
        name: "Kimi-K2.6",
        api: "openai-completions",
        base_url: "https://www.codebuddy.ai/v2",
        reasoning: true,
        thinking_levels: &[],
        input: &[ModelInput::Text, ModelInput::Image],
        cost: (0.0, 0.0, 0.0, 0.0),
        context_window: 262_144,
        max_tokens: 262_144,
        headers: CODEBUDDY_HEADERS,
        supports_developer_role: Some(false),
        max_tokens_field: Some(crate::types::MaxTokensField::MaxTokens),
    },
    ForkModel {
        provider: "codebuddy",
        id: "kimi-k2.5",
        name: "Kimi-K2.5",
        api: "openai-completions",
        base_url: "https://www.codebuddy.ai/v2",
        reasoning: true,
        thinking_levels: &[],
        input: &[ModelInput::Text, ModelInput::Image],
        cost: (0.0, 0.0, 0.0, 0.0),
        context_window: 262_144,
        max_tokens: 262_144,
        headers: CODEBUDDY_HEADERS,
        supports_developer_role: Some(false),
        max_tokens_field: Some(crate::types::MaxTokensField::MaxTokens),
    },
    ForkModel {
        provider: "codebuddy",
        id: "hy3-preview",
        name: "Hy3 Preview",
        api: "openai-completions",
        base_url: "https://www.codebuddy.ai/v2",
        reasoning: true,
        thinking_levels: &[],
        input: &[ModelInput::Text],
        cost: (0.0, 0.0, 0.0, 0.0),
        context_window: 262_144,
        max_tokens: 262_144,
        headers: CODEBUDDY_HEADERS,
        supports_developer_role: Some(false),
        max_tokens_field: Some(crate::types::MaxTokensField::MaxTokens),
    },
    ForkModel {
        provider: "codebuddy",
        id: "deepseek-v4-pro",
        name: "DeepSeek-V4-Pro",
        api: "openai-completions",
        base_url: "https://www.codebuddy.ai/v2",
        reasoning: true,
        thinking_levels: &[],
        input: &[ModelInput::Text],
        cost: (0.0, 0.0, 0.0, 0.0),
        context_window: 1_000_000,
        max_tokens: 384_000,
        headers: CODEBUDDY_HEADERS,
        supports_developer_role: Some(false),
        max_tokens_field: Some(crate::types::MaxTokensField::MaxTokens),
    },
    ForkModel {
        provider: "codebuddy",
        id: "deepseek-v4.1-flash",
        name: "DeepSeek-V4.1-Flash",
        api: "openai-completions",
        base_url: "https://www.codebuddy.ai/v2",
        reasoning: true,
        thinking_levels: &[],
        input: &[ModelInput::Text, ModelInput::Image],
        cost: (0.0, 0.0, 0.0, 0.0),
        context_window: 1_000_000,
        max_tokens: 384_000,
        headers: CODEBUDDY_HEADERS,
        supports_developer_role: Some(false),
        max_tokens_field: Some(crate::types::MaxTokensField::MaxTokens),
    },
    ForkModel {
        provider: "codebuddy",
        id: "deepseek-v3-2-volc",
        name: "DeepSeek-V3.2",
        api: "openai-completions",
        base_url: "https://www.codebuddy.ai/v2",
        reasoning: true,
        thinking_levels: &[],
        input: &[ModelInput::Text],
        cost: (0.0, 0.0, 0.0, 0.0),
        context_window: 128_000,
        max_tokens: 64_000,
        headers: CODEBUDDY_HEADERS,
        supports_developer_role: Some(false),
        max_tokens_field: Some(crate::types::MaxTokensField::MaxTokens),
    },
    // --- Genspark (genspark.ai agents/GenCode, LiteLLM proxy) ------------------
    // Credits-priced: costs are credit prices converted at the Plus rate
    // ($24.99 / 10,000 credits = $0.0025/credit). The model list is a
    // server-driven snapshot (2026-10-11); thinking maps mirror the proxy's
    // accepted reasoning_effort variants per model.
    ForkModel {
        provider: "genspark",
        id: "claude-fable-5",
        name: "Claude Fable 5 (Genspark)",
        api: "openai-completions",
        base_url: "https://www.genspark.ai/api/llm_proxy/v1",
        reasoning: true,
        thinking_levels: EFFORT_NO_OFF,
        input: &[ModelInput::Text, ModelInput::Image],
        cost: (13.75, 68.75, 0.0, 0.0),
        context_window: 1_000_000,
        max_tokens: 128_000,
        headers: &[],
        supports_developer_role: None,
        max_tokens_field: None,
    },
    ForkModel {
        provider: "genspark",
        id: "claude-opus-5",
        name: "Claude Opus 5 (Genspark)",
        api: "openai-completions",
        base_url: "https://www.genspark.ai/api/llm_proxy/v1",
        reasoning: true,
        thinking_levels: EFFORT_NO_OFF,
        input: &[ModelInput::Text, ModelInput::Image],
        cost: (6.875, 34.375, 0.6875, 8.59375),
        context_window: 1_000_000,
        max_tokens: 128_000,
        headers: &[],
        supports_developer_role: None,
        max_tokens_field: None,
    },
    ForkModel {
        provider: "genspark",
        id: "claude-sonnet-5-5",
        name: "Claude Sonnet 5.5 (Genspark)",
        api: "openai-completions",
        base_url: "https://www.genspark.ai/api/llm_proxy/v1",
        reasoning: true,
        thinking_levels: EFFORT_NO_OFF,
        input: &[ModelInput::Text, ModelInput::Image],
        cost: (2.75, 13.75, 0.0, 0.0),
        context_window: 1_000_000,
        max_tokens: 128_000,
        headers: &[],
        supports_developer_role: None,
        max_tokens_field: None,
    },
    ForkModel {
        provider: "genspark",
        id: "claude-haiku-5-5",
        name: "Claude Haiku 5.5 (Genspark)",
        api: "openai-completions",
        base_url: "https://www.genspark.ai/api/llm_proxy/v1",
        reasoning: true,
        thinking_levels: EFFORT_LOW_MEDIUM_HIGH,
        input: &[ModelInput::Text, ModelInput::Image],
        cost: (0.1375, 0.6875, 0.0, 0.0),
        context_window: 1_000_000,
        max_tokens: 128_000,
        headers: &[],
        supports_developer_role: None,
        max_tokens_field: None,
    },
    ForkModel {
        provider: "genspark",
        id: "gpt-6-astra",
        name: "GPT-6 Astra (Genspark)",
        api: "openai-completions",
        base_url: "https://www.genspark.ai/api/llm_proxy/v1",
        reasoning: true,
        thinking_levels: EFFORT_WITH_NONE,
        input: &[ModelInput::Text, ModelInput::Image],
        cost: (13.75, 68.75, 0.0, 0.0),
        context_window: 1_000_000,
        max_tokens: 128_000,
        headers: &[],
        supports_developer_role: None,
        max_tokens_field: None,
    },
    ForkModel {
        provider: "genspark",
        id: "gpt-6-sol",
        name: "GPT-6 Sol (Genspark)",
        api: "openai-completions",
        base_url: "https://www.genspark.ai/api/llm_proxy/v1",
        reasoning: true,
        thinking_levels: EFFORT_WITH_NONE,
        input: &[ModelInput::Text, ModelInput::Image],
        cost: (2.75, 13.75, 0.0, 0.0),
        context_window: 1_000_000,
        max_tokens: 128_000,
        headers: &[],
        supports_developer_role: None,
        max_tokens_field: None,
    },
    ForkModel {
        provider: "genspark",
        id: "gpt-6-luna",
        name: "GPT-6 Luna (Genspark)",
        api: "openai-completions",
        base_url: "https://www.genspark.ai/api/llm_proxy/v1",
        reasoning: true,
        thinking_levels: EFFORT_WITH_NONE,
        input: &[ModelInput::Text, ModelInput::Image],
        cost: (0.1375, 0.6875, 0.0, 0.0),
        context_window: 1_000_000,
        max_tokens: 128_000,
        headers: &[],
        supports_developer_role: None,
        max_tokens_field: None,
    },
    ForkModel {
        provider: "genspark",
        id: "gpt-5.6-sol",
        name: "GPT-5.6 Sol (Genspark)",
        api: "openai-completions",
        base_url: "https://www.genspark.ai/api/llm_proxy/v1",
        reasoning: true,
        thinking_levels: EFFORT_WITH_NONE,
        input: &[ModelInput::Text, ModelInput::Image],
        cost: (5.5, 27.5, 0.0, 0.0),
        context_window: 1_000_000,
        max_tokens: 128_000,
        headers: &[],
        supports_developer_role: None,
        max_tokens_field: None,
    },
    ForkModel {
        provider: "genspark",
        id: "gpt-5.6-terra",
        name: "GPT-5.6 Terra (Genspark)",
        api: "openai-completions",
        base_url: "https://www.genspark.ai/api/llm_proxy/v1",
        reasoning: true,
        thinking_levels: EFFORT_WITH_NONE,
        input: &[ModelInput::Text, ModelInput::Image],
        cost: (2.75, 16.5, 0.0, 0.0),
        context_window: 1_000_000,
        max_tokens: 128_000,
        headers: &[],
        supports_developer_role: None,
        max_tokens_field: None,
    },
    ForkModel {
        provider: "genspark",
        id: "deepseek-v4-pro-0813",
        name: "DeepSeek V4 Pro (Genspark)",
        api: "openai-completions",
        base_url: "https://www.genspark.ai/api/llm_proxy/v1",
        reasoning: true,
        thinking_levels: EFFORT_WITH_NONE,
        input: &[ModelInput::Text],
        cost: (1.815, 5.445, 0.0, 0.0),
        context_window: 1_000_000,
        max_tokens: 128_000,
        headers: &[],
        supports_developer_role: None,
        max_tokens_field: None,
    },
    ForkModel {
        provider: "genspark",
        id: "deep-seek-v4.1-flash",
        name: "DeepSeek V4.1 Flash (Genspark)",
        api: "openai-completions",
        base_url: "https://www.genspark.ai/api/llm_proxy/v1",
        reasoning: true,
        thinking_levels: EFFORT_WITH_NONE,
        input: &[ModelInput::Text, ModelInput::Image],
        cost: (0.4125, 1.65, 0.0, 0.0),
        context_window: 1_000_000,
        max_tokens: 128_000,
        headers: &[],
        supports_developer_role: None,
        max_tokens_field: None,
    },
    ForkModel {
        provider: "genspark",
        id: "glm-5p3",
        name: "GLM-5.3 (Genspark)",
        api: "openai-completions",
        base_url: "https://www.genspark.ai/api/llm_proxy/v1",
        reasoning: true,
        thinking_levels: EFFORT_NO_OFF_WITH_MAX,
        input: &[ModelInput::Text],
        cost: (1.925, 6.05, 0.0, 0.0),
        context_window: 1_000_000,
        max_tokens: 131_072,
        headers: &[],
        supports_developer_role: None,
        max_tokens_field: None,
    },
    ForkModel {
        provider: "genspark",
        id: "kimi-k3",
        name: "Kimi K3 (Genspark)",
        api: "openai-completions",
        base_url: "https://www.genspark.ai/api/llm_proxy/v1",
        reasoning: true,
        thinking_levels: EFFORT_NO_OFF_WITH_MAX,
        input: &[ModelInput::Text, ModelInput::Image],
        cost: (4.125, 20.625, 0.0, 0.0),
        context_window: 1_048_576,
        max_tokens: 32_768,
        headers: &[],
        supports_developer_role: None,
        max_tokens_field: None,
    },
    ForkModel {
        provider: "genspark",
        id: "grok-4.5",
        name: "Grok 4.5 (Genspark)",
        api: "openai-completions",
        base_url: "https://www.genspark.ai/api/llm_proxy/v1",
        reasoning: true,
        thinking_levels: EFFORT_LOW_MEDIUM_HIGH,
        input: &[ModelInput::Text, ModelInput::Image],
        cost: (2.75, 8.25, 0.0, 0.0),
        context_window: 200_000,
        max_tokens: 32_768,
        headers: &[],
        supports_developer_role: None,
        max_tokens_field: None,
    },
    ForkModel {
        provider: "genspark",
        id: "minimax-m3",
        name: "MiniMax M3 (Genspark)",
        api: "openai-completions",
        base_url: "https://www.genspark.ai/api/llm_proxy/v1",
        reasoning: true,
        thinking_levels: EFFORT_WITH_NONE,
        input: &[ModelInput::Text, ModelInput::Image],
        cost: (0.4125, 1.65, 0.0, 0.0),
        context_window: 524_288,
        max_tokens: 32_768,
        headers: &[],
        supports_developer_role: None,
        max_tokens_field: None,
    },
    ForkModel {
        provider: "genspark",
        id: "mimo-v2.6-flash",
        name: "MiMo V2.6 Flash (Genspark)",
        api: "openai-completions",
        base_url: "https://www.genspark.ai/api/llm_proxy/v1",
        reasoning: false,
        thinking_levels: &[],
        input: &[ModelInput::Text, ModelInput::Image],
        cost: (0.1925, 0.385, 0.0, 0.0),
        context_window: 1_048_576,
        max_tokens: 65_536,
        headers: &[],
        supports_developer_role: None,
        max_tokens_field: None,
    },
];

/// The fork's built-in providers, merged into the registry's built-in set.
#[must_use]
pub fn fork_provider_models() -> Vec<Model> {
    FORK_MODELS.clone()
}

/// The fork's provider ids (the display-name and env-key maps stay in sync
/// with this list).
#[must_use]
pub fn fork_providers() -> &'static [&'static str] {
    &["muse", "codebuddy", "genspark", "subquadratic"]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::ModelExt;

    #[test]
    fn entries_are_unique_per_provider() {
        let models = fork_provider_models();
        let mut seen = std::collections::HashSet::new();
        for model in &models {
            assert!(
                seen.insert((model.provider.as_str(), model.id.as_str())),
                "duplicate fork entry {}/{}",
                model.provider,
                model.id
            );
        }
    }

    #[test]
    fn subquadratic_snap_joins_the_judge_chain_through_the_typesafe_dialect() {
        let models = fork_provider_models();
        let snap = models
            .iter()
            .find(|model| model.provider == "subquadratic")
            .expect("the subquadratic catalog entry exists");
        assert_eq!(snap.id, "subq-snap-preview");
        // The wire dialect is what resolves the judge candidate; a decision
        // model must never resolve as a chat backend.
        assert_eq!(snap.api, "typesafe");
        assert_eq!(snap.base_url, "https://api.subq.ai");
        // Input-only billing: the output side of the tuple stays zero so
        // usage accounting never invents output cost.
        assert_eq!(snap.cost.input, 0.05.into());
        assert_eq!(snap.cost.output, 0.0.into());
    }

    #[test]
    fn muse_pins_the_responses_wire_and_the_api_version_header() {
        for model in fork_provider_models()
            .iter()
            .filter(|m| m.provider == "muse")
        {
            assert_eq!(model.api, "openai-responses", "{}", model.id);
            assert_eq!(model.base_url, "https://api.meta.ai/v1");
            let headers = model.headers.as_ref().expect("muse carries headers");
            assert_eq!(headers.get("x-api-version"), Some(&"1.0.0".to_string()));
            let map = model
                .thinking_level_map
                .as_ref()
                .expect("muse thinking map");
            assert_eq!(map.get(&ModelThinkingLevel::Off), Some(&None));
            assert_eq!(
                map.get(&ModelThinkingLevel::Max),
                Some(&Some("max".to_string()))
            );
            assert_eq!(
                map.get(&ModelThinkingLevel::Minimal),
                Some(&Some("minimal".to_string()))
            );
        }
    }

    #[test]
    fn genspark_models_pin_the_llm_proxy() {
        let models: Vec<_> = fork_provider_models()
            .into_iter()
            .filter(|m| m.provider == "genspark")
            .collect();
        assert_eq!(models.len(), 16, "the curated genspark snapshot");
        for model in &models {
            assert_eq!(model.api, "openai-completions", "{}", model.id);
            assert_eq!(model.base_url, "https://www.genspark.ai/api/llm_proxy/v1");
        }
    }

    #[test]
    fn codebuddy_models_carry_the_gateway_headers() {
        let models: Vec<_> = fork_provider_models()
            .into_iter()
            .filter(|m| m.provider == "codebuddy")
            .collect();
        assert_eq!(models.len(), 15, "the codebuddy.ai IDE lineup");
        for model in &models {
            assert_eq!(model.api, "openai-completions", "{}", model.id);
            assert_eq!(model.base_url, "https://www.codebuddy.ai/v2");
            let headers = model.headers.as_ref().expect("codebuddy carries headers");
            for (name, _) in CODEBUDDY_HEADERS {
                assert!(
                    headers.contains_key(*name),
                    "{} misses header {name}",
                    model.id
                );
            }
            // The gateway's security policy rejects `developer`-role messages
            // outright ("Illegal API invocation from an unapproved channel"),
            // so the system prompt must go out as a `system` message.
            let Some(crate::types::CompatKind::OpenAiCompletions(compat)) = model.compat_kind()
            else {
                panic!("codebuddy model {} carries a compat object", model.id);
            };
            assert_eq!(compat.supports_developer_role, Some(false));
            // The gateway only reads the legacy `max_tokens` form; a
            // `max_completion_tokens` field is silently dropped.
            assert_eq!(
                compat.max_tokens_field,
                Some(crate::types::MaxTokensField::MaxTokens)
            );
        }
    }
}
