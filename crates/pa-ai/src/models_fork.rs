//! Fork-only built-in provider catalog.
// FORK-ONLY: this file does not exist upstream. See docs/fork-identity.md.
//!
//! The upstream catalog chain (remote fetch -> disk cache -> bundled snapshot ->
//! [`crate::models_generated`]) never carries this fork's providers, so the
//! shipped [`FORK_CATALOG_ASSET`] carries them and `pa-core`'s registry merges
//! them into the built-in set after the chain resolves. The asset is compiled
//! into the binary, so the offline path needs no network and no disk read; a
//! runtime `$AGENT_DIR/models/fork-models.v1.json` overrides it by
//! `(provider, id)`. [`crate::models_generated`] stays a byte-faithful mirror of
//! the TS fallback file; fork additions never touch it.
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

use crate::types::Model;

/// The shipped fork catalog, compiled into the binary. This asset is the
/// source: adding or changing a fork model is an asset edit, not a Rust edit
/// and a rebuild. The compiled-in form is also the offline path (no network,
/// no disk read).
const FORK_CATALOG_ASSET: &str = include_str!("../assets/fork-models.v1.json");

/// The parsed catalog, once per process.
static FORK_MODELS: LazyLock<Vec<Model>> = LazyLock::new(|| {
    serde_json::from_str(FORK_CATALOG_ASSET)
        .expect("the shipped fork catalog must parse (crates/pa-ai/assets/fork-models.v1.json)")
});

/// The fork's built-in providers, merged into the registry's built-in set.
#[must_use]
pub fn fork_provider_models() -> Vec<Model> {
    FORK_MODELS.clone()
}

/// Most judgment questions one request may carry for `provider`.
///
/// An endpoint fact, not a chain fact: `SubQ Snap` answers 422 to a 17-question
/// request, so its batches must be split. `None` means "no known cap" — the
/// caller sends the request whole. Expressed in code because the shipped
/// catalog's schema has no field for it.
#[must_use]
pub fn judgment_question_cap(provider: &str) -> Option<usize> {
    match provider {
        "subquadratic" => Some(16),
        _ => None,
    }
}

/// The fork's provider ids (the display-name and env-key maps stay in sync
/// with this list).
#[must_use]
pub fn fork_providers() -> &'static [&'static str] {
    &[
        "muse",
        "codebuddy",
        "genspark",
        "subquadratic",
        "ninerouter",
        // The free-tier natives. Their models were already in the catalog while
        // the provider id was absent here, so anything keyed off this list had no
        // entry for them (issue #9).
        "opencode",
        "agnes",
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{ModelExt, ModelThinkingLevel};

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

    /// The cap is what makes a >16-question batch legal against `SubQ Snap`:
    /// the endpoint answers 422 to 17 (`models_fork.rs` entry comment). A
    /// provider without a known cap must stay uncapped rather than inherit
    /// another endpoint's limit.
    #[test]
    fn only_the_endpoint_that_caps_questions_reports_one() {
        assert_eq!(judgment_question_cap("subquadratic"), Some(16));
        assert_eq!(judgment_question_cap("typesafe"), None);
        assert_eq!(judgment_question_cap("muse"), None);
    }

    #[test]
    fn subquadratic_snap_joins_the_judge_chain_through_the_typesafe_dialect() {
        let models = fork_provider_models();
        let snap = models
            .iter()
            .find(|model| model.provider == "subquadratic")
            .expect("the subquadratic catalog entry exists");
        assert_eq!(snap.id, "subq-snap-1.2");
        // The wire dialect is what resolves the judge candidate; a decision
        // model must never resolve as a chat backend.
        assert_eq!(snap.api, "typesafe");
        assert_eq!(snap.base_url, "https://api.subq.ai");
        // Input-only billing: the output side of the tuple stays zero so
        // usage accounting never invents output cost.
        assert_eq!(snap.cost.input, 0.04.into());
        assert_eq!(snap.cost.output, 0.0.into());
    }

    #[test]
    fn every_catalog_dialect_except_the_judgment_one_serves_chat_turns() {
        // The picker filters on `serves_chat_turns`; the catalog cross-check
        // keeps the predicate and the catalog in lockstep: a new chat dialect
        // must join the predicate or its models silently vanish from every
        // picker, and a judgment-only dialect must never leak back in.
        for model in fork_provider_models() {
            let expected = model.api != "typesafe";
            assert_eq!(
                crate::providers::serves_chat_turns(&model.api),
                expected,
                "{}/{} (api {})",
                model.provider,
                model.id,
                model.api
            );
        }
        let models = fork_provider_models();
        let snap = models
            .iter()
            .find(|model| model.provider == "subquadratic")
            .expect("the subquadratic catalog entry exists");
        assert!(
            !crate::providers::serves_chat_turns(&snap.api),
            "the judgment dialect must never resolve as a chat backend"
        );
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
                map.get(&ModelThinkingLevel::Minimal),
                Some(&Some("minimal".to_string()))
            );
            // 1.1/1.2 reject `max` (400 with the supported list); only 1.3+ accepts it.
            let accepts_max = matches!(
                model.id.as_str(),
                "muse-spark-1.3" | "muse-spark-1.3-contributor"
            );
            assert_eq!(
                map.get(&ModelThinkingLevel::Max).is_some(),
                accepts_max,
                "{}: max must be advertised only by 1.3+",
                model.id
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

    /// The gateway's security policy rejects `developer`-role messages outright
    /// ("Illegal API invocation from an unapproved channel"), so the system
    /// prompt must go out as a `system` message; and it only reads the legacy
    /// `max_tokens` form, silently dropping `max_completion_tokens`.
    #[test]
    fn codebuddy_models_carry_the_gateway_headers_and_compat() {
        let models: Vec<_> = fork_provider_models()
            .into_iter()
            .filter(|model| model.provider == "codebuddy")
            .collect();
        assert_eq!(models.len(), 15, "the codebuddy.ai IDE lineup");
        for model in models {
            assert_eq!(model.api, "openai-completions", "{}", model.id);
            assert_eq!(model.base_url, "https://www.codebuddy.ai/v2");
            let headers = model.headers.as_ref().expect("codebuddy carries headers");
            for name in [
                "User-Agent",
                "X-Product",
                "X-IDE-Type",
                "X-IDE-Name",
                "x-requested-with",
                "x-codebuddy-request",
            ] {
                assert!(headers.contains_key(name), "{} misses {name}", model.id);
            }
            let Some(crate::types::CompatKind::OpenAiCompletions(compat)) = model.compat_kind()
            else {
                panic!("codebuddy model {} carries a compat object", model.id);
            };
            assert_eq!(compat.supports_developer_role, Some(false));
            assert_eq!(
                compat.max_tokens_field,
                Some(crate::types::MaxTokensField::MaxTokens)
            );
        }
    }

    /// 9router v0.5.109 (`313559f4`): the `OpenCode` free `deepseek-v4.1-flash`
    /// serves `low|high|max` verbatim — no low→high collapse, no medium/xhigh.
    #[test]
    fn opencode_free_deepseek_serves_low_high_max_only() {
        for (provider, id) in [
            ("opencode", "deepseek-v4.1-flash-free"),
            ("ninerouter", "oc/deepseek-v4.1-flash-free"),
        ] {
            let model = fork_provider_models()
                .into_iter()
                .find(|model| model.provider == provider && model.id == id)
                .unwrap_or_else(|| panic!("{provider}/{id} entry"));
            let map = model.thinking_level_map.expect("the entry carries a map");
            let served: Vec<&str> = map.values().filter_map(|wire| wire.as_deref()).collect();
            assert_eq!(served, vec!["low", "high", "max"], "{provider}/{id}");
        }
    }

    #[test]
    fn an_asset_only_model_is_served() {
        // The asset is the source: this id exists in no Rust table.
        let data: Vec<serde_json::Value> =
            serde_json::from_str(FORK_CATALOG_ASSET).expect("the asset parses");
        let asset_ids: std::collections::HashSet<&str> = data
            .iter()
            .filter_map(|entry| entry.get("id").and_then(serde_json::Value::as_str))
            .collect();
        let served: std::collections::HashSet<String> = fork_provider_models()
            .into_iter()
            .map(|model| model.id)
            .collect();
        for id in asset_ids {
            assert!(
                served.contains(id),
                "the served catalog must carry the asset entry {id}"
            );
        }
    }
}
