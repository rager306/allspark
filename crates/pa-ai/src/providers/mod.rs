//! Streaming provider implementations ported from `packages/ai/src/providers`.

/// Whether the `api` dialect has a session provider implementation: a model
/// outside this set cannot serve a chat turn, so picker surfaces must not
/// offer it (`typesafe` is the judgment dialect — a decision model must
/// never resolve as a chat backend).
pub fn serves_chat_turns(api: &str) -> bool {
    matches!(
        api,
        anthropic::API_ANTHROPIC_MESSAGES
            | azure_openai_responses::API_AZURE_OPENAI_RESPONSES
            | bedrock::API_BEDROCK_CONVERSE_STREAM
            | google_shared::API_GOOGLE_GENERATIVE_AI
            | google_shared::API_GOOGLE_VERTEX
            | mistral::API_MISTRAL_CONVERSATIONS
            | openai_codex_responses::API_OPENAI_CODEX_RESPONSES
            | openai_completions::API_OPENAI_COMPLETIONS
            | openai_responses::API_OPENAI_RESPONSES
    )
}

pub mod anthropic;
pub mod azure_openai_responses;
pub mod bedrock;
pub mod faux;
pub mod google;
pub mod google_shared;
pub mod google_stream;
pub mod google_vertex;
pub mod mistral;
pub mod openai_codex_responses;
pub mod openai_completions;
pub mod openai_responses;
pub mod openai_responses_hooks;
pub mod openai_responses_shared;
pub mod openai_responses_stream;
pub mod simple_options;
pub mod transform_messages;
