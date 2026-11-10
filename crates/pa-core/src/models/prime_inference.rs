//! Private Prime Inference models and the private-model predicates.

use pa_types::ai::{CompatKind, Model, ModelCompat, ModelCost, PRIME_INFERENCE_PROVIDER_ID};
use pa_types::JsNumber;

pub const PRIME_INFERENCE_BASE_URL: &str = "https://api.pinference.ai/api/v1";

/// Private ids: `internal/*`, `dev/*`, or any id containing `:`.
///
/// ONE implementation, in `pa-models` (the lower crate, which already owns the private
/// route table). The fork carried a copy here that differed by one keyword —
/// `to_lowercase()` where the other said `to_ascii_lowercase()` — so a non-ASCII id could
/// get different answers from the catalog and from the resolver (review #21). Re-exported
/// rather than duplicated: the call sites keep their path, the behaviour has one owner.
pub use pa_models::prime_inference::is_private_prime_inference_model_id;

/// Whether a `(provider, id)` pair names a private Prime Inference model.
///
/// The question is about the PAIR: an `internal/*` id on another provider is an ordinary
/// public model there, and a public id on `prime-inference` is public. This is the one
/// statement of that rule (TS `isPrivatePrimeInferenceModel({ provider, id })`);
/// [`is_private_prime_inference_model`] applies it to a [`Model`]'s two fields, and the
/// resolver re-exports this name for its callers that hold only strings (review #21 kept
/// the two as separate implementations, which then disagreed by one keyword).
#[must_use]
pub fn is_private_prime_inference_reference(provider: &str, model_id: &str) -> bool {
    provider == PRIME_INFERENCE_PROVIDER_ID && is_private_prime_inference_model_id(model_id)
}

#[must_use]
pub fn is_private_prime_inference_model(model: &Model) -> bool {
    is_private_prime_inference_reference(&model.provider, &model.id)
}

/// The bundled private model table. Private route templates matter: the public
/// provider default carries request shapes the private endpoint rejects.
#[must_use]
pub fn private_prime_inference_models() -> Vec<Model> {
    vec![Model {
        id: "internal/glm-5.2-fast".to_string(),
        name: "GLM 5.2 Fast".to_string(),
        api: "openai-completions".to_string(),
        provider: "prime-inference".to_string(),
        base_url: PRIME_INFERENCE_BASE_URL.to_string(),
        reasoning: true,
        input: vec![pa_types::ai::ModelInput::Text],
        headers: None,
        thinking_level_map: None,
        cost: ModelCost {
            input: JsNumber::from(0u64),
            output: JsNumber::from(0u64),
            cache_read: JsNumber::from(0u64),
            cache_write: JsNumber::from(0u64),
        },
        context_window: 400_000,
        max_tokens: 131_072,
        max_tokens_explicit: false,
        featured: Some(true),
        serving: None,
        compat: Some(ModelCompat::from_kind(CompatKind::OpenAiCompletions(
            Box::new(pa_types::ai::OpenAiCompletionsCompat {
                supports_developer_role: Some(false),
                max_tokens_field: Some(pa_types::ai::MaxTokensField::MaxTokens),
                ..pa_types::ai::OpenAiCompletionsCompat::default()
            }),
        ))),
    }]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The pair rule's oracle: `(provider, id)` classification, non-ASCII ids included.
    ///
    /// `İNTERNAL/x` stays public on purpose: both id predicates fold case with an ASCII
    /// lowercase (`to_ascii_lowercase`), so a non-ASCII capital never grows into the
    /// `internal/` prefix. Pinning the value keeps a future Unicode fold from silently
    /// widening the private set.
    #[test]
    fn the_pair_rule_classifies_private_references() {
        let cases = [
            ("prime-inference", "internal/glm-5.2-fast", true),
            ("prime-inference", "DEV/x", true),
            ("prime-inference", "z-ai/glm:exacto", true),
            ("prime-inference", "z-ai/glm-5.3", false),
            ("prime-inference", "İNTERNAL/x", false),
            ("prime-inference", "internal/İ", true),
            ("PRIME-INFERENCE", "internal/glm-5.2-fast", false),
            ("anthropic", "internal/glm-5.2-fast", false),
            ("", "internal/glm-5.2-fast", false),
        ];
        let template = private_prime_inference_models().remove(0);
        for (provider, id, expected) in cases {
            let model = Model {
                id: id.to_string(),
                provider: provider.to_string(),
                ..template.clone()
            };
            assert_eq!(
                is_private_prime_inference_reference(provider, id),
                expected,
                "{provider}/{id} (reference)"
            );
            assert_eq!(
                is_private_prime_inference_model(&model),
                expected,
                "{provider}/{id} (model)"
            );
        }
    }

    /// One definition, not two (review #21): the pair `(provider, id)` is the unit of the
    /// question, and the other entry points are thin wrappers on it.
    ///
    /// A behaviour test cannot see a duplicate that still agrees — which is exactly how the
    /// earlier pair drifted (it differed only by `to_lowercase` vs `to_ascii_lowercase`) — so
    /// the duplicate's absence is asserted at the definition site. The marker is split in two
    /// literals so this test's own text is not one of the hits, and it is a `fn` header
    /// substring, which no visibility or formatting change hides.
    #[test]
    fn the_private_reference_rule_is_defined_once() {
        let definition = concat!("fn is_private_prime_inference", "_reference(");
        let definitions = |source: &str| source.matches(definition).count();
        assert_eq!(
            definitions(include_str!("prime_inference.rs")),
            1,
            "the pair rule is defined here, once"
        );
        assert_eq!(
            definitions(include_str!("resolver.rs")),
            0,
            "resolver.rs must re-export the pair rule, not define a second copy"
        );
    }

    #[test]
    fn private_id_predicate() {
        assert!(is_private_prime_inference_model_id("internal/glm-5.2-fast"));
        assert!(is_private_prime_inference_model_id("DEV/x"));
        assert!(is_private_prime_inference_model_id("z-ai/glm:exacto"));
        assert!(!is_private_prime_inference_model_id("z-ai/glm-5.3"));
    }

    #[test]
    fn generated_catalog_base_urls_match_const() {
        let models = pa_ai::models_generated::get_models("prime-inference");
        assert!(
            !models.is_empty(),
            "generated prime-inference catalog is empty"
        );
        for model in models {
            assert_eq!(
                model.base_url, PRIME_INFERENCE_BASE_URL,
                "generated model {} diverges from PRIME_INFERENCE_BASE_URL",
                model.id
            );
        }
    }
}
