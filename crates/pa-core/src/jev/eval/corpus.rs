//! The frozen corpora: one per Phase-2 consumer. Hand-labelled, blind,
//! split dev/test, and pinned by [`Corpus::fingerprint`] in the fixture
//! tests - editing a case is a conscious re-freeze.
//!
//! Labels were assigned from the case text alone, before any candidate ran
//! (the "blind" rule); the dev split is where a threshold may be tuned, and
//! the test split is read once per corpus instance.
//!
//! Both corpora encode the *current* deterministic behaviour as the
//! baseline, so a delta means "better than what the fork ships today":
//!
//! - auto-thinking: the baseline is the deterministic default
//!   (`moderate` -> `medium`), which is exactly what a turn does with no
//!   judge at all.
//! - graph re-rank: the baseline is the search's own RRF order (top-1 wins),
//!   which is what `graph` returns today.

use super::{Case, Corpus, EvalError, Split};
use crate::jev::auto_thinking::Difficulty;

/// The auto-thinking corpus: 28 coding prompts, dev 16 / test 12, all three
/// difficulty labels in both splits.
pub type ThinkingCorpus = Corpus<String, Difficulty>;

/// The graph re-rank corpus: 26 symbol-search cases, dev 14 / test 12; the
/// label is the index of the candidate that actually answers the query, and
/// the baseline always picks index 0 (the RRF top-1).
pub type RerankCorpus = Corpus<RerankCase, usize>;

/// One re-rank case: a query plus the candidate list the search returned,
/// in RRF order (index 0 is the search's own top-1).
#[derive(Debug, Clone, serde::Serialize)]
pub struct RerankCase {
    pub query: String,
    pub candidates: Vec<RerankSpec>,
}

/// One candidate of a re-rank case.
#[derive(Debug, Clone, serde::Serialize)]
pub struct RerankSpec {
    pub symbol: String,
    pub file: String,
    pub label: String,
}

impl RerankCase {
    /// Build a case from `(symbol, file, kind)` triples.
    #[must_use]
    pub fn new(query: &str, candidates: &[(&str, &str, &str)]) -> RerankCase {
        RerankCase {
            query: query.to_string(),
            candidates: candidates
                .iter()
                .map(|(symbol, file, label)| RerankSpec {
                    symbol: (*symbol).to_string(),
                    file: (*file).to_string(),
                    label: (*label).to_string(),
                })
                .collect(),
        }
    }
}

/// One auto-thinking fixture row: `(id, split, label, prompt)`.
type ThinkingRow = (&'static str, Split, Difficulty, &'static str);

/// The auto-thinking cases. Prompts are paraphrases of real fork work
/// (documentation lookups, mechanical edits, multi-file features, and
/// open-ended debugging), each with a unique marker-free but distinctive
/// text the scripted judge keys on.
pub const THINKING_CASES: &[ThinkingRow] = &[
    // ── dev: threshold tuning ──
    (
        "think-dev-01",
        Split::Dev,
        Difficulty::Trivial,
        "what does the --verbose flag of cargo test do",
    ),
    (
        "think-dev-02",
        Split::Dev,
        Difficulty::Trivial,
        "rename the local variable `hits` to `matches` in search.rs",
    ),
    (
        "think-dev-03",
        Split::Dev,
        Difficulty::Trivial,
        "show me the current value of DEFAULT_TIMEOUT_MS",
    ),
    (
        "think-dev-04",
        Split::Dev,
        Difficulty::Trivial,
        "add a trailing newline to the CHANGELOG entry",
    ),
    (
        "think-dev-05",
        Split::Dev,
        Difficulty::Trivial,
        "which file declares the Judge trait",
    ),
    (
        "think-dev-06",
        Split::Dev,
        Difficulty::Moderate,
        "add a --json flag to the status command and cover it with one test",
    ),
    (
        "think-dev-07",
        Split::Dev,
        Difficulty::Moderate,
        "make the retry helper honour retry-after for 429 responses",
    ),
    (
        "think-dev-08",
        Split::Dev,
        Difficulty::Moderate,
        "wire the new setting into the settings manager getters",
    ),
    (
        "think-dev-09",
        Split::Dev,
        Difficulty::Moderate,
        "split the tool module into dispatch and rendering files",
    ),
    (
        "think-dev-10",
        Split::Dev,
        Difficulty::Moderate,
        "add an integration test that indexes a temp project and searches it",
    ),
    (
        "think-dev-11",
        Split::Dev,
        Difficulty::Moderate,
        "the build fails on the new enum arm, fix the exhaustive match",
    ),
    (
        "think-dev-12",
        Split::Dev,
        Difficulty::Hard,
        "the session occasionally hangs after compaction, find out why",
    ),
    (
        "think-dev-13",
        Split::Dev,
        Difficulty::Hard,
        "design a cross-session admission limit that never starves a provider",
    ),
    (
        "think-dev-14",
        Split::Dev,
        Difficulty::Hard,
        "search latency regressed 3x after the MMR change, diagnose and fix",
    ),
    (
        "think-dev-15",
        Split::Dev,
        Difficulty::Hard,
        "two writers race on the answer cache under load, make it correct",
    ),
    (
        "think-dev-16",
        Split::Dev,
        Difficulty::Hard,
        "decide how the judge layer should degrade when every candidate is down",
    ),
    // ── test: the decision (one touch) ──
    (
        "think-test-01",
        Split::Test,
        Difficulty::Trivial,
        "print the current git branch",
    ),
    (
        "think-test-02",
        Split::Test,
        Difficulty::Trivial,
        "delete the unused import in cache.rs",
    ),
    (
        "think-test-03",
        Split::Test,
        Difficulty::Trivial,
        "what is the default value of reserveTokens",
    ),
    (
        "think-test-04",
        Split::Test,
        Difficulty::Trivial,
        "bump the patch version in Cargo.toml",
    ),
    (
        "think-test-05",
        Split::Test,
        Difficulty::Moderate,
        "add a `--since` filter to the changes action",
    ),
    (
        "think-test-06",
        Split::Test,
        Difficulty::Moderate,
        "move the truncation helper into its own module and re-export it",
    ),
    (
        "think-test-07",
        Split::Test,
        Difficulty::Moderate,
        "the doc comment on JudgeError is stale, update it",
    ),
    (
        "think-test-08",
        Split::Test,
        Difficulty::Moderate,
        "teach the graph tool to accept a comma-separated label filter",
    ),
    (
        "think-test-09",
        Split::Test,
        Difficulty::Moderate,
        "add a unit test for the cooldown expiry path",
    ),
    (
        "think-test-10",
        Split::Test,
        Difficulty::Hard,
        "memory grows without bound in a long session, find the leak",
    ),
    (
        "think-test-11",
        Split::Test,
        Difficulty::Hard,
        "choose a cache key scheme that stays correct across judge model upgrades",
    ),
    (
        "think-test-12",
        Split::Test,
        Difficulty::Hard,
        "the tool roster sometimes loses the native tools after a profile switch",
    ),
];

/// Build the auto-thinking corpus.
///
/// # Errors
///
/// Whatever [`Corpus::new`] rejects (a fixture edit that breaks the split
/// invariants).
pub fn thinking_corpus() -> Result<ThinkingCorpus, EvalError> {
    let cases = THINKING_CASES
        .iter()
        .map(|(id, split, label, prompt)| Case {
            id: (*id).to_string(),
            input: (*prompt).to_string(),
            label: *label,
            split: *split,
            tags: Vec::new(),
        })
        .collect();
    Corpus::new("auto-thinking", "2026-10-16", cases)
}

/// One re-rank fixture row: `(id, split, expected index, query, candidates)`.
type RerankRow = (
    &'static str,
    Split,
    usize,
    &'static str,
    &'static [(&'static str, &'static str, &'static str)],
);

/// The graph re-rank cases. Every list is in RRF order: index 0 is the
/// lexical/degree winner, which is wrong on the cases labelled 1..=3 (a
/// substring or a structural neighbour outranked the real answer).
pub const RERANK_CASES: &[RerankRow] = &[
    // ── dev ──
    (
        "rerank-dev-01",
        Split::Dev,
        1,
        "where is the answer cache lookup implemented",
        &[
            (
                "cache::CacheStats::lookup",
                "crates/pa-core/src/jev/cache.rs",
                "method",
            ),
            (
                "cache::AnswerCache::lookup",
                "crates/pa-core/src/jev/cache.rs",
                "method",
            ),
            (
                "cache::AnswerCache::record",
                "crates/pa-core/src/jev/cache.rs",
                "method",
            ),
            (
                "settings::get_jev_cache_enabled",
                "crates/pa-core/src/settings/manager.rs",
                "method",
            ),
        ],
    ),
    (
        "rerank-dev-02",
        Split::Dev,
        0,
        "how does the chain cool down a rejected candidate",
        &[
            (
                "chain::record_rejection",
                "crates/pa-core/src/jev/chain.rs",
                "function",
            ),
            (
                "chain::is_cooled_down",
                "crates/pa-core/src/jev/chain.rs",
                "function",
            ),
            (
                "chain::enforce_native_only",
                "crates/pa-core/src/jev/chain.rs",
                "function",
            ),
            ("error::JevError", "crates/pa-jev/src/error.rs", "struct"),
        ],
    ),
    (
        "rerank-dev-03",
        Split::Dev,
        2,
        "which function truncates the prompt for the classifier",
        &[
            (
                "auto_thinking::preprocess_prompt",
                "crates/pa-core/src/jev/auto_thinking.rs",
                "function",
            ),
            (
                "auto_thinking::strip_ansi",
                "crates/pa-core/src/jev/auto_thinking.rs",
                "function",
            ),
            (
                "auto_thinking::truncate_middle",
                "crates/pa-core/src/jev/auto_thinking.rs",
                "function",
            ),
            (
                "auto_thinking::collapse_whitespace",
                "crates/pa-core/src/jev/auto_thinking.rs",
                "function",
            ),
        ],
    ),
    (
        "rerank-dev-04",
        Split::Dev,
        0,
        "how is the answer cache key built",
        &[
            (
                "cache::question_key",
                "crates/pa-core/src/jev/cache.rs",
                "function",
            ),
            (
                "cache::CachedAnswers",
                "crates/pa-core/src/jev/cache.rs",
                "struct",
            ),
            (
                "cache::AnswerCache",
                "crates/pa-core/src/jev/cache.rs",
                "struct",
            ),
            (
                "models::ModelRegistry::get_all",
                "crates/pa-core/src/models.rs",
                "method",
            ),
        ],
    ),
    (
        "rerank-dev-05",
        Split::Dev,
        1,
        "what decides whether the jev tool joins the roster",
        &[
            (
                "jev::should_install_jev_tool",
                "crates/pa-core/src/jev/mod.rs",
                "function",
            ),
            (
                "jev::resolve_judge",
                "crates/pa-core/src/jev/mod.rs",
                "function",
            ),
            (
                "jev::has_native_judge",
                "crates/pa-core/src/jev/mod.rs",
                "function",
            ),
            (
                "settings::get_jev_enabled",
                "crates/pa-core/src/settings/manager.rs",
                "method",
            ),
        ],
    ),
    (
        "rerank-dev-06",
        Split::Dev,
        3,
        "where does the session switch the thinking level",
        &[
            (
                "session_engine::AgentSession::set_thinking_level",
                "crates/pa-core/src/session_engine/mod.rs",
                "method",
            ),
            (
                "session_engine::AgentSession::set_model",
                "crates/pa-core/src/session_engine/mod.rs",
                "method",
            ),
            (
                "session_engine::AgentSession::prompt",
                "crates/pa-core/src/session_engine/admission.rs",
                "method",
            ),
            (
                "session_engine::AgentSession::set_model_and_thinking_level",
                "crates/pa-core/src/session_engine/mod.rs",
                "method",
            ),
        ],
    ),
    (
        "rerank-dev-07",
        Split::Dev,
        0,
        "which type carries the judgment request",
        &[
            (
                "types::JudgmentRequest",
                "crates/pa-jev/src/types.rs",
                "struct",
            ),
            (
                "types::JudgmentResult",
                "crates/pa-jev/src/types.rs",
                "struct",
            ),
            ("types::JudgmentState", "crates/pa-jev/src/types.rs", "enum"),
            ("wire::WireDialect", "crates/pa-jev/src/wire.rs", "enum"),
        ],
    ),
    (
        "rerank-dev-08",
        Split::Dev,
        2,
        "how does the client price a judgment",
        &[
            (
                "client::JevClient::judge",
                "crates/pa-jev/src/client.rs",
                "method",
            ),
            (
                "client::JevClient::new",
                "crates/pa-jev/src/client.rs",
                "function",
            ),
            ("types::Usage", "crates/pa-jev/src/types.rs", "struct"),
            (
                "types::JEV_INPUT_PRICE_PER_MTOK",
                "crates/pa-jev/src/types.rs",
                "constant",
            ),
        ],
    ),
    (
        "rerank-dev-09",
        Split::Dev,
        1,
        "which function renders a judgment for the model",
        &[
            (
                "tool::JevTool::dispatch",
                "crates/pa-core/src/jev/tool.rs",
                "method",
            ),
            (
                "tool::render_result",
                "crates/pa-core/src/jev/tool.rs",
                "function",
            ),
            (
                "tool::params_schema",
                "crates/pa-core/src/jev/tool.rs",
                "function",
            ),
            (
                "tool::JevTool::request_classify",
                "crates/pa-core/src/jev/tool.rs",
                "method",
            ),
        ],
    ),
    (
        "rerank-dev-10",
        Split::Dev,
        0,
        "where are the graph search hits assembled",
        &[
            (
                "graph::GraphTool::action_search",
                "crates/pa-core/src/tools/graph.rs",
                "method",
            ),
            (
                "graph::GraphTool::action_trace",
                "crates/pa-core/src/tools/graph.rs",
                "method",
            ),
            (
                "graph::GraphTool::dispatch",
                "crates/pa-core/src/tools/graph.rs",
                "method",
            ),
            (
                "search::SearchOptions",
                "crates/pa-graph/src/search.rs",
                "struct",
            ),
        ],
    ),
    (
        "rerank-dev-11",
        Split::Dev,
        3,
        "how does the native judge merge cached and fresh answers",
        &[
            (
                "native::NativeJudge::new",
                "crates/pa-core/src/jev/native.rs",
                "function",
            ),
            (
                "native::NativeJudge::map_error",
                "crates/pa-core/src/jev/native.rs",
                "method",
            ),
            (
                "cache::AnswerCache::record",
                "crates/pa-core/src/jev/cache.rs",
                "method",
            ),
            (
                "native::NativeJudge::judge",
                "crates/pa-core/src/jev/native.rs",
                "method",
            ),
        ],
    ),
    (
        "rerank-dev-12",
        Split::Dev,
        1,
        "where does the judge candidate list get its cooldown ledger",
        &[
            (
                "chain::ChainJudge::new",
                "crates/pa-core/src/jev/chain.rs",
                "function",
            ),
            (
                "chain::shared_rejections",
                "crates/pa-core/src/jev/chain.rs",
                "function",
            ),
            (
                "chain::ChainJudge::with_rejections",
                "crates/pa-core/src/jev/chain.rs",
                "function",
            ),
            (
                "chain::JudgeCandidate",
                "crates/pa-core/src/jev/chain.rs",
                "struct",
            ),
        ],
    ),
    (
        "rerank-dev-13",
        Split::Dev,
        0,
        "which settings getter reads the jev model pin",
        &[
            (
                "settings::get_jev_model",
                "crates/pa-core/src/settings/manager.rs",
                "method",
            ),
            (
                "settings::get_jev_timeout_ms",
                "crates/pa-core/src/settings/manager.rs",
                "method",
            ),
            (
                "settings::get_jev_fallback_chain",
                "crates/pa-core/src/settings/manager.rs",
                "method",
            ),
            (
                "jev::DEFAULT_JEV_MODEL",
                "crates/pa-core/src/jev/mod.rs",
                "constant",
            ),
        ],
    ),
    (
        "rerank-dev-14",
        Split::Dev,
        2,
        "how is a choice question validated before it is sent",
        &[
            (
                "types::Question::validate",
                "crates/pa-jev/src/types.rs",
                "method",
            ),
            (
                "types::Question::kind",
                "crates/pa-jev/src/types.rs",
                "method",
            ),
            (
                "types::JudgmentRequest::validate",
                "crates/pa-jev/src/types.rs",
                "method",
            ),
            (
                "types::MAX_CHOICE_OPTIONS",
                "crates/pa-jev/src/types.rs",
                "constant",
            ),
        ],
    ),
    // ── test: the decision (one touch) ──
    (
        "rerank-test-01",
        Split::Test,
        1,
        "where is the escape option appended to a menu",
        &[
            (
                "tool::JevTool::request_classify",
                "crates/pa-core/src/jev/tool.rs",
                "method",
            ),
            (
                "tool::is_escape_name",
                "crates/pa-core/src/jev/tool.rs",
                "function",
            ),
            (
                "tool::ESCAPE_OPTION",
                "crates/pa-core/src/jev/tool.rs",
                "constant",
            ),
            (
                "tool::parse_question",
                "crates/pa-core/src/jev/tool.rs",
                "function",
            ),
        ],
    ),
    (
        "rerank-test-02",
        Split::Test,
        0,
        "which function hashes the corpus for the gate",
        &[
            (
                "eval::sha256_hex",
                "crates/pa-core/src/jev/eval/mod.rs",
                "function",
            ),
            (
                "eval::Corpus::fingerprint",
                "crates/pa-core/src/jev/eval/mod.rs",
                "method",
            ),
            (
                "eval::Corpus::unseal_test",
                "crates/pa-core/src/jev/eval/mod.rs",
                "method",
            ),
            (
                "eval::validate",
                "crates/pa-core/src/jev/eval/mod.rs",
                "function",
            ),
        ],
    ),
    (
        "rerank-test-03",
        Split::Test,
        3,
        "how does the gate reject a tie",
        &[
            (
                "eval::gate",
                "crates/pa-core/src/jev/eval/mod.rs",
                "function",
            ),
            (
                "eval::GateDecision",
                "crates/pa-core/src/jev/eval/mod.rs",
                "enum",
            ),
            (
                "eval::GateRejection",
                "crates/pa-core/src/jev/eval/mod.rs",
                "enum",
            ),
            (
                "eval::TunedThresholds::tune",
                "crates/pa-core/src/jev/eval/mod.rs",
                "function",
            ),
        ],
    ),
    (
        "rerank-test-04",
        Split::Test,
        0,
        "where does the difficulty classifier build its question",
        &[
            (
                "auto_thinking::difficulty_request",
                "crates/pa-core/src/jev/auto_thinking.rs",
                "function",
            ),
            (
                "auto_thinking::Difficulty::thinking_level",
                "crates/pa-core/src/jev/auto_thinking.rs",
                "method",
            ),
            (
                "auto_thinking::DifficultyClassifier::classify",
                "crates/pa-core/src/jev/auto_thinking.rs",
                "method",
            ),
            (
                "auto_thinking::MAX_PROMPT_CHARS",
                "crates/pa-core/src/jev/auto_thinking.rs",
                "constant",
            ),
        ],
    ),
    (
        "rerank-test-05",
        Split::Test,
        2,
        "which function turns a difficulty into a thinking level",
        &[
            (
                "auto_thinking::Difficulty::parse",
                "crates/pa-core/src/jev/auto_thinking.rs",
                "method",
            ),
            (
                "auto_thinking::Difficulty::as_str",
                "crates/pa-core/src/jev/auto_thinking.rs",
                "method",
            ),
            (
                "auto_thinking::Difficulty::thinking_level",
                "crates/pa-core/src/jev/auto_thinking.rs",
                "method",
            ),
            (
                "auto_thinking::Difficulty::deterministic_default",
                "crates/pa-core/src/jev/auto_thinking.rs",
                "method",
            ),
        ],
    ),
    (
        "rerank-test-06",
        Split::Test,
        1,
        "how are demoted candidates reported",
        &[
            (
                "graph_rerank::RerankOutcome::demoted_any",
                "crates/pa-core/src/jev/graph_rerank.rs",
                "method",
            ),
            (
                "graph_rerank::RerankOutcome",
                "crates/pa-core/src/jev/graph_rerank.rs",
                "struct",
            ),
            (
                "graph_rerank::DEMOTE_BELOW",
                "crates/pa-core/src/jev/graph_rerank.rs",
                "constant",
            ),
            (
                "graph_rerank::VERIFIED_MARKER",
                "crates/pa-core/src/jev/graph_rerank.rs",
                "constant",
            ),
        ],
    ),
    (
        "rerank-test-07",
        Split::Test,
        0,
        "where does the reranker bound its top-k",
        &[
            (
                "graph_rerank::GraphReranker::with_top_k",
                "crates/pa-core/src/jev/graph_rerank.rs",
                "method",
            ),
            (
                "graph_rerank::MAX_RERANK_CANDIDATES",
                "crates/pa-core/src/jev/graph_rerank.rs",
                "constant",
            ),
            (
                "graph_rerank::rerank_request",
                "crates/pa-core/src/jev/graph_rerank.rs",
                "function",
            ),
            (
                "graph_rerank::GraphReranker::rerank",
                "crates/pa-core/src/jev/graph_rerank.rs",
                "method",
            ),
        ],
    ),
    (
        "rerank-test-08",
        Split::Test,
        2,
        "which function cleans the prompt before judging it",
        &[
            (
                "auto_thinking::strip_fenced_blocks",
                "crates/pa-core/src/jev/auto_thinking.rs",
                "function",
            ),
            (
                "auto_thinking::strip_ansi",
                "crates/pa-core/src/jev/auto_thinking.rs",
                "function",
            ),
            (
                "auto_thinking::preprocess_prompt",
                "crates/pa-core/src/jev/auto_thinking.rs",
                "function",
            ),
            (
                "auto_thinking::collapse_whitespace",
                "crates/pa-core/src/jev/auto_thinking.rs",
                "function",
            ),
        ],
    ),
    (
        "rerank-test-09",
        Split::Test,
        1,
        "where does the tool validate the question shape",
        &[
            (
                "tool::JevTool::dispatch",
                "crates/pa-core/src/jev/tool.rs",
                "method",
            ),
            (
                "types::JudgmentRequest::validate",
                "crates/pa-jev/src/types.rs",
                "method",
            ),
            (
                "tool::render_result",
                "crates/pa-core/src/jev/tool.rs",
                "function",
            ),
            (
                "tool::JevTool::request_ask",
                "crates/pa-core/src/jev/tool.rs",
                "method",
            ),
        ],
    ),
    (
        "rerank-test-10",
        Split::Test,
        0,
        "which function picks a threshold grid value",
        &[
            (
                "eval::TunedThresholds::tune",
                "crates/pa-core/src/jev/eval/mod.rs",
                "function",
            ),
            (
                "eval::TuningPolicy",
                "crates/pa-core/src/jev/eval/mod.rs",
                "struct",
            ),
            (
                "eval::TuningError",
                "crates/pa-core/src/jev/eval/mod.rs",
                "enum",
            ),
            (
                "eval::SplitReport::accuracy_delta_points",
                "crates/pa-core/src/jev/eval/mod.rs",
                "method",
            ),
        ],
    ),
    (
        "rerank-test-11",
        Split::Test,
        3,
        "how does the reranker keep the rrf order",
        &[
            (
                "graph_rerank::GraphReranker::new",
                "crates/pa-core/src/jev/graph_rerank.rs",
                "function",
            ),
            (
                "graph_rerank::question_id",
                "crates/pa-core/src/jev/graph_rerank.rs",
                "function",
            ),
            (
                "graph_rerank::RerankCandidate",
                "crates/pa-core/src/jev/graph_rerank.rs",
                "struct",
            ),
            (
                "graph_rerank::GraphReranker::rerank_or_error",
                "crates/pa-core/src/jev/graph_rerank.rs",
                "method",
            ),
        ],
    ),
    (
        "rerank-test-12",
        Split::Test,
        1,
        "where is the answer cache bounded",
        &[
            (
                "cache::AnswerCache::record",
                "crates/pa-core/src/jev/cache.rs",
                "method",
            ),
            (
                "cache::MAX_ENTRIES",
                "crates/pa-core/src/jev/cache.rs",
                "constant",
            ),
            (
                "cache::CachedAnswers",
                "crates/pa-core/src/jev/cache.rs",
                "struct",
            ),
            (
                "cache::AnswerCache::lookup",
                "crates/pa-core/src/jev/cache.rs",
                "method",
            ),
        ],
    ),
];

/// Build the graph re-rank corpus.
///
/// # Errors
///
/// Whatever [`Corpus::new`] rejects.
pub fn rerank_corpus() -> Result<RerankCorpus, EvalError> {
    let cases = RERANK_CASES
        .iter()
        .map(|(id, split, expected, query, candidates)| Case {
            id: (*id).to_string(),
            input: RerankCase::new(query, candidates),
            label: *expected,
            split: *split,
            tags: Vec::new(),
        })
        .collect();
    Corpus::new("graph-rerank", "2026-10-16", cases)
}
