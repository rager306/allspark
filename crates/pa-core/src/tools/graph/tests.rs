use crate::tools::graph::build::*;
use crate::tools::graph::tool::schema::{params_schema, ACCEPTED_PARAMS};
use crate::tools::graph::tool::GraphTool;
use pa_agent::types::AgentTool;
use pa_agent::types::AgentToolResult;
use pa_graph::{GraphStore, IndexReport, SearchOptions};

/// The review's §7: "сохранять witness path", and §10's "почему этот модуль затронут".
/// "C, via CALLS" says how a caller was reached; the CHAIN says why.
#[tokio::test]
async fn impact_explains_why_through_the_witness_path() {
    let dir = tempfile::tempdir().unwrap();
    // A three-hop chain: leaf <- mid <- top. `impact leaf` must show the whole chain for
    // the deepest caller, not just its last edge.
    std::fs::write(
        dir.path().join("lib.rs"),
        "pub fn leaf() -> u32 { 1 }\n\
             pub fn mid() -> u32 { leaf() }\n\
             pub fn top() -> u32 { mid() }\n",
    )
    .unwrap();
    let tool = tool(dir.path());
    tool.dispatch(&serde_json::json!({ "action": "index" }))
        .unwrap();
    let result = tool
        .dispatch(&serde_json::json!({
            "action": "impact",
            "symbol": "leaf",
            "depth": 3,
        }))
        .unwrap();
    let details = result.details.clone();
    let callers = details["callers"].as_array().expect("callers");
    let top = callers
        .iter()
        .find(|caller| caller["qualifiedName"] == serde_json::json!("lib::top"))
        .unwrap_or_else(|| panic!("top must be reached at depth 2: {details}"));
    let witness: Vec<&str> = top["witness"]
        .as_array()
        .expect("a witness path")
        .iter()
        .filter_map(|step| step.as_str())
        .collect();
    assert_eq!(
        witness,
        vec!["lib::leaf", "lib::mid", "lib::top"],
        "the witness must be the WHOLE chain from the changed symbol"
    );
    assert!(
        text_of(&result).contains("why: lib::leaf -> lib::mid -> lib::top"),
        "and the readable answer must show the chain: {}",
        text_of(&result)
    );
}

fn tool(dir: &std::path::Path) -> GraphTool {
    GraphTool::new(dir.join("graph.db"), dir.to_path_buf())
}

/// The first text block of a result, for assertions.
fn text_of(result: &AgentToolResult) -> &str {
    match &result.content[0] {
        pa_agent::types::ToolResultContent::Text(content) => &content.text,
        other @ pa_agent::types::ToolResultContent::Image(_) => {
            panic!("expected a text block, got {other:?}")
        }
    }
}

#[tokio::test]
async fn index_then_search_finds_the_symbol() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("lib.rs"),
        "pub fn parse_config(path: &str) -> u32 { load(path) }\n\
             pub fn load(path: &str) -> u32 { path.len() as u32 }\n",
    )
    .unwrap();
    let tool = tool(dir.path());
    let result = tool
        .dispatch(&serde_json::json!({ "action": "index" }))
        .unwrap();
    assert!(!result.is_error);
    let text = text_of(&result);
    assert!(text.contains("Indexed project"), "{text}");

    let search = tool
        .dispatch(&serde_json::json!({ "action": "search", "query": "load" }))
        .unwrap();
    let text = text_of(&search);
    assert!(text.contains("load"), "{text}");
}

/// Review (tools report, #1.2): a zero-step trace is UNKNOWN, not "none".
///
/// Go method-values (`r.POST("/", handler.Serve)`), interface dispatch and
/// cross-package calls do not resolve into call edges, so an agent that reads
/// "0 step(s)" as "no callers" removes live code. The neutral rendering said
/// nothing; the answer must say what a zero actually means and name the
/// cross-check.
/// A pack is evidence for a decision, so each element must say whether it still
/// matches the disk: editing from an indexed line number after the file moved
/// writes into the wrong place.
///
/// The freshness check is tested as its own unit rather than through
/// `action=context`, because that action auto-reindexes a stale tree — the
/// refresh is correct, and it makes the stale branch unreachable from the tool.
/// A unit test that went through the tool would pass for the wrong reason.
#[test]
fn the_freshness_check_separates_a_moved_symbol_from_an_unreadable_file() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("lib.rs"),
        "pub fn fresh_parser() -> u32 { 1 }\n\npub fn moved_parser() -> u32 { 2 }\n",
    )
    .unwrap();

    // Where the index says the symbol is (line 3): the file still agrees.
    assert_eq!(
        current_placement(dir.path(), "lib.rs", "moved_parser"),
        Ok(Some((3, 3)))
    );

    // Move it down WITHOUT touching the body: the index would still say 3.
    std::fs::write(
        dir.path().join("lib.rs"),
        "pub fn fresh_parser() -> u32 { 1 }\n\n\n\n\npub fn moved_parser() -> u32 { 2 }\n",
    )
    .unwrap();
    assert_eq!(
        current_placement(dir.path(), "lib.rs", "moved_parser"),
        Ok(Some((6, 6))),
        "the check must see the new line, which is what makes `stale` possible"
    );

    // A file that is gone: `None`, not an error — the symbol may exist elsewhere.
    assert_eq!(current_placement(dir.path(), "gone.rs", "x"), Ok(None));
    // A symbol the file no longer defines: no placement.
    assert_eq!(
        current_placement(dir.path(), "lib.rs", "vanished"),
        Ok(None)
    );
}

/// A pack with nothing to show must say UNKNOWN, and must report the coverage
/// that explains why — a bare "0" reads as "no such code".
#[tokio::test]
async fn an_empty_pack_reports_coverage_not_an_absence_claim() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("lib.rs"), "pub fn only() -> u32 { 1 }\n").unwrap();
    let tool = tool(dir.path());
    tool.dispatch(&serde_json::json!({ "action": "index" }))
        .unwrap();
    let pack = tool
        .dispatch(&serde_json::json!({
            "action": "context",
            "query": "zzz_matches_nothing",
        }))
        .unwrap();
    let text = text_of(&pack);
    assert!(text.contains("UNKNOWN"), "{text}");
    assert!(
        text.contains("Coverage:"),
        "the pack states its own coverage: {text}"
    );
    let details = pack.details;
    assert!(details["coverage"]["sites"].is_number(), "{details}");
}

/// The review's §4: "Impact с UNKNOWN и lower bound ... Нулевой caller set не означает
/// безопасное удаление". The trap: a call site the resolver could not place has NO edge,
/// so a trace cannot see it — and "0 callers" then reads as "safe to delete".
#[tokio::test]
async fn impact_reports_the_callers_the_walk_could_not_follow() {
    let dir = tempfile::tempdir().unwrap();
    // `victim` exists; two files name it. One call resolves, one cannot be placed (the
    // callee is spelled through a path this resolver does not model).
    std::fs::write(
        dir.path().join("lib.rs"),
        "pub fn victim() -> u32 { 1 }\n\
             pub fn direct_caller() -> u32 { victim() }\n\
             pub fn opaque_caller(v: Vec<u32>) -> u32 { v.victim() }\n",
    )
    .unwrap();
    let tool = tool(dir.path());
    tool.dispatch(&serde_json::json!({ "action": "index" }))
        .unwrap();

    let result = tool
        .dispatch(&serde_json::json!({
            "action": "impact",
            "symbol": "victim",
        }))
        .unwrap();
    let text = text_of(&result);
    let details = result.details.clone();

    // The resolved caller is found.
    assert!(
        details["callers"].as_array().is_some_and(|c| !c.is_empty()),
        "the direct caller must be in the report: {details}"
    );
    // And the report states its own omission count rather than presenting a caller set
    // as complete.
    assert!(
        details["envelope"]["projectUnresolvedSites"].is_number(),
        "the envelope must carry the project's unresolved total: {details}"
    );
    assert!(
        text.contains("Impact of"),
        "the header names the symbol: {text}"
    );
}

/// The dangerous case end to end. A symbol with NO resolved caller but an unresolved
/// mention must say LOWER BOUND — that is the shape that reads as "safe to delete".
#[tokio::test]
async fn an_unresolved_mention_makes_an_empty_caller_set_a_lower_bound() {
    let dir = tempfile::tempdir().unwrap();
    // `lonely` is named by a call this resolver cannot place (`v.lonely()`), so no CALLS
    // edge exists — but the site IS recorded.
    std::fs::write(
        dir.path().join("lib.rs"),
        // TWO definitions named `lonely` in different modules, and a call site in the
        // THIRD module: no import binds it, so the resolver cannot choose and records a
        // miss instead of guessing. That is the invisible-caller shape.
        "pub mod a { pub fn lonely() -> u32 { 1 } }\n\
             pub mod b { pub fn lonely() -> u32 { 2 } }\n\
             pub fn opaque_caller() -> u32 { lonely() }\n",
    )
    .unwrap();
    let tool = tool(dir.path());
    tool.dispatch(&serde_json::json!({ "action": "index" }))
        .unwrap();
    let result = tool
        .dispatch(&serde_json::json!({
            "action": "impact",
            "symbol": "lonely",
        }))
        .unwrap();
    let details = result.details.clone();
    assert_eq!(
        details["envelope"]["unresolvedMentions"],
        serde_json::json!(1),
        "the fixture must HAVE an unvouched-for mention (a name guess counts), or the \
             test proves nothing: {details}"
    );
    let text = text_of(&result);
    // The candidate is NAMED with its reason, which is the property that matters: a
    // count alone tells a reader something was omitted but not what to check.
    assert!(
        text.contains("opaque_caller") && text.contains("name guess"),
        "the unvouched-for mention must be named with its reason: {text}"
    );
}

/// The other branch: no resolved caller and nothing unresolved either — the zero may be
/// stated, and even then it names the index-completeness bound.
#[tokio::test]
async fn a_conclusive_zero_names_the_index_bound() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("lib.rs"), "pub fn lonely() -> u32 { 1 }\n").unwrap();
    let tool = tool(dir.path());
    tool.dispatch(&serde_json::json!({ "action": "index" }))
        .unwrap();
    let result = tool
        .dispatch(&serde_json::json!({
            "action": "impact",
            "symbol": "lonely",
        }))
        .unwrap();
    let details = result.details.clone();
    assert_eq!(details["envelope"]["resolvedCallers"], serde_json::json!(0));
    assert_eq!(
        details["envelope"]["unresolvedMentions"],
        serde_json::json!(0)
    );
    let text = text_of(&result);
    assert!(
        text.contains("bounded by index completeness"),
        "a conclusive zero still names what it cannot see: {text}"
    );
}

/// A cancelled walk must SAY it was cancelled: "2 steps" from a stopped walk reads
/// as "2 neighbours", which is the wrong conclusion about the graph.
#[tokio::test]
async fn a_cancelled_trace_header_says_so() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
            dir.path().join("lib.rs"),
            "pub fn leaf() -> u32 { 1 }\npub fn mid() -> u32 { leaf() }\npub fn root() -> u32 { mid() }\n",
        )
        .unwrap();
    let tool = tool(dir.path());
    tool.dispatch(&serde_json::json!({ "action": "index" }))
        .unwrap();
    // A past deadline is the deterministic form: the clock is not raced.
    let result = tool
        .dispatch(&serde_json::json!({
            "action": "trace",
            "symbol": "root",
            "timeout_ms": 0,
        }))
        .unwrap();
    let text = text_of(&result);
    assert!(
        text.contains("STOPPED by cancellation"),
        "a stopped walk must say so: {text}"
    );
    assert!(
        text.contains("not the reachable set"),
        "and must not read as the whole answer: {text}"
    );
}

/// A short trace must say whether it stopped on the answer cap or on COST.
/// The two call for different next moves — a bigger `limit` versus a
/// narrower query — so "truncated" alone is not enough.
#[tokio::test]
async fn the_trace_header_names_the_budget_that_stopped_it() {
    use std::fmt::Write as _;
    let dir = tempfile::tempdir().unwrap();
    // A fan-out wide enough that a budget of 1 stops the walk immediately.
    let mut src = String::from("pub fn leaf() -> u32 { 1 }\n");
    for i in 0..12 {
        let _ = writeln!(src, "pub fn mid{i}() -> u32 {{ leaf() }}");
    }
    src.push_str("pub fn root() -> u32 {\n");
    for i in 0..12 {
        let _ = writeln!(src, "    mid{i}() +");
    }
    src.push_str("    0 }\n");
    std::fs::write(dir.path().join("lib.rs"), src).unwrap();
    let tool = tool(dir.path());
    tool.dispatch(&serde_json::json!({ "action": "index" }))
        .unwrap();

    let by_work = tool
        .dispatch(&serde_json::json!({
            "action": "trace",
            "symbol": "root",
            "work_budget": 1,
        }))
        .unwrap();
    let text = text_of(&by_work);
    assert!(
        text.contains("work budget"),
        "the header must name the work budget: {text}"
    );
    assert!(
        text.contains("COST, not on the graph's size"),
        "and say what that implies: {text}"
    );

    let by_limit = tool
        .dispatch(&serde_json::json!({
            "action": "trace",
            "symbol": "root",
            "limit": 1,
            "work_budget": 0,
        }))
        .unwrap();
    let text = text_of(&by_limit);
    assert!(
        text.contains("node limit"),
        "the step cap has its own wording: {text}"
    );
    assert!(
        !text.contains("work budget"),
        "and must not be confused with the work cap: {text}"
    );
}

#[tokio::test]
async fn a_zero_step_trace_says_unknown_not_none() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
            dir.path().join("lib.rs"),
            "pub fn leaf() -> u32 { 1 }\n             pub fn calls_leaf() -> u32 { leaf() }\n             pub fn orphan() -> u32 { 2 }\n",
        )
        .unwrap();
    let tool = tool(dir.path());
    tool.dispatch(&serde_json::json!({ "action": "index" }))
        .unwrap();
    let trace = tool
        .dispatch(&serde_json::json!({
            "action": "trace",
            "symbol": "orphan",
            "direction": "in",
        }))
        .unwrap();
    let text = text_of(&trace);
    assert!(
        text.contains("0 step(s)"),
        "the fixture has no caller of orphan: {text}"
    );
    assert!(
        text.to_lowercase().contains("unknown"),
        "a zero-step trace must carry the UNKNOWN caveat: {text}"
    );
    assert!(
        text.contains("lsp") || text.to_lowercase().contains("text search"),
        "the caveat must name the cross-check: {text}"
    );
    assert_eq!(
        trace.details.get("unknown"),
        Some(&serde_json::json!(true)),
        "the flag lets a caller branch on it without parsing text"
    );

    // A trace WITH steps stays neutral: the caveat must not ride answers
    // that actually resolved.
    let forward = tool
        .dispatch(&serde_json::json!({
            "action": "trace",
            "symbol": "calls_leaf",
            "direction": "out",
        }))
        .unwrap();
    assert!(
        !text_of(&forward).to_lowercase().contains("unknown"),
        "a resolved trace is not unknown: {}",
        text_of(&forward)
    );
}

#[tokio::test]
async fn trace_and_status_work_over_the_indexed_store() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("lib.rs"),
        "pub fn parse_config(path: &str) -> u32 { load(path) }\n\
             pub fn load(path: &str) -> u32 { path.len() as u32 }\n",
    )
    .unwrap();
    let tool = tool(dir.path());
    tool.dispatch(&serde_json::json!({ "action": "index" }))
        .unwrap();
    let trace = tool
        .dispatch(&serde_json::json!({ "action": "trace", "symbol": "parse_config" }))
        .unwrap();
    assert!(
        text_of(&trace).contains("parse_config"),
        "{}",
        text_of(&trace)
    );
    let status = tool
        .dispatch(&serde_json::json!({ "action": "status" }))
        .unwrap();
    assert!(text_of(&status).contains("node(s)"));
}

#[tokio::test]
async fn unknown_action_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let tool = tool(dir.path());
    let error = tool
        .dispatch(&serde_json::json!({ "action": "nope" }))
        .unwrap_err();
    assert!(
        error.to_string().contains("unknown graph action"),
        "{error}"
    );
}

/// A judge that answers the re-rank's `noul` questions from a table,
/// keyed by the symbol it sees in the question text.
struct SymbolJudge {
    relevant: Vec<&'static str>,
}

#[async_trait::async_trait]
impl crate::jev::Judge for SymbolJudge {
    fn label(&self) -> &'static str {
        "scripted/symbols"
    }

    async fn judge(
        &self,
        request: &pa_jev::JudgmentRequest,
    ) -> Result<pa_jev::JudgmentResult, crate::jev::JudgeError> {
        let mut answers = std::collections::BTreeMap::new();
        for (id, question) in &request.questions {
            let pa_jev::Question::Noul { instructions, .. } = question else {
                panic!("the re-rank asks noul questions");
            };
            // The question renders the symbol in quotes, so an exact
            // symbol match cannot hit `lib::load_balancer`.
            let relevant = self
                .relevant
                .iter()
                .any(|symbol| instructions.contains(&format!("{symbol:?}")));
            answers.insert(
                id.clone(),
                pa_jev::Answer::Noul {
                    noul: if relevant { 0.95 } else { 0.02 },
                },
            );
        }
        Ok(pa_jev::JudgmentResult {
            api: "scripted".to_string(),
            provider: "scripted".to_string(),
            model: "scripted/symbols".to_string(),
            answers,
            usage: pa_jev::Usage::default(),
        })
    }
}

/// A store with two symbols whose names both match `load`.
fn indexed_store(dir: &std::path::Path) -> GraphTool {
    std::fs::write(
        dir.join("lib.rs"),
        "pub fn load_balancer(path: &str) -> u32 { 0 }\n\
             pub fn load(path: &str) -> u32 { path.len() as u32 }\n",
    )
    .unwrap();
    let tool = tool(dir);
    tool.dispatch(&serde_json::json!({ "action": "index" }))
        .unwrap();
    tool
}

#[tokio::test]
async fn without_a_reranker_the_search_result_is_untouched() {
    let dir = tempfile::tempdir().unwrap();
    let tool = indexed_store(dir.path());
    let result = tool
        .dispatch(&serde_json::json!({ "action": "search", "query": "load" }))
        .unwrap();
    assert!(
        result
            .details
            .get(crate::jev::graph_rerank::VERIFIED_MARKER)
            .is_none(),
        "an unconfigured tool carries no marker"
    );
    assert!(!text_of(&result).contains("kept-by-jev"));
}

#[tokio::test]
async fn the_reranker_marks_survivors_and_demotes_confident_misses() {
    let dir = tempfile::tempdir().unwrap();
    let judge: std::sync::Arc<dyn crate::jev::Judge> = std::sync::Arc::new(SymbolJudge {
        relevant: vec!["lib::load"],
    });
    let tool = indexed_store(dir.path()).with_reranker(Some(std::sync::Arc::new(
        crate::jev::GraphReranker::new(judge),
    )));
    let query = serde_json::json!({ "action": "search", "query": "load" });
    // The re-rank runs on the async execute path (it awaits the judge).
    let result = pa_agent::types::AgentTool::execute(
        std::sync::Arc::new(tool),
        "call".to_string(),
        query,
        pa_agent::abort::AbortController::new().signal(),
        std::sync::Arc::new(|_| {}),
    )
    .await
    .expect("the tool answers");
    let marker = result
        .details
        .get(crate::jev::graph_rerank::VERIFIED_MARKER)
        .expect("a re-ranked result is marked");
    assert_eq!(marker["judged"].as_u64(), Some(2), "{marker}");
    let text = text_of(&result);
    assert!(text.contains("re-ranked by jev"), "{text}");
    assert!(text.contains("kept-by-jev"), "{text}");
    assert!(text.contains("demoted-by-jev"), "{text}");
    // A demoted candidate is kept and flagged, never silently deleted.
    let rows = result.details["results"].as_array().expect("rows");
    assert_eq!(rows.len(), 2);
    let kept: Vec<bool> = rows
        .iter()
        .map(|row| row["kept"].as_bool().unwrap_or_default())
        .collect();
    assert_eq!(kept, vec![true, false], "the survivor comes first");
    // The demoted row keeps its probability as evidence.
    assert!(
        rows[1]["relevance"].as_f64().is_some_and(|p| p < 0.1),
        "{:?}",
        rows[1]
    );
}

/// Review #8: the pool a model can ask for must be bounded, and the cast that feeds it
/// must never go negative. `pool as i64` on a saturated `usize` is negative, and SQLite
/// reads a negative LIMIT as "no limit" — the opposite of what a bound is for.
#[tokio::test]
async fn a_search_pool_is_clamped_and_never_negative() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("lib.rs"), "pub fn alpha() -> u32 { 1 }\n").unwrap();
    let tool = tool(dir.path());
    tool.dispatch(&serde_json::json!({ "action": "index" }))
        .unwrap();

    // A limit and multiplier far past the ceilings: both are reduced, and the reduction
    // is REPORTED rather than silently applied.
    let result = tool
        .dispatch(&serde_json::json!({
            "action": "search",
            "query": "alpha",
            "limit": 1_000_000u64,
            "candidate_multiplier": 1_000_000u64,
        }))
        .unwrap();
    let applied = &result.details["search"];
    let limit = applied["limit"]
        .as_u64()
        .expect("the applied limit is echoed");
    assert!(
        limit <= crate::tools::graph::args::SEARCH_MAX_LIMIT as u64,
        "the limit must be clamped to the ceiling, got {limit}: {applied}"
    );
    let multiplier = applied["candidate_multiplier"]
        .as_u64()
        .expect("the applied multiplier is echoed");
    assert!(
        multiplier.saturating_mul(limit) <= crate::tools::graph::args::SEARCH_MAX_POOL as u64,
        "and the pool it implies must fit the pool ceiling: {applied}"
    );
    assert_eq!(
        applied["candidate_multiplierClampedTo"].as_u64(),
        Some(multiplier),
        "a reduced request must SAY it was reduced: {applied}"
    );
}

/// Review (tools report, #3): `rerank: false` is the per-call opt-out.
///
/// The judge's order is one input among several; a caller who wants the
/// store's own RRF order — or simply does not want to spend a judgment on a
/// scoping query — had no way to say so for one call.
/// Review (tools report, finding 5): `status` states the jev gate's
/// effective resolution, the read-once rule, and the on≡auto equivalence.
///
/// The trap was repeated in the reporting session: `settings.json` was
/// edited, the agent claimed the change was live, a targeted grep proved
/// otherwise — twice. No surface said the gate is read at construction or
/// that `on` resolves to `auto`.
#[tokio::test]
async fn status_states_the_rerank_gate_and_the_read_once_rule() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("lib.rs"), "pub fn a() -> u32 { 1 }\n").unwrap();
    // The gate wired the way the engine wires `graphRerank: "on"` WITHOUT
    // a judge: raw spelling "on", resolved off.
    let tool = indexed_store(dir.path()).with_rerank_gate(Some("on".to_string()), false);
    let status = tool
        .dispatch(&serde_json::json!({ "action": "status" }))
        .unwrap();
    let text = text_of(&status);
    assert!(text.contains("Judge re-rank: off"), "{text}");
    assert!(
        text.contains("[jev] graphRerank = on"),
        "the raw spelling is echoed: {text}"
    );
    assert!(
        text.contains("resolves to auto"),
        "the on≡auto equivalence must be stated: {text}"
    );
    assert!(
        text.contains("NEXT session") || text.contains("NEXT"),
        "the read-once rule must be stated: {text}"
    );

    // An armed gate says so and names the per-call opt-out.
    let judge: std::sync::Arc<dyn crate::jev::Judge> =
        std::sync::Arc::new(SymbolJudge { relevant: vec![] });
    let armed = indexed_store(dir.path())
        .with_reranker(Some(std::sync::Arc::new(crate::jev::GraphReranker::new(
            judge,
        ))))
        .with_rerank_gate(Some("auto".to_string()), true);
    let status = armed
        .dispatch(&serde_json::json!({ "action": "status" }))
        .unwrap();
    let text = text_of(&status);
    assert!(text.contains("Judge re-rank: armed"), "{text}");
    assert!(text.contains("rerank: false"), "{text}");
}

#[tokio::test]
async fn rerank_false_skips_the_judge_for_one_call() {
    let dir = tempfile::tempdir().unwrap();
    let judge: std::sync::Arc<dyn crate::jev::Judge> = std::sync::Arc::new(SymbolJudge {
        relevant: vec!["lib::load"],
    });
    let tool = indexed_store(dir.path()).with_reranker(Some(std::sync::Arc::new(
        crate::jev::GraphReranker::new(judge),
    )));
    let result = pa_agent::types::AgentTool::execute(
        std::sync::Arc::new(tool),
        "call".to_string(),
        serde_json::json!({ "action": "search", "query": "load", "rerank": false }),
        pa_agent::abort::AbortController::new().signal(),
        std::sync::Arc::new(|_| {}),
    )
    .await
    .expect("the tool answers");
    assert!(
        result
            .details
            .get(crate::jev::graph_rerank::VERIFIED_MARKER)
            .is_none(),
        "the judge must not have run: {:?}",
        result
            .details
            .as_object()
            .map(|o| o.keys().collect::<Vec<_>>())
    );
    assert!(
        !text_of(&result).contains("kept-by-jev"),
        "no judgment labels: {}",
        text_of(&result)
    );
    // The rows are the store's own, still complete.
    let rows = result.details["results"].as_array().expect("rows");
    assert_eq!(rows.len(), 2, "the opt-out must not shrink the hit set");
}

/// A misspelt opt-out is an error, not a silent `false`.
#[tokio::test]
async fn a_non_boolean_rerank_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let tool = indexed_store(dir.path());
    let error = tool
        .dispatch(&serde_json::json!({
            "action": "search",
            "query": "load",
            "rerank": "false",
        }))
        .unwrap_err();
    assert!(error.to_string().contains("`rerank`"), "{error}");
}

#[tokio::test]
async fn an_aborted_search_never_waits_for_the_judgment() {
    struct HangingJudge;
    #[async_trait::async_trait]
    impl crate::jev::Judge for HangingJudge {
        fn label(&self) -> &'static str {
            "hanging"
        }
        async fn judge(
            &self,
            _request: &pa_jev::JudgmentRequest,
        ) -> Result<pa_jev::JudgmentResult, crate::jev::JudgeError> {
            std::future::pending().await
        }
    }
    let dir = tempfile::tempdir().unwrap();
    let judge: std::sync::Arc<dyn crate::jev::Judge> = std::sync::Arc::new(HangingJudge);
    let tool = indexed_store(dir.path()).with_reranker(Some(std::sync::Arc::new(
        crate::jev::GraphReranker::new(judge),
    )));
    let controller = pa_agent::abort::AbortController::new();
    controller.abort();
    // The aborted signal short-circuits the judgment: the store's result
    // returns instead of waiting on the pending judge.
    let result = pa_agent::types::AgentTool::execute(
        std::sync::Arc::new(tool),
        "call".to_string(),
        serde_json::json!({ "action": "search", "query": "load" }),
        controller.signal(),
        std::sync::Arc::new(|_| {}),
    )
    .await
    .expect("the search still answers");
    assert!(
        result
            .details
            .get(crate::jev::graph_rerank::VERIFIED_MARKER)
            .is_none(),
        "an aborted re-rank leaves no marker"
    );
    assert!(
        text_of(&result).contains("hit(s) for"),
        "{}",
        text_of(&result)
    );
}

#[tokio::test]
async fn a_failing_rerank_judge_leaves_the_search_result_untouched() {
    struct FailingJudge;
    #[async_trait::async_trait]
    impl crate::jev::Judge for FailingJudge {
        fn label(&self) -> &'static str {
            "failing"
        }
        async fn judge(
            &self,
            _request: &pa_jev::JudgmentRequest,
        ) -> Result<pa_jev::JudgmentResult, crate::jev::JudgeError> {
            Err(crate::jev::JudgeError::Candidate {
                identity: "failing".to_string(),
                message: "scripted failure".to_string(),
                status: None,
            })
        }
    }
    let dir = tempfile::tempdir().unwrap();
    let judge: std::sync::Arc<dyn crate::jev::Judge> = std::sync::Arc::new(FailingJudge);
    let tool = indexed_store(dir.path()).with_reranker(Some(std::sync::Arc::new(
        crate::jev::GraphReranker::new(judge),
    )));
    let result = pa_agent::types::AgentTool::execute(
        std::sync::Arc::new(tool),
        "call".to_string(),
        serde_json::json!({ "action": "search", "query": "load" }),
        pa_agent::abort::AbortController::new().signal(),
        std::sync::Arc::new(|_| {}),
    )
    .await
    .expect("fail-open: the store's own result still answers");
    assert!(
        result
            .details
            .get(crate::jev::graph_rerank::VERIFIED_MARKER)
            .is_none(),
        "a failed re-rank leaves no marker"
    );
    assert!(!text_of(&result).contains("kept-by-jev"));
}

#[test]
fn the_tool_exposes_its_name_and_schema() {
    let dir = tempfile::tempdir().unwrap();
    let tool = tool(dir.path());
    assert_eq!(tool.name(), "graph");
    assert_eq!(tool.label(), "Graph");
    assert_eq!(tool.parameters()["required"][0], "action");
}

/// Every action rejects a parameter it does not read, and the message
/// names both the offender and the accepted set.
///
/// Reading parameters with `.get(..)` means a misspelt key silently falls
/// back to the default: `GitNexus` #3261 is exactly that (a caller sent
/// `depth` instead of `maxDepth`, got the default and a 200), and here the
/// same shape was worse because `depth` meant two different things. A
/// caller cannot distinguish a typo from an empty result, so the tool must
/// say which key it did not understand.
#[tokio::test]
async fn an_unknown_parameter_is_an_error_naming_it() {
    for action in [
        "index",
        "reindex",
        "search",
        "trace",
        "architecture",
        "coverage",
        "changes",
        "status",
        "verify",
        "adr",
        "context",
        "impact",
        "explain_change",
    ] {
        let dir = tempfile::tempdir().unwrap();
        let tool = std::sync::Arc::new(tool(dir.path()));
        // `maxDepth` is the GitNexus spelling that started this: a plausible
        // name no action reads.
        let error = pa_agent::types::AgentTool::execute(
            tool,
            "call".to_string(),
            serde_json::json!({ "action": action, "maxDepth": 3 }),
            pa_agent::abort::AbortController::new().signal(),
            std::sync::Arc::new(|_| {}),
        )
        .await
        .expect_err("an unknown parameter must fail the call");
        let message = error.to_string();
        assert!(
            message.contains("unknown parameter \"maxDepth\""),
            "action {action:?} must name the offending key: {message}"
        );
        assert!(
            message.contains(&format!("for action {action:?}")),
            "the message must name the action: {message}"
        );
        assert!(
            message.contains("accepted are "),
            "the message must list the accepted keys: {message}"
        );
    }
}

/// The two depths are different parameters now, and the index one still
/// accepts its legacy spelling.
///
/// Both used to be `depth`: a string on `index` and a number on `trace`. A
/// caller that learned one shape and used it on the other action got a
/// silent default instead of a type error.
#[tokio::test]
async fn the_index_depth_and_the_trace_depth_are_different_parameters() {
    // Each case gets its own project: a skeleton pass replaces the graph, so
    // a trace after it would have no symbol left to start from.
    let skeleton_dir = tempfile::tempdir().unwrap();
    std::fs::write(
        skeleton_dir.path().join("lib.rs"),
        "pub fn one() -> u32 { 1 }\n",
    )
    .unwrap();
    // `index_depth` is the canonical name: a skeleton pass records one file
    // node and no symbol.
    let skeleton = run_action(
        skeleton_dir.path(),
        "index",
        serde_json::json!({ "index_depth": "skeleton" }),
    )
    .await
    .expect("index_depth is accepted");
    assert!(
        text_of(&skeleton).contains("1 nodes"),
        "a skeleton pass records one node: {}",
        text_of(&skeleton)
    );
    // The legacy spelling still works on `index`, and only there.
    let legacy_dir = tempfile::tempdir().unwrap();
    std::fs::write(
        legacy_dir.path().join("lib.rs"),
        "pub fn one() -> u32 { 1 }\n",
    )
    .unwrap();
    let legacy = run_action(
        legacy_dir.path(),
        "index",
        serde_json::json!({ "depth": "skeleton" }),
    )
    .await
    .expect("legacy depth is still accepted on index");
    assert!(
        text_of(&legacy).contains("1 nodes"),
        "the legacy spelling still means the build depth: {}",
        text_of(&legacy)
    );
    // On `trace`, `depth` is the BFS hop count (an integer).
    let trace_dir = tempfile::tempdir().unwrap();
    std::fs::write(
        trace_dir.path().join("lib.rs"),
        "pub fn one() -> u32 { two() }\npub fn two() -> u32 { 2 }\n",
    )
    .unwrap();
    run_action(trace_dir.path(), "index", serde_json::json!({}))
        .await
        .expect("index");
    let trace = run_action(
        trace_dir.path(),
        "trace",
        serde_json::json!({ "symbol": "one", "depth": 1 }),
    )
    .await
    .expect("trace reads depth as the BFS hop count");
    assert!(
        text_of(&trace).contains("Trace from"),
        "{}",
        text_of(&trace)
    );
    // Passing the index spelling to `trace` is now an error, not a default.
    let error = run_action(
        trace_dir.path(),
        "trace",
        serde_json::json!({ "index_depth": "full" }),
    )
    .await
    .expect_err("trace must reject the index spelling");
    assert!(
        error
            .to_string()
            .contains("unknown parameter \"index_depth\""),
        "{error}"
    );
}

/// The advertised schema names every parameter the actions read, under the
/// same spelling. It used to declare `depth` twice — once as the trace
/// integer, once as the index string — so a JSON object had one surviving
/// key and the model saw whichever came last.
#[test]
fn the_schema_names_the_parameters_the_actions_read() {
    let schema = params_schema();
    let properties = schema["properties"].as_object().expect("properties");
    for key in ["index_depth", "depth"] {
        assert!(
            properties.contains_key(key),
            "the schema must declare {key}"
        );
    }
    assert_eq!(
        properties["index_depth"]["type"], "string",
        "index_depth is the build depth",
    );
    assert_eq!(
        properties["depth"]["type"], "integer",
        "the remaining `depth` is the trace BFS hop count",
    );
    // Every schema property is read by some action; every canonical
    // parameter of some action is in the schema. Aliases (`root`, `refA`,
    // `refB`, the legacy `depth` on index) are deliberately schema-less.
    let aliases = ["root", "refA", "refB", "depth", "action"];
    let mut declared: Vec<&str> = properties.keys().map(String::as_str).collect();
    declared.sort_unstable();
    assert_eq!(
        declared,
        vec![
            "action",
            "candidate_multiplier",
            "confidence_floor",
            "consequences",
            "context",
            "decision",
            "depth",
            "direction",
            "edge_types",
            "extensions",
            "force",
            "id",
            "index_depth",
            "label",
            "lease_wait",
            "limit",
            "max_file_bytes",
            "mmr",
            "path",
            "query",
            "ref_a",
            "ref_b",
            "regex",
            "rerank",
            "respect_gitignore",
            "scope",
            "status",
            "supersedes",
            "symbol",
            "timeout_ms",
            "title",
            "view",
            "work_budget",
        ],
        "the schema drifted from what the actions read",
    );
    for (action, accepted) in ACCEPTED_PARAMS {
        for name in *accepted {
            assert!(
                aliases.contains(name) || properties.contains_key(*name),
                "action {action:?} reads {name:?}, which the schema does not declare",
            );
        }
    }
}

/// A tree with one visible source and two under a `.gitignore`, plus the
/// bare `.git` directory the `ignore` crate needs before it applies git's
/// rules at all.
fn ignored_fixture(dir: &std::path::Path) {
    std::fs::create_dir(dir.join(".git")).unwrap();
    std::fs::write(dir.join(".gitignore"), "scratch/\ngen/\n").unwrap();
    std::fs::write(dir.join("a.rs"), "pub fn tracked() -> u32 { 1 }\n").unwrap();
    std::fs::create_dir(dir.join("scratch")).unwrap();
    std::fs::write(dir.join("scratch/b.rs"), "pub fn scratch() -> u32 { 2 }\n").unwrap();
    std::fs::create_dir(dir.join("gen")).unwrap();
    std::fs::write(dir.join("gen/c.rs"), "pub fn generated() -> u32 { 3 }\n").unwrap();
}

/// The ignored set is the parameter's whole point, and both modes must be
/// reachable from the tool: `index` defaults to the LEAN view (the implicit
/// cold-start build always was lean; a wide explicit default re-admitted
/// vendored packages and gitignored state into the first graph — the tools
/// report measured 763 such files), `false` is the on-disk view, and
/// `details` says which mode ran so the counts are readable.
#[tokio::test]
async fn index_hides_ignored_files_unless_asked_to_index_them() {
    let lean_dir = tempfile::tempdir().unwrap();
    ignored_fixture(lean_dir.path());
    let lean = run_action(lean_dir.path(), "index", serde_json::json!({}))
        .await
        .expect("index");
    assert_eq!(
        lean.details["added"], 1,
        "the default is the lean view: {}",
        lean.details
    );
    assert_eq!(
        lean.details["respect_gitignore"], true,
        "the effective mode is echoed: {}",
        lean.details
    );
    let search = run_action(
        lean_dir.path(),
        "search",
        serde_json::json!({ "query": "generated" }),
    )
    .await
    .expect("search");
    assert!(
        text_of(&search).contains("0 hit(s)"),
        "an ignored symbol must not be in a lean graph: {}",
        text_of(&search)
    );

    let wide_dir = tempfile::tempdir().unwrap();
    ignored_fixture(wide_dir.path());
    let wide = run_action(
        wide_dir.path(),
        "index",
        serde_json::json!({ "respect_gitignore": false }),
    )
    .await
    .expect("index");
    assert_eq!(
        wide.details["added"], 3,
        "the explicit on-disk view indexes the ignored set: {}",
        wide.details
    );
    assert_eq!(wide.details["respect_gitignore"], false);
    assert!(
        text_of(&wide).contains("3 files parsed"),
        "{}",
        text_of(&wide)
    );
}

/// `reindex` shares `index`'s discovery, so the parameter decides its new-file
/// set too: a file that appears under an ignore rule is skipped in the
/// default (lean) mode and picked up by the explicit on-disk mode.
#[tokio::test]
async fn reindex_reads_the_same_ignore_mode() {
    let dir = tempfile::tempdir().unwrap();
    ignored_fixture(dir.path());
    run_action(dir.path(), "index", serde_json::json!({}))
        .await
        .expect("index");
    std::fs::write(
        dir.path().join("scratch/late.rs"),
        "pub fn late() -> u32 { 4 }\n",
    )
    .unwrap();

    let lean = run_action(dir.path(), "reindex", serde_json::json!({}))
        .await
        .expect("reindex");
    assert!(
        lean.details["reindexed_paths"]
            .as_array()
            .is_some_and(Vec::is_empty),
        "the default mode does not see the new ignored file: {}",
        lean.details
    );

    std::fs::write(
        dir.path().join("scratch/later.rs"),
        "pub fn later() -> u32 { 5 }\n",
    )
    .unwrap();
    let wide = run_action(
        dir.path(),
        "reindex",
        serde_json::json!({ "respect_gitignore": false }),
    )
    .await
    .expect("reindex");
    assert!(
        wide.details["reindexed_paths"]
            .as_array()
            .is_some_and(|paths| paths.iter().any(|p| p == "scratch/later.rs")),
        "the explicit on-disk mode finds the new ignored file: {}",
        wide.details
    );
    assert_eq!(wide.details["respect_gitignore"], false);
    // ... and it pruned nothing: both passes here are on-disk.
    assert_eq!(
        wide.details["removed"], 0,
        "the wide pass keeps every file it can see: {}",
        wide.details
    );
}

/// A value the mode cannot read is an error naming the type, not a silent
/// `false`: the two modes index different file sets, so guessing would hide
/// which one ran.
#[tokio::test]
async fn a_non_boolean_respect_gitignore_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    ignored_fixture(dir.path());
    let error = run_action(
        dir.path(),
        "index",
        serde_json::json!({ "respect_gitignore": "true" }),
    )
    .await
    .expect_err("a string is not a boolean");
    assert!(error.to_string().contains("`respect_gitignore`"), "{error}");
}

/// An implicit build is lean by construction: a cold query walks the tree
/// before anyone asked for a graph, so it must not index the ignored set (on a
/// real tree that set is generated output and copies of the same sources, so
/// every symbol would answer twice). The wide view stays one explicit `index`
/// away.
#[tokio::test]
async fn the_cold_start_build_is_lean() {
    let dir = tempfile::tempdir().unwrap();
    ignored_fixture(dir.path());
    let result = run_action(
        dir.path(),
        "search",
        serde_json::json!({ "query": "tracked" }),
    )
    .await
    .expect("cold search");
    assert!(
        text_of(&result).contains("indexed the project first"),
        "{}",
        text_of(&result)
    );
    let ignored = run_action(
        dir.path(),
        "search",
        serde_json::json!({ "query": "generated" }),
    )
    .await
    .expect("search");
    assert!(
        text_of(&ignored).contains("0 hit(s)"),
        "the cold build indexed an ignored file: {}",
        text_of(&ignored)
    );
}

/// A query on a cold graph builds the index first, so the first answer is
/// real and says so. This is the behavioural fix for the session where
/// `search`/`trace` on an unindexed project answered "0 hits" and the agent
/// concluded the graph had nothing to say, then read files one by one.
/// Run one graph action against a fresh sandbox project.
async fn run_action(
    dir: &std::path::Path,
    action: &str,
    extra: serde_json::Value,
) -> anyhow::Result<AgentToolResult> {
    let tool = std::sync::Arc::new(tool(dir));
    let mut params = serde_json::json!({ "action": action });
    if let (Some(obj), Some(extra_obj)) = (params.as_object_mut(), extra.as_object()) {
        for (k, v) in extra_obj {
            obj.insert(k.clone(), v.clone());
        }
    }
    pa_agent::types::AgentTool::execute(
        tool,
        "call".to_string(),
        params,
        pa_agent::abort::AbortController::new().signal(),
        std::sync::Arc::new(|_| {}),
    )
    .await
}

#[tokio::test]
async fn a_cold_graph_indexes_itself_on_the_first_query() {
    // A real mini-project: two files with a call edge the indexer can see.
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("src");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(
        src.join("a.rs"),
        "pub fn entry() -> u32 { helper() }\npub fn helper() -> u32 { 1 }\n",
    )
    .unwrap();

    let result = run_action(dir.path(), "search", serde_json::json!({"query": "helper"}))
        .await
        .expect("search on a cold graph must build and answer");
    let text = text_of(&result);
    assert!(
        text.contains("indexed the project first"),
        "the auto-index is named: {text}"
    );
    assert!(
        text.contains("1 hit(s)") || text.contains("2 hit(s)"),
        "the query answers with real hits, not 0: {text}"
    );
    // The tool stamps its own wall time so the run summary and the TUI card
    // can show it (fork tools used to report none at all).
    assert!(
        result
            .details
            .get("durationMs")
            .and_then(serde_json::Value::as_u64)
            .is_some(),
        "the graph result carries durationMs: {}",
        result.details
    );
}

/// A warm graph must NOT claim it indexed anything: the note follows the
/// build, not every query.
#[tokio::test]
async fn a_warm_graph_does_not_claim_an_index_build() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("src");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(src.join("a.rs"), "pub fn one() -> u32 { 1 }\n").unwrap();

    // Warm the graph through the public surface, then query it.
    run_action(dir.path(), "index", serde_json::json!({}))
        .await
        .expect("index");
    let result = run_action(dir.path(), "search", serde_json::json!({"query": "one"}))
        .await
        .expect("search");
    assert!(
        !text_of(&result).contains("indexed the project first"),
        "a warm graph must not claim a build: {}",
        text_of(&result)
    );
}

/// `architecture` used to put the whole report in `details` and leave the
/// model-visible `content` at a one-line stub — but providers only forward
/// `content`, so the model never saw the architecture data. The report
/// body must live in the content text itself.
/// `adr` is the `adr` module's first consumer: a decision and the code it
/// describes live in the same store, so they cannot drift apart.
#[tokio::test]
async fn adr_records_and_lists_decisions() {
    let dir = tempfile::tempdir().expect("tempdir");
    let saved = run_action(
        dir.path(),
        "adr",
        serde_json::json!({
            "title": "Use one SQLite file for graph and decisions",
            "decision": "Keep ADRs beside the graph",
            "status": "accepted"
        }),
    )
    .await
    .expect("record");
    let text = text_of(&saved).to_string();
    assert!(text.contains("1 record"), "expected one record: {text}");
    assert!(
        text.contains("Use one SQLite file for graph and decisions"),
        "the title must be listed: {text}"
    );
    assert!(text.contains("[accepted]"), "status must be shown: {text}");
    let listed = run_action(dir.path(), "adr", serde_json::json!({}))
        .await
        .expect("list");
    assert!(
        text_of(&listed).to_string().contains("1 record"),
        "the second call must not duplicate the record"
    );
}

/// The cascade's decision must reach the model: an identifier-shaped query
/// skips the signals tier, and the answer says so instead of looking narrow.
#[tokio::test]
async fn an_identifier_search_reports_the_plan_it_used() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(
        dir.path().join("a.rs"),
        "pub fn parse_config() {}\npub fn other() { parse_config(); }\n",
    )
    .expect("a.rs");
    let result = run_action(
        dir.path(),
        "search",
        serde_json::json!({"query": "parse_config"}),
    )
    .await
    .expect("search");
    let text = text_of(&result);
    assert!(
        text.contains("identifier query"),
        "the line must name the shape the plan was made for: {text}"
    );
    assert!(
        text.contains("signals(shape)"),
        "the skipped tier and its reason must be named: {text}"
    );
    let cascade = &result.details["cascade"];
    assert_eq!(cascade["shape"], "identifier");
    assert_eq!(cascade["running"][0], "name");
    assert!(
        cascade["skipped"].as_array().is_some_and(|a| !a.is_empty()),
        "details must carry the skip list: {}",
        result.details
    );
}

/// A fixture whose answers collide: four symbols share the words "parse
/// config", three of them with near-identical names, plus one that only
/// shares the docstring words. This is what MMR exists to spread out.
fn write_near_duplicates(dir: &std::path::Path) {
    std::fs::write(
        dir.join("a.rs"),
        "/// parse the config file\n\
             pub fn read_settings() -> u32 { 0 }\n\
             pub fn parse_config_file() -> u32 { 0 }\n\
             pub fn parse_config_path() -> u32 { 0 }\n\
             pub fn parse_config_dir() -> u32 { 0 }\n\
             pub fn load_config() -> u32 { 0 }\n",
    )
    .expect("a.rs");
}

/// Qualified names of the hits, in the order the tool returned them.
fn hit_names(result: &AgentToolResult) -> Vec<String> {
    result.details["results"]
        .as_array()
        .expect("results")
        .iter()
        .map(|row| {
            row["qualified_name"]
                .as_str()
                .expect("qualified_name")
                .to_string()
        })
        .collect()
}

/// `mmr` reaches the store's diversity reranking, and its absence does not.
///
/// Red-first: with the `mmr` read removed, every case below returns the
/// plain relevance order and the assertions on 0.5 and 1.0 fail. The
/// parameter is a *diversity* weight, the complement of the store's lambda,
/// so a larger value must reorder toward the distinct hit rather than away
/// from it.
#[tokio::test]
async fn mmr_diversifies_a_near_duplicate_result_set() {
    let plain_dir = tempfile::tempdir().expect("tempdir");
    write_near_duplicates(plain_dir.path());
    let plain = run_action(
        plain_dir.path(),
        "search",
        serde_json::json!({"query": "parse config file", "limit": 3}),
    )
    .await
    .expect("search");
    assert_eq!(
        hit_names(&plain),
        [
            "a::parse_config_file",
            "a::parse_config_path",
            "a::parse_config_dir",
        ],
        "without mmr the three near-duplicate names lead: {}",
        text_of(&plain)
    );

    let mid_dir = tempfile::tempdir().expect("tempdir");
    write_near_duplicates(mid_dir.path());
    let mid = run_action(
        mid_dir.path(),
        "search",
        serde_json::json!({"query": "parse config file", "limit": 3, "mmr": 0.5}),
    )
    .await
    .expect("search");
    assert_ne!(
        hit_names(&mid),
        hit_names(&plain),
        "mmr=0.5 must change the order on a near-duplicate set: {}",
        text_of(&mid)
    );
    assert_eq!(mid.details["search"]["mmr"], 0.5, "the weight is echoed");

    let full_dir = tempfile::tempdir().expect("tempdir");
    write_near_duplicates(full_dir.path());
    let full = run_action(
        full_dir.path(),
        "search",
        serde_json::json!({"query": "parse config file", "limit": 3, "mmr": 1.0}),
    )
    .await
    .expect("search");
    // The most distinct hit is the one that shares only the docstring
    // words; pure diversity must pull it into the top three.
    assert_eq!(
        hit_names(&full)[1],
        "a::read_settings",
        "mmr=1.0 must promote the distinct hit: {}",
        text_of(&full)
    );
    // mmr=0 is "no reranking", not "rerank at lambda 1": the order must be
    // the plain one, so a caller can ask for it explicitly without getting
    // a different answer.
    let zero_dir = tempfile::tempdir().expect("tempdir");
    write_near_duplicates(zero_dir.path());
    let zero = run_action(
        zero_dir.path(),
        "search",
        serde_json::json!({"query": "parse config file", "limit": 3, "mmr": 0.0}),
    )
    .await
    .expect("search");
    assert_eq!(
        hit_names(&zero),
        hit_names(&plain),
        "mmr=0 must mean no reranking: {}",
        text_of(&zero)
    );
}

/// An out-of-range `mmr` is refused with the range in the message.
///
/// The store clamps instead (`hybrid::mmr_rerank` calls `.clamp(0, 1)`), so
/// without the check the caller would get a silently different ranking and
/// never learn which value actually ran.
#[tokio::test]
async fn an_out_of_range_mmr_is_refused_naming_the_range() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_near_duplicates(dir.path());
    for value in [serde_json::json!(1.5), serde_json::json!(-0.1)] {
        let error = run_action(
            dir.path(),
            "search",
            serde_json::json!({"query": "parse config", "mmr": value}),
        )
        .await
        .expect_err("an out-of-range mmr must fail the call");
        let message = error.to_string();
        assert!(
            message.contains("`mmr` must be in [0, 1]"),
            "the message must name the range: {message}"
        );
        assert!(
            message.contains(&value.to_string()),
            "the message must name the rejected value {value}: {message}"
        );
    }
}

/// `label` restricts the result to the requested node kinds.
///
/// Red-first: with the `label` read removed, the struct and the functions
/// both come back and the one-hit assertion fails.
#[tokio::test]
async fn label_restricts_the_result_set() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(
        dir.path().join("a.rs"),
        "pub fn parse_config(path: &str) -> u32 { 0 }\n\
             pub struct Config { pub path: String }\n\
             pub fn config_path() -> u32 { 0 }\n",
    )
    .expect("a.rs");
    let all = run_action(dir.path(), "search", serde_json::json!({"query": "config"}))
        .await
        .expect("search");
    assert!(
        hit_names(&all).len() > 1,
        "the unfiltered query must see more than the struct: {}",
        text_of(&all)
    );
    let structs = run_action(
        dir.path(),
        "search",
        serde_json::json!({"query": "config", "label": "struct"}),
    )
    .await
    .expect("search");
    assert_eq!(
        hit_names(&structs),
        ["a::Config"],
        "label=struct must keep only the struct: {}",
        text_of(&structs)
    );
    assert_eq!(
        structs.details["search"]["labels"],
        serde_json::json!(["Struct"])
    );
    // Two labels, mixed case and loose spacing: the accepted spelling is
    // case-insensitive, and a second label widens the set back.
    let both = run_action(
        dir.path(),
        "search",
        serde_json::json!({"query": "config", "label": "Struct, function"}),
    )
    .await
    .expect("search");
    assert_eq!(
        hit_names(&both).len(),
        hit_names(&all).len(),
        "struct+function must cover every hit here: {}",
        text_of(&both)
    );
}

/// An unknown label is refused with the accepted spellings listed.
///
/// `Label::from_str_or_default` maps anything unknown to `Function`, so a
/// typo would quietly search functions; this error is what stops that.
#[tokio::test]
async fn an_unknown_label_is_refused_listing_the_accepted_ones() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_near_duplicates(dir.path());
    let error = run_action(
        dir.path(),
        "search",
        serde_json::json!({"query": "config", "label": "strct"}),
    )
    .await
    .expect_err("an unknown label must fail the call");
    let message = error.to_string();
    assert!(
        message.contains("unknown label \"strct\""),
        "the message must name the offending label: {message}"
    );
    for accepted in ["function", "method", "struct", "enum", "file"] {
        assert!(
            message.contains(accepted),
            "the message must list {accepted}: {message}"
        );
    }
}

/// An invalid `regex` is refused with the compiler's diagnosis, position
/// included, and a valid one filters the hits.
///
/// Red-first: with the `regex` read removed, the invalid pattern is never
/// compiled and the call succeeds, so the `expect_err` fails.
#[tokio::test]
async fn a_regex_filters_the_hits_and_an_invalid_one_is_refused() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(
        dir.path().join("a.rs"),
        "pub fn config_path() -> u32 { 0 }\n\
             pub fn load_config() -> u32 { 0 }\n",
    )
    .expect("a.rs");
    let filtered = run_action(
        dir.path(),
        "search",
        serde_json::json!({"query": "config", "regex": "^config_"}),
    )
    .await
    .expect("search");
    assert_eq!(
        hit_names(&filtered),
        ["a::config_path"],
        "the regex must filter the ranked candidates: {}",
        text_of(&filtered)
    );
    assert_eq!(
        filtered.details["search"]["regex"], "^config_",
        "the pattern is echoed"
    );
    let error = run_action(
        dir.path(),
        "search",
        serde_json::json!({"query": "config", "regex": "("}),
    )
    .await
    .expect_err("an invalid regex must fail the call");
    let message = error.to_string();
    assert!(
        message.contains("unclosed group"),
        "the compiler's own diagnosis must reach the caller: {message}"
    );
    assert!(
        message.contains('^'),
        "the position caret must reach the caller: {message}"
    );
}

/// `candidate_multiplier` is accepted and echoed, and a value below one is
/// refused: the store silently floors it (`max(1)`), so a caller that asked
/// for 0 would never learn the pool did not shrink.
#[tokio::test]
async fn the_candidate_multiplier_is_echoed_and_must_be_at_least_one() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_near_duplicates(dir.path());
    let result = run_action(
        dir.path(),
        "search",
        serde_json::json!({"query": "config", "candidate_multiplier": 9}),
    )
    .await
    .expect("search");
    assert_eq!(result.details["search"]["candidate_multiplier"], 9);
    let error = run_action(
        dir.path(),
        "search",
        serde_json::json!({"query": "config", "candidate_multiplier": 0}),
    )
    .await
    .expect_err("a zero multiplier must fail the call");
    assert!(
        error
            .to_string()
            .contains("`candidate_multiplier` must be an integer >= 1"),
        "{error}"
    );
}

/// A call that sets no new knob answers exactly as it did before the knobs
/// existed: the same order, the same scores, the same rendered lines, and no
/// `search` echo in `details`.
///
/// The baseline is the store itself: the tool's answer for a bare
/// `limit`-only call must equal `GraphStore::search_report` with default
/// options, which is the whole of what `action_search` used to do. Pinning
/// the store's live output, then the exact rendered text, keeps the
/// assertion honest without hard-coding BM25 constants an unrelated indexer
/// change would move.
///
/// The fixture has more hits than the limit on purpose: on a set that fits
/// the limit, MMR keeps every hit and a leaked default can hide.
///
/// Red-first: make any knob apply when it was not asked for (a bare
/// `opts = opts.with_mmr(0.5)`, or an unconditional echo) and this fails.
#[tokio::test]
async fn a_search_without_knobs_matches_the_store_defaults() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_near_duplicates(dir.path());
    let result = run_action(
        dir.path(),
        "search",
        serde_json::json!({"query": "parse config file", "limit": 3}),
    )
    .await
    .expect("search");
    assert!(
        result.details.get("search").is_none(),
        "no knob must mean no `details.search`: {}",
        result.details
    );
    // What the tool was supposed to do: the store's own defaults, nothing more.
    let store = GraphStore::open(dir.path().join("graph.db")).expect("open");
    let (hits, cascade) = store
        .search_report(
            "parse config file",
            &SearchOptions::new(project_name(dir.path())).with_limit(3),
        )
        .expect("store search");
    let expected_results: Vec<serde_json::Value> = hits
        .iter()
        .map(|hit| {
            serde_json::json!({
                "id": hit.id,
                "label": hit.label.as_str(),
                "name": hit.name,
                "qualified_name": hit.qualified_name,
                "file_path": hit.file_path,
                "start_line": hit.start_line,
                "end_line": hit.end_line,
                "score": hit.score,
                "in_degree": hit.in_degree,
                "out_degree": hit.out_degree,
            })
        })
        .collect();
    let mut details = result.details.clone();
    details
        .as_object_mut()
        .expect("details object")
        .remove("durationMs");
    assert_eq!(
        details,
        serde_json::json!({
            "hits": hits.len(),
            "results": expected_results,
            "cascade": {
                "shape": cascade.shape.as_str(),
                "running": cascade.running.iter().map(|t| t.as_str()).collect::<Vec<_>>(),
                "skipped": cascade
                    .skipped
                    .iter()
                    .map(|(tier, reason)| serde_json::json!({
                        "tier": tier.as_str(),
                        "reason": reason.as_str(),
                    }))
                    .collect::<Vec<_>>(),
                "cost": cascade.cost,
            },
        }),
        "the unknobbed answer drifted from the store's defaults: {}",
        text_of(&result)
    );
    // The rendered lines are the model's view of that answer and are part of
    // the contract too, so pin them exactly.
    assert_eq!(
        text_of(&result),
        "The graph was empty, so this call indexed the project first: 6 nodes, \
             5 edges, 1 files parsed. This build is lean: files the project's \
             gitignore rules exclude are not in it (run `index` to include them).\n\
             [cascade: phrase query, ran name+signals; skipped structure(shape),\
             community(shape)]\n\
             3 hit(s) for \"parse config file\":\n\
             - a::parse_config_file (a.rs:3) [Function] score=0.020\n\
             - a::parse_config_path (a.rs:4) [Function] score=0.020\n\
             - a::parse_config_dir (a.rs:5) [Function] score=0.020\n\
             [graph: fresh | 1 files]",
        "the rendered answer drifted"
    );
}

/// V1 (`fts5_camelcase`): a `camelCase` query reaches the `snake_case`
/// symbol, through the split-token column rather than by luck: the raw
/// query term does not occur in the stored name.
#[tokio::test]
async fn a_camel_case_query_finds_the_snake_case_symbol() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(
        dir.path().join("a.rs"),
        "pub fn parse_config(path: &str) -> u32 { 0 }\n\
             pub fn other() -> u32 { 0 }\n",
    )
    .expect("a.rs");
    let result = run_action(
        dir.path(),
        "search",
        serde_json::json!({"query": "parseConfig"}),
    )
    .await
    .expect("search");
    assert_eq!(
        hit_names(&result).first().map(String::as_str),
        Some("a::parse_config"),
        "a camelCase query must find the snake_case symbol: {}",
        text_of(&result)
    );
}

/// Write `count` one-file-per-symbol sources under `src/.`.
fn write_bulk_tree(dir: &std::path::Path, count: usize) {
    let src = dir.join("src");
    std::fs::create_dir_all(&src).unwrap();
    for n in 0..count {
        std::fs::write(
            src.join(format!("m{n}.rs")),
            format!(
                "pub fn symbol_{n}() -> u32 {{ helper_{n}() }}\n\
                     pub fn helper_{n}() -> u32 {{ {n} }}\n"
            ),
        )
        .unwrap();
    }
}

/// A read must not queue behind an `index`.
///
/// `index` holds the store's write lock for the whole build (the measured
/// full pass is 133 s); a `search` is 2 ms of SQLite. Red-first evidence:
/// with the read path taking the writer's lock this test observes the
/// search returning only after the build released it — i.e. it fails.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_search_answers_while_an_index_holds_the_writer() {
    let dir = tempfile::tempdir().unwrap();
    write_bulk_tree(dir.path(), 1);
    let tool = std::sync::Arc::new(tool(dir.path()));
    // Warm the graph: the first query would otherwise build it, and this
    // test is about what waits on what, not about the cold-start build.
    tool.dispatch(&serde_json::json!({ "action": "index" }))
        .expect("warm index");
    // Real work for the in-flight build to hold the lock for.
    write_bulk_tree(dir.path(), 300);

    let build_tool = std::sync::Arc::clone(&tool);
    let build = tokio::task::spawn_blocking(move || {
        let started = std::time::Instant::now();
        let report = build_tool.dispatch(&serde_json::json!({ "action": "index" }));
        (report, started.elapsed())
    });
    // Readiness: wait until a writer actually holds the store lock.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while tool.store.try_lock().is_ok() {
        assert!(
            std::time::Instant::now() < deadline,
            "the index build never took the writer lock"
        );
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }

    let started = std::time::Instant::now();
    let search = tool.dispatch(&serde_json::json!({ "action": "search", "query": "helper_7" }));
    let search_elapsed = started.elapsed();
    assert!(search.is_ok(), "search failed: {search:?}");
    // The state the fix is about: the search returned while the build still
    // held the writer. A `try_lock` that succeeds here means the search
    // came back only after the build released the lock — it queued.
    assert!(
        tool.store.try_lock().is_err(),
        "the index released the writer before the search returned, so the search \
             queued behind the build (it took {search_elapsed:?})"
    );
    // A backstop for the same defect on a machine where the build ends
    // sooner: measured here at 19 ms for the search against a 1.3 s build,
    // and 1.49 s when the search queues.
    assert!(
        search_elapsed < std::time::Duration::from_millis(500),
        "search took {search_elapsed:?} while an index was in flight; it queued behind \
             the build instead of using the reader"
    );
    let (report, build_elapsed) = build.await.unwrap();
    assert!(report.is_ok(), "index failed: {report:?}");
    assert!(
        build_elapsed > search_elapsed,
        "the fixture was too small to distinguish the two paths: index {build_elapsed:?}, \
             search {search_elapsed:?}"
    );
}

/// A read issued during the *first* build must not answer from the store
/// that build is still filling.
///
/// This is the false negative the cold-start index exists to prevent: a
/// search reading an uncommitted store answers "0 hits" for everything, and
/// the agent concludes the graph has nothing to say. A read during a build
/// therefore waits on the writer, and only a read of a *populated* store
/// goes to the reader.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_search_during_the_first_build_does_not_answer_zero_hits() {
    let dir = tempfile::tempdir().unwrap();
    write_bulk_tree(dir.path(), 400);
    let tool = std::sync::Arc::new(tool(dir.path()));

    // The first search performs the cold-start build itself.
    let cold_tool = std::sync::Arc::clone(&tool);
    let cold = tokio::task::spawn_blocking(move || {
        cold_tool.dispatch(&serde_json::json!({ "action": "search", "query": "helper" }))
    });
    // Readiness: the build is running, observed on the flag that gates the
    // read path, not on a timing guess.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while !tool.in_flight.load(std::sync::atomic::Ordering::Acquire) {
        assert!(
            std::time::Instant::now() < deadline,
            "the cold-start build never began"
        );
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }

    let search = tool
        .dispatch(&serde_json::json!({ "action": "search", "query": "helper" }))
        .expect("search during the first build");
    // `details.hits`, not the text: "20 hit(s)" contains "0 hit(s)".
    let hits = search
        .details
        .get("hits")
        .and_then(serde_json::Value::as_u64);
    assert!(
        hits.is_some_and(|hits| hits > 0),
        "a search during the first build read the store mid-fill: {}",
        text_of(&search)
    );
    let cold = cold.await.unwrap().expect("the cold search completes");
    assert!(
        cold.details
            .get("hits")
            .and_then(serde_json::Value::as_u64)
            .is_some_and(|hits| hits > 0),
        "the cold search answers with real hits: {}",
        text_of(&cold)
    );
}

#[tokio::test]
async fn architecture_report_lives_in_content_not_details() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("src");
    std::fs::create_dir_all(&src).unwrap();
    // a.rs depends on b.rs (a cross-file call edge), so `layers` has two
    // real depth buckets and `structure` has one real module edge.
    std::fs::write(
        src.join("a.rs"),
        "pub fn entry() -> u32 { helper() }
",
    )
    .unwrap();
    std::fs::write(
        src.join("b.rs"),
        "pub fn helper() -> u32 { 41 }
",
    )
    .unwrap();

    let layers = run_action(
        dir.path(),
        "architecture",
        serde_json::json!({"view": "layers"}),
    )
    .await
    .expect("architecture answers");
    let text = text_of(&layers);
    assert!(text.contains("Architecture view `layers`"), "{text}");
    assert!(
        text.contains("layer 0") && text.contains("layer 1"),
        "the depth buckets are in the content, not a stub: {text}"
    );
    assert!(text.contains("b.rs"), "{text}");
    assert!(text.contains("a.rs"), "the dependent file is named: {text}");
    assert!(
        !text.contains("see details"),
        "the old stub pointed the model at invisible details: {text}"
    );
    // `details` keeps the structured rows for tests and the UI.
    assert!(
        layers.details["layers"]
            .as_array()
            .is_some_and(|rows| !rows.is_empty()),
        "{}",
        layers.details
    );

    let structure = run_action(
        dir.path(),
        "architecture",
        serde_json::json!({"view": "structure"}),
    )
    .await
    .expect("architecture answers");
    let text = text_of(&structure);
    assert!(
        text.contains("module edge(s)") && text.contains("->"),
        "the structure edges are in the content: {text}"
    );
    assert!(text.contains("b.rs"), "{text}");

    let overview = run_action(dir.path(), "architecture", serde_json::json!({}))
        .await
        .expect("architecture answers");
    let text = text_of(&overview);
    assert!(
        text.contains("node(s)") && text.contains("labels ["),
        "the census is in the content: {text}"
    );
    assert!(
        !text.contains("see details"),
        "no stub for the default view either: {text}"
    );
}

/// `confidence_floor` filters the trace, echoes its value, and refuses a
/// value outside `[0, 1]` naming the range.
///
/// Red-first: with the `confidence_floor` read removed from `action_trace`,
/// both steps come back, `details.confidence_floor` is absent and the
/// "filtered out" line never appears — every assertion below fails.
#[tokio::test]
async fn the_confidence_floor_filters_steps_and_refuses_a_value_outside_the_range() {
    let dir = tempfile::tempdir().expect("tempdir");
    // Each callee name is unique in the project, so both edges resolve at the
    // `unique` confidence 0.75: `b` lands at 0.75 and `c` at 0.5625, and a
    // floor of 0.6 splits them.
    std::fs::write(
        dir.path().join("lib.rs"),
        "pub fn a() -> u32 { b() }\npub fn b() -> u32 { c() }\npub fn c() -> u32 { 1 }\n",
    )
    .expect("lib.rs");
    let plain = run_action(
        dir.path(),
        "trace",
        serde_json::json!({"symbol": "a", "depth": 2}),
    )
    .await
    .expect("trace");
    assert_eq!(
        plain.details["steps"].as_array().map(Vec::len),
        Some(2),
        "no floor keeps every step: {}",
        text_of(&plain)
    );

    let floored = run_action(
        dir.path(),
        "trace",
        serde_json::json!({"symbol": "a", "depth": 2, "confidence_floor": 0.6}),
    )
    .await
    .expect("a floor in range is accepted");
    let names: Vec<&str> = floored.details["steps"]
        .as_array()
        .expect("steps")
        .iter()
        .map(|step| step["name"].as_str().expect("name"))
        .collect();
    assert_eq!(names, vec!["b"], "c at 0.5625 is below 0.6: {names:?}");
    assert_eq!(floored.details["confidence_floor"], 0.6);
    assert_eq!(floored.details["filtered"], 1);
    let text = text_of(&floored);
    assert!(
        text.contains("below the confidence floor were filtered out"),
        "a short listing must say why it is short: {text}"
    );
    assert!(
        text.contains("1 step(s)"),
        "the header counts the kept steps: {text}"
    );

    // Out of range: an error, not a clamp, and it names the range.
    for bad in [serde_json::json!(1.5), serde_json::json!(-0.1)] {
        let error = run_action(
            dir.path(),
            "trace",
            serde_json::json!({"symbol": "a", "confidence_floor": bad}),
        )
        .await
        .expect_err("an out-of-range floor must be refused");
        let message = error.to_string();
        assert!(
            message.contains("`confidence_floor` must be in [0, 1]") && message.contains("got"),
            "the error names the range and the value: {message}"
        );
    }
}

/// `lease_wait` reaches the writer lease: `0` means one attempt, so a second
/// index during a build fails instead of waiting it out.
///
/// Red-first: with the `lease_wait` read removed, the default 300 s wait
/// applies, so the contended call below returns after the build released the
/// lease — `expect_err` panics.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_lease_wait_is_echoed_and_zero_means_one_attempt() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_bulk_tree(dir.path(), 1);
    // Two tools on one database, as two sessions would be. Both are warmed
    // before the build so the contended call measures the lease, not a cold
    // open.
    let builder = std::sync::Arc::new(tool(dir.path()));
    let waiter = std::sync::Arc::new(tool(dir.path()));
    for warming in [&builder, &waiter] {
        warming
            .dispatch(&serde_json::json!({ "action": "index" }))
            .expect("warm index");
    }
    write_bulk_tree(dir.path(), 400);

    let build_tool = std::sync::Arc::clone(&builder);
    let build = tokio::task::spawn_blocking(move || {
        build_tool.dispatch(&serde_json::json!({ "action": "index" }))
    });
    // Readiness on the lease itself, not a sleep: this probe only fails once
    // another holder owns the lock file, so it cannot pass early.
    let db = dir.path().join("graph.db");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    while pa_graph::lease::IndexLease::acquire(
        &db,
        std::time::Duration::ZERO,
        &pa_graph::lease::CancelToken::new(),
    )
    .is_ok()
    {
        assert!(
            std::time::Instant::now() < deadline,
            "the build never took the writer lease"
        );
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }

    let started = std::time::Instant::now();
    let contended = waiter.dispatch(&serde_json::json!({"action": "index", "lease_wait": 0}));
    let elapsed = started.elapsed();
    let error = contended.expect_err("lease_wait 0 must not wait for the build");
    let message = error.to_string();
    assert!(
        message.contains("index writer lease") && message.contains("timed out"),
        "the failure is the lease timeout: {message}"
    );
    assert!(
        elapsed < std::time::Duration::from_secs(5),
        "lease_wait 0 took {elapsed:?}: it waited instead of trying once"
    );
    assert!(
        !build.is_finished(),
        "the fixture was too small: the build ended before the contended call"
    );
    build
        .await
        .expect("the build task joins")
        .expect("the build itself succeeds");

    // And the value is echoed once the lease is free.
    let after = run_action(dir.path(), "index", serde_json::json!({"lease_wait": 0}))
        .await
        .expect("an uncontended index with lease_wait 0");
    assert_eq!(
        after.details["lease_wait"], 0,
        "the wait is echoed in milliseconds"
    );
}

/// `extensions` restricts the parsed set, and a requested extension that
/// matched nothing is named rather than reading as an empty codebase.
///
/// Red-first: with the `with_extensions` call removed, the Python file is
/// indexed and both the file-set and the unmatched-list assertions fail.
#[tokio::test]
async fn the_extension_allowlist_restricts_the_set_and_names_the_unmatched() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(
        dir.path().join("a.rs"),
        "pub fn rust_symbol() -> u32 { 1 }\n",
    )
    .expect("a.rs");
    std::fs::write(
        dir.path().join("b.py"),
        "def python_symbol():\n    return 1\n",
    )
    .expect("b.py");

    // `tsx` matches nothing here: not an error, but it must be named.
    let scoped = run_action(
        dir.path(),
        "index",
        serde_json::json!({"extensions": ".RS, tsx"}),
    )
    .await
    .expect("an unknown extension must not error");
    assert_eq!(
        scoped.details["extensions"],
        serde_json::json!(["rs", "tsx"]),
        "the echo is normalised: {}",
        scoped.details
    );
    assert_eq!(
        scoped.details["extensions_unmatched"],
        serde_json::json!(["tsx"]),
        "the extension that matched nothing is reported: {}",
        scoped.details
    );
    let text = text_of(&scoped);
    assert!(
        text.contains("tsx") && text.contains("matched no indexed file"),
        "the model reads why the set is narrow: {text}"
    );

    let store = GraphStore::open(dir.path().join("graph.db")).expect("open");
    let mut files: Vec<String> = store
        .all_file_hashes(&project_name(dir.path()))
        .expect("hashes")
        .into_keys()
        .collect();
    files.sort();
    assert_eq!(files, vec!["a.rs"], "only the allowed extension is parsed");

    // A second scoped run whose extension exists reports nothing unmatched.
    let matched = run_action(dir.path(), "index", serde_json::json!({"extensions": "rs"}))
        .await
        .expect("index");
    assert_eq!(
        matched.details["extensions_unmatched"],
        serde_json::json!([]),
        "{}",
        matched.details
    );
    assert!(
        !text_of(&matched).contains("matched no indexed file"),
        "a satisfied allowlist says nothing extra: {}",
        text_of(&matched)
    );

    // `reindex` shares discovery and the writer lease, so it reads the same
    // three knobs: the echo proves the values reached its run, not just the
    // schema.
    let reindexed = run_action(
        dir.path(),
        "reindex",
        serde_json::json!({"extensions": "rs", "lease_wait": 0, "max_file_bytes": 4096}),
    )
    .await
    .expect("reindex reads the same scope");
    assert_eq!(reindexed.details["extensions"], serde_json::json!(["rs"]));
    assert_eq!(reindexed.details["lease_wait"], 0);
    assert_eq!(reindexed.details["max_file_bytes"], 4096);
}

/// `max_file_bytes` excludes an oversized file, and the skip is visible.
///
/// Red-first: with the `with_max_file_bytes` call removed, `big.rs` is
/// indexed, so both the file-set and the coverage assertions fail.
#[tokio::test]
async fn the_max_file_bytes_excludes_an_oversized_file_and_the_skip_is_visible() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("a.rs"), "pub fn small() -> u32 { 1 }\n").expect("a.rs");
    let padding = "// padding padding padding padding padding padding padding padding\n";
    std::fs::write(
        dir.path().join("big.rs"),
        format!("{padding}{padding}pub fn big() -> u32 {{ 2 }}\n"),
    )
    .expect("big.rs");

    let scoped = run_action(
        dir.path(),
        "index",
        serde_json::json!({"max_file_bytes": 64}),
    )
    .await
    .expect("a size cap is accepted");
    assert_eq!(scoped.details["max_file_bytes"], 64);

    let store = GraphStore::open(dir.path().join("graph.db")).expect("open");
    let project = project_name(dir.path());
    let mut indexed: Vec<String> = store
        .all_file_hashes(&project)
        .expect("hashes")
        .into_keys()
        .collect();
    indexed.sort();
    assert_eq!(indexed, vec!["a.rs"], "the oversized file is not indexed");

    // The skip is in the report's own numbers and names its reason.
    let report = &scoped.details;
    assert_eq!(report["skipped"], 1, "{report}");
    let notes = store.coverage_rows(&project).expect("coverage rows");
    let big = notes
        .iter()
        .find(|(rel, _, _)| rel == "big.rs")
        .unwrap_or_else(|| panic!("no coverage note for big.rs: {notes:?}"));
    assert_eq!(big.1, "skipped");
    assert!(
        big.2.starts_with("oversized: ") && big.2.ends_with(" bytes"),
        "the note says why: {big:?}"
    );
}

/// With none of the four knobs set, `details` carries no new keys and the
/// answer keeps its old shape.
///
/// This is the compatibility half: the four parameters are additions, so a
/// call that uses none of them must be indistinguishable from one made
/// before they existed.
#[tokio::test]
async fn a_run_with_no_scope_knob_carries_no_new_keys() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(
        dir.path().join("lib.rs"),
        "pub fn one() -> u32 { two() }\npub fn two() -> u32 { 2 }\n",
    )
    .expect("lib.rs");
    let index = run_action(dir.path(), "index", serde_json::json!({}))
        .await
        .expect("index");
    let mut keys: Vec<&str> = index
        .details
        .as_object()
        .expect("details object")
        .keys()
        .map(String::as_str)
        .filter(|key| *key != "durationMs")
        .collect();
    keys.sort_unstable();
    // The report's own fields, plus the ignore mode (there since that parameter
    // landed) -- and nothing else.
    let default_report = serde_json::to_value(IndexReport::default()).expect("report json");
    let mut expected: Vec<&str> = default_report
        .as_object()
        .expect("report object")
        .keys()
        .map(String::as_str)
        .collect();
    expected.push("respect_gitignore");
    expected.sort_unstable();
    assert_eq!(keys, expected, "an unscoped index grew new details keys");
    assert!(!text_of(&index).contains("matched no indexed file"));

    let trace = run_action(
        dir.path(),
        "trace",
        serde_json::json!({"symbol": "one", "depth": 1}),
    )
    .await
    .expect("trace");
    let mut trace_keys: Vec<&str> = trace
        .details
        .as_object()
        .expect("details object")
        .keys()
        .map(String::as_str)
        .collect();
    trace_keys.sort_unstable();
    assert_eq!(
        trace_keys,
        vec!["durationMs", "origin", "origin_id", "steps", "truncated"],
        "an unfiltered trace grew new details keys: {}",
        trace.details
    );
}
/// §4.1 (audit): the project key was the root's basename, so two checkouts
/// named `service` shared one row set — and the WARM read path answered from
/// `file_count > 0` alone, so a query for B silently read A's symbols. The
/// root is validated before any read, and §8 (below) removes the collision
/// itself: the second tree gets its own qualified key instead of being
/// refused the database for the rest of its life.
///
/// Both halves are asserted here: B is indexable in a database A already
/// owns, AND B's answer never carries A's symbols.
#[tokio::test]
async fn a_second_root_with_the_same_basename_gets_its_own_rows() {
    let a = tempfile::tempdir().unwrap().path().join("service");
    let b = tempfile::tempdir().unwrap().path().join("service");
    std::fs::create_dir_all(&a).unwrap();
    std::fs::create_dir_all(&b).unwrap();
    std::fs::write(a.join("lib.rs"), "pub fn only_in_a() -> u32 { 1 }\n").unwrap();
    std::fs::write(b.join("lib.rs"), "pub fn only_in_b() -> u32 { 2 }\n").unwrap();

    // One shared DB, as the default `graph.dbPath` gives.
    let db = tempfile::tempdir().unwrap();
    let db_path = db.path().join("graph.db");
    let tool_a = GraphTool::new(db_path.clone(), a);
    tool_a
        .dispatch(&serde_json::json!({ "action": "index" }))
        .unwrap();

    // B is the same basename, a different root: it indexes under its own
    // qualified key instead of being refused the shared database.
    let tool_b = GraphTool::new(db_path.clone(), b.clone());
    tool_b
        .dispatch(&serde_json::json!({ "action": "index" }))
        .expect("a second tree with the same basename must be indexable");

    let answer_b = tool_b
        .dispatch(&serde_json::json!({ "action": "search", "query": "only_in_b" }))
        .expect("B answers from its own rows");
    assert!(
        text_of(&answer_b).contains("only_in_b"),
        "B's own symbol must be found: {}",
        text_of(&answer_b)
    );
    let foreign = tool_b
        .dispatch(&serde_json::json!({ "action": "search", "query": "only_in_a" }))
        .expect("the query itself is well formed");
    // The reply ECHOES the query (`1 hit(s) for "only_in_a"`), so the oracle
    // reads the hit names: a cascade answers something even for a miss
    // (measured: it returned B's own `only_in_b`), and only the symbol can
    // distinguish an answer from an echo.
    // The key is `results` (the tool's own details shape). Reading `hits`
    // returned an empty vec for ever, so this negative assertion could never
    // fail: the oracle passed on the broken code it was written to catch
    // (review 4.4). Assert the key first, so a rename fails loudly here.
    let hits = foreign
        .details
        .get("results")
        .and_then(serde_json::Value::as_array)
        .cloned()
        .expect("the search details carry a `results` array");
    assert!(
        !hits.iter().any(|hit| {
            hit.get("name").and_then(serde_json::Value::as_str) == Some("only_in_a")
        }),
        "no symbol from the other tree may appear as a hit: {}",
        text_of(&foreign)
    );

    // A still answers from its own rows.
    let answer_a = tool_a
        .dispatch(&serde_json::json!({ "action": "search", "query": "only_in_a" }))
        .expect("A answers from its own rows");
    assert!(text_of(&answer_a).contains("only_in_a"));

    // The qualified key is stable: a second tool over B computes the same
    // project and sees the rows the first one wrote.
    let tool_b_again = GraphTool::new(db_path, b);
    let again = tool_b_again
        .dispatch(&serde_json::json!({ "action": "search", "query": "only_in_b" }))
        .expect("the same root resolves to the same project");
    assert!(text_of(&again).contains("only_in_b"));
}
/// The review's check #7: "Superseded ADR ... не становятся подтверждёнными dependency
/// facts." A decision that a newer one replaced must SAY so in the same breath, or a
/// reader acts on a decision that no longer holds. The record already printed what it
/// supersedes; nothing printed what replaced IT.
/// The review's §2, ADR's next step: "Связать ADR с code/evidence и контекстом". An ADR
/// names files in prose and nothing read them, so "which decision governs this file"
/// required grep. The `indexed` half matters most: a path the ADR names but the tree does
/// not have is drift, and it is only visible against the index.
#[test]
fn prose_references_are_extracted_and_checked() {
    // The exact shapes an ADR uses: a backticked path with a line, a bare name, a path
    // with a colon in it, and prose that must NOT be extracted.
    let text = "See `crates/pa-core/src/tools/graph.rs:120` and graph.rs, plus \
                    settings/types.rs and the word done.rs already. Also a/b:c.rs stays.";
    let refs = pa_graph::adr::code_refs_in(text);
    assert!(
        refs.contains(&"crates/pa-core/src/tools/graph.rs".to_string()),
        "a backticked path with a line suffix must resolve to the path: {refs:?}"
    );
    assert!(
        refs.contains(&"graph.rs".to_string()),
        "a bare file name in prose counts, since that is how prose refers to a file: {refs:?}"
    );
    assert!(
        refs.contains(&"settings/types.rs".to_string()),
        "and a relative path: {refs:?}"
    );
    assert!(
        !refs.iter().any(|r| r.contains(":120")),
        "the line suffix must be stripped: {refs:?}"
    );
}

#[tokio::test]
async fn an_adr_names_the_code_it_governs_and_the_drift() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("lib.rs"), "pub fn x() -> u32 { 1 }\n").unwrap();
    let tool = tool(dir.path());
    tool.dispatch(&serde_json::json!({ "action": "index" }))
        .unwrap();
    let project = tool.project_key(dir.path());
    let adr = pa_graph::adr::Adr {
        id: 0,
        title: "Govern lib.rs".to_string(),
        status: pa_graph::adr::AdrStatus::Accepted,
        context: "lib.rs holds x. A file named gone.rs was removed.".to_string(),
        decision: "Keep lib.rs the single owner.".to_string(),
        consequences: "gone.rs no longer exists.".to_string(),
        supersedes: None,
        project,
    };
    let id = tool
        .with_read(|store| {
            pa_graph::adr::AdrStore::new(store.connection())
                .map_err(|err| anyhow::anyhow!("adr store: {err}"))?
                .record_adr(&adr)
                .map_err(|err| anyhow::anyhow!("record: {err}"))
        })
        .unwrap();
    let result = tool
        .dispatch(&serde_json::json!({ "action": "adr", "id": id }))
        .unwrap();
    let text = text_of(&result);
    assert!(
        text.contains("Code this decision names:"),
        "an ADR that names files must list them: {text}"
    );
    assert!(
        text.contains("lib.rs") && text.contains("symbol"),
        "an indexed file must report its symbols: {text}"
    );
    assert!(
        text.contains("gone.rs") && text.contains("NOT IN THE TREE"),
        "a named file the tree does not have is DRIFT and must say so: {text}"
    );
}

/// The review's §9 lists `graph.explain_change` as one task-oriented action: "затронутые
/// symbols, witness paths, relevant ADR/tests, unresolved areas". Every part existed
/// separately; nothing composed them, so the answer required three calls and a manual
/// merge.
#[tokio::test]
async fn explain_change_answers_all_four_parts_in_one_call() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("lib.rs"),
        "pub fn changed() -> u32 { 1 }\n\
             pub fn caller() -> u32 { changed() }\n",
    )
    .unwrap();
    std::fs::create_dir_all(dir.path().join("tests")).unwrap();
    std::fs::write(
        dir.path().join("tests/check.rs"),
        "pub fn checks_it() -> u32 { crate::changed() }\n",
    )
    .unwrap();
    let tool = tool(dir.path());
    tool.dispatch(&serde_json::json!({ "action": "index" }))
        .unwrap();
    let project = tool.project_key(dir.path());
    tool.with_read(|store| {
        pa_graph::adr::AdrStore::new(store.connection())
            .map_err(|err| anyhow::anyhow!("adr store: {err}"))?
            .record_adr(&pa_graph::adr::Adr {
                id: 0,
                title: "lib.rs owns the counter".to_string(),
                status: pa_graph::adr::AdrStatus::Accepted,
                context: "`lib.rs` holds the only counter.".to_string(),
                decision: "Keep it there.".to_string(),
                consequences: "Callers depend on lib.rs.".to_string(),
                supersedes: None,
                project: project.clone(),
            })
            .map_err(|err| anyhow::anyhow!("record: {err}"))
    })
    .unwrap();

    let result = tool
        .dispatch(&serde_json::json!({
            "action": "explain_change",
            "symbol": "changed",
            "depth": 3,
        }))
        .unwrap();
    let details = result.details.clone();

    // 1. affected symbols, each with its witness path
    let callers = details["impact"]["callers"].as_array().expect("callers");
    assert!(
        callers.iter().any(|c| c["qualifiedName"]
            .as_str()
            .is_some_and(|qn| qn.ends_with("caller"))),
        "the affected caller must be listed: {details}"
    );
    assert!(
        callers
            .iter()
            .any(|c| c["witness"].as_array().is_some_and(|w| w.len() >= 2)),
        "and each caller must carry its witness path: {details}"
    );

    // 2. relevant decision, matched by the CHANGED FILE
    let decisions = details["decisions"].as_array().expect("decisions");
    assert!(
        decisions
            .iter()
            .any(|d| d["title"] == serde_json::json!("lib.rs owns the counter")),
        "a decision naming the changed file must be listed: {details}"
    );

    // 3. test files, by path convention (the graph has no test label)
    let tests = details["tests"].as_array().expect("tests");
    assert!(
        tests
            .iter()
            .any(|t| t.as_str().is_some_and(|s| s.contains("check.rs"))),
        "a calling file under tests/ must be named: {details}"
    );

    // 4. what is NOT covered, stated rather than implied
    let gaps = details["notCovered"].as_array().expect("notCovered");
    assert!(
        gaps.iter()
            .any(|g| g.as_str().is_some_and(|s| s.contains("PATH CONVENTION"))),
        "the test-set's own weakness must be stated: {details}"
    );
    assert!(
        text_of(&result).contains("Not covered:"),
        "and printed, not only in JSON: {}",
        text_of(&result)
    );
}

/// The review's §2, `cycles`' next step: "объяснение исходными связями". A cycle printed
/// as files says a dependency exists but not which symbols make it, and the fix is always
/// at a symbol.
#[tokio::test]
async fn a_cycle_names_the_symbols_that_close_it() {
    let dir = tempfile::tempdir().unwrap();
    // Two files that call each other: a real file-level cycle.
    std::fs::write(
        dir.path().join("a.rs"),
        "pub fn from_a() -> u32 { crate::b::from_b() }\n",
    )
    .unwrap();
    std::fs::write(
        dir.path().join("b.rs"),
        "pub fn from_b() -> u32 { crate::a::from_a() }\n",
    )
    .unwrap();
    let tool = tool(dir.path());
    tool.dispatch(&serde_json::json!({ "action": "index" }))
        .unwrap();
    let result = tool
        .dispatch(&serde_json::json!({ "action": "architecture", "view": "cycles" }))
        .unwrap();
    let text = text_of(&result);
    assert!(
        text.contains("cycle"),
        "the fixture must produce a cycle, or the test proves nothing: {text}"
    );
    assert!(
        text.contains("via ") && (text.contains("from_a") || text.contains("from_b")),
        "a cycle must name the symbols that close it, not only the files: {text}"
    );
}

/// The review's §2, `verify`'s next step: "Различать здоровье индекса и
/// полноту/правильность конкретного ответа." A graph that resolved half its calls can pass
/// the receipt chain and still give a lower-bound caller set — an agent reading "accept"
/// would take a thin answer as complete.
#[tokio::test]
async fn verify_states_answer_coverage_beside_the_receipt_chain() {
    let dir = tempfile::tempdir().unwrap();
    // Two unresolved calls (no such definitions) out of three sites: well under the
    // clean threshold, so the answer must be labelled partial.
    std::fs::write(
        dir.path().join("lib.rs"),
        "pub fn real() -> u32 { 1 }\n\
             pub fn caller() -> u32 { real() + wibble() + wobble() }\n",
    )
    .unwrap();
    let tool = tool(dir.path());
    tool.dispatch(&serde_json::json!({ "action": "index" }))
        .unwrap();
    let result = tool
        .dispatch(&serde_json::json!({ "action": "verify" }))
        .unwrap();
    let details = result.details.clone();
    let coverage = &details["coverage"];
    assert!(
        coverage["sites"].as_u64().unwrap_or(0) >= 3,
        "the fixture must record its call sites: {details}"
    );
    assert!(
        coverage["resolved"].as_u64().unwrap_or(1) < coverage["sites"].as_u64().unwrap_or(1),
        "and some must be unresolved: {details}"
    );
    let text = text_of(&result);
    assert!(
        text.contains("answer coverage"),
        "the verdict must state answer coverage, not only the chain: {text}"
    );
    assert!(
        text.contains("ANSWERS ARE PARTIAL"),
        "a thin index must say its answers are partial: {text}"
    );
}

#[tokio::test]
async fn a_superseded_decision_says_it_no_longer_holds() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("lib.rs"), "pub fn x() {}\n").unwrap();
    let tool = tool(dir.path());
    tool.dispatch(&serde_json::json!({ "action": "index" }))
        .unwrap();

    let record = |title: &str, decision: &str, supersedes: Option<i64>| {
        let mut params = serde_json::json!({
            "action": "adr",
            "title": title,
            "context": "a decision was needed",
            "decision": decision,
            "consequences": "follow it",
        });
        if let Some(id) = supersedes {
            params["supersedes"] = serde_json::json!(id);
        }
        tool.dispatch(&params).unwrap()
    };
    let first = record("use a flat map", "flat", None);
    let all = tool
        .dispatch(&serde_json::json!({ "action": "adr" }))
        .unwrap();
    let listing = text_of(&all);
    let old_id: i64 = listing
        .split("- #")
        .nth(1)
        .and_then(|rest| rest.split_whitespace().next())
        .and_then(|digits| digits.parse().ok())
        .unwrap_or_else(|| {
            panic!(
                "id in listing: {listing}
first: {}",
                text_of(&first)
            )
        });

    // A newer decision replaces it.
    record("use a sharded map", "sharded", Some(old_id));

    let read = tool
        .dispatch(&serde_json::json!({ "action": "adr", "id": old_id }))
        .unwrap();
    let body = text_of(&read);
    assert!(
        body.contains("SUPERSEDED BY #"),
        "the replaced decision must name its replacement: {body}"
    );
    assert!(
        body.contains("no longer holds"),
        "and must say the decision does not stand: {body}"
    );

    // A decision nobody replaced carries no such line.
    let all = tool
        .dispatch(&serde_json::json!({ "action": "adr" }))
        .unwrap();
    let listing = text_of(&all);
    let newest: i64 = listing
        .split("- #")
        .skip(1)
        .filter_map(|rest| rest.split_whitespace().next())
        .filter_map(|digits| digits.parse::<i64>().ok())
        .max()
        .expect("two decisions exist");
    let read = tool
        .dispatch(&serde_json::json!({ "action": "adr", "id": newest }))
        .unwrap();
    assert!(
        !text_of(&read).contains("SUPERSEDED BY"),
        "the replacement itself is current: {}",
        text_of(&read)
    );
}

/// Review 4.4: `id` must not read another workspace's decision.
///
/// The list was scoped in a previous change; the by-id read was not, so an
/// agent that saw a decision id in one checkout could read a different
/// checkout's body — the same cross-tree leak the warm search path had.
#[tokio::test]
async fn an_adr_body_is_scoped_to_its_workspace_unless_scope_all() {
    let a = tempfile::tempdir().unwrap().path().join("service-a");
    let b = tempfile::tempdir().unwrap().path().join("service-b");
    std::fs::create_dir_all(&a).unwrap();
    std::fs::create_dir_all(&b).unwrap();
    std::fs::write(a.join("lib.rs"), "pub fn a() {}\n").unwrap();
    std::fs::write(b.join("lib.rs"), "pub fn b() {}\n").unwrap();
    let db = tempfile::tempdir().unwrap();
    let db_path = db.path().join("graph.db");

    let tool_a = GraphTool::new(db_path.clone(), a);
    tool_a
        .dispatch(&serde_json::json!({
            "action": "adr",
            "title": "A-only decision",
            "context": "the A tree",
            "decision": "keep it here",
            "consequences": "B must not see it",
        }))
        .unwrap();
    // The list is a SUMMARY (text lines + a count), so the id comes off the
    // rendering `- #<id> [status] title`, which is the shape a model reads.
    let listed = tool_a
        .dispatch(&serde_json::json!({ "action": "adr" }))
        .unwrap();
    let listing = text_of(&listed);
    let id: i64 = listing
        .split("- #")
        .nth(1)
        .and_then(|rest| rest.split_whitespace().next())
        .and_then(|digits| digits.parse().ok())
        .unwrap_or_else(|| panic!("the decision id must be in the listing: {listing}"));

    // B asks for A's id: the scoped read refuses it.
    let tool_b = GraphTool::new(db_path, b);
    let refused = tool_b.dispatch(&serde_json::json!({ "action": "adr", "id": id }));
    let message = format!("{refused:?}");
    assert!(
        refused.is_err() || !message.contains("keep it here"),
        "another workspace's decision body leaked: {message}"
    );

    // `scope: all` is the explicit opt-in that reads it.
    let all = tool_b
        .dispatch(&serde_json::json!({ "action": "adr", "id": id, "scope": "all" }))
        .expect("scope=all answers any workspace");
    assert!(
        text_of(&all).contains("keep it here"),
        "scope=all must return the body: {}",
        text_of(&all)
    );
}

/// §4.2 (audit): the `adr` table had no project field and `action_adr`
/// queried globally, so two projects with different names shared one set of
/// decisions, and the tool answered id/status/title only — an agent could
/// record a decision and never read its body back. Both closed: rows are
/// scoped to the project, and `id` returns the full body.
#[tokio::test]
async fn adr_records_are_scoped_to_their_project_and_bodies_are_readable() {
    let a = tempfile::tempdir().unwrap().path().join("service-a");
    let b = tempfile::tempdir().unwrap().path().join("service-b");
    std::fs::create_dir_all(&a).unwrap();
    std::fs::create_dir_all(&b).unwrap();
    std::fs::write(a.join("lib.rs"), "pub fn a() {}\n").unwrap();
    std::fs::write(b.join("lib.rs"), "pub fn b() {}\n").unwrap();
    let db = tempfile::tempdir().unwrap();
    let db_path = db.path().join("graph.db");

    let tool_a = GraphTool::new(db_path.clone(), a);
    tool_a
        .dispatch(&serde_json::json!({
            "action": "adr",
            "title": "Use SQLite for the graph",
            "context": "the index must survive restarts",
            "decision": "one SQLite file per workspace",
            "consequences": "a broken file is a rebuild, not a lost repo",
        }))
        .unwrap();

    // B's list must not carry A's decision.
    let tool_b = GraphTool::new(db_path, b);
    let list_b = tool_b
        .dispatch(&serde_json::json!({ "action": "adr" }))
        .unwrap();
    assert!(
        !text_of(&list_b).contains("SQLite for the graph"),
        "another project's decision leaked into the list: {}",
        text_of(&list_b)
    );

    // A sees it, and `id: 1` returns the body.
    let list_a = tool_a
        .dispatch(&serde_json::json!({ "action": "adr" }))
        .unwrap();
    assert!(text_of(&list_a).contains("SQLite for the graph"));
    let body = tool_a
        .dispatch(&serde_json::json!({ "action": "adr", "id": 1 }))
        .unwrap();
    let text = text_of(&body);
    assert!(text.contains("the index must survive restarts"), "{text}");
    assert!(text.contains("a broken file is a rebuild"), "{text}");
}
/// §4.3 (audit): an edit made after the index must be visible to the next
/// query (read-your-edit), and a query must never wait for a writer.
#[tokio::test]
async fn an_edit_after_the_index_is_visible_to_the_next_search() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("lib.rs"),
        "pub fn before_edit() -> u32 { 1 }\n",
    )
    .unwrap();
    let tool = tool(dir.path());
    tool.dispatch(&serde_json::json!({ "action": "index" }))
        .unwrap();
    let first = tool
        .dispatch(&serde_json::json!({ "action": "search", "query": "before_edit" }))
        .unwrap();
    assert!(
        text_of(&first).contains("before_edit"),
        "{}",
        text_of(&first)
    );

    // The edit lands AFTER the index; a stale mtime makes the pair differ.
    std::fs::write(
        dir.path().join("lib.rs"),
        "pub fn after_edit() -> u32 { 2 }\n// a longer body changes the (mtime, size) pair\n",
    )
    .unwrap();
    // `ensure_indexed` compares (mtime_ns, size); the new body is longer, so
    // the pair differs even when the filesystem's mtime granularity is coarse.

    let second = tool
        .dispatch(&serde_json::json!({ "action": "search", "query": "after_edit" }))
        .unwrap();
    assert!(
        text_of(&second).contains("after_edit"),
        "the edit must be visible: {}",
        text_of(&second)
    );
    assert!(
        text_of(&second).contains("refreshed"),
        "the answer must say the graph was refreshed: {}",
        text_of(&second)
    );
}
