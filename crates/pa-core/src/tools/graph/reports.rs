//! The response vocabulary: impact, evidence pack, change explanation.
//!
//! Pure data with `Serialize`; no behaviour lives here.

/// The blast radius of changing one symbol, with an HONEST envelope.
///
/// The review's §4 borrows `GitNexus`'s "impact with UNKNOWN and lower bound": a caller set
/// is a LOWER BOUND, never a proof of safety", and `epistemic envelope` means the answer
/// states its own omissions instead of reading as complete.
///
/// This is why `trace` alone cannot answer it. `trace` walks resolved `CALLS` edges, and a
/// call the resolver could not place (a method value, an interface dispatch, a name guess
/// below the floor) has NO edge — so "0 callers" from a trace means "0 resolved callers",
/// which an agent reads as "safe to delete". Measured on this repo: 2 643 unresolved call
/// sites in `pa-core` alone.
///
/// The count comes from `call_sites`, where every site is recorded whether it resolved or
/// not (the Д1 work), so the envelope can name what the walk could not see.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImpactReport {
    /// The symbol whose change is being assessed.
    pub symbol: String,
    /// Callers reached over resolved edges, nearest first.
    pub callers: Vec<ImpactCaller>,
    /// Sites in the tree that NAME this symbol as a callee but did not resolve to it.
    ///
    /// Each is a possible caller the walk cannot follow: method values, interface
    /// dispatch, a name the resolver guessed elsewhere. They are candidates, not facts —
    /// the review is explicit that a similarity or name match does not become a confirmed
    /// dependency.
    pub unresolved_candidates: Vec<ImpactCandidate>,
    /// What the answer could NOT see, in numbers.
    pub envelope: ImpactEnvelope,
}

/// What a change to one symbol touches, assembled from the answers that already exist.
///
/// The review's §9 names `graph.explain_change` as one of four task-oriented actions:
/// "затронутые symbols, witness paths, relevant ADR/tests, unresolved areas". Every part
/// existed as a separate answer; nothing composed them, so an agent asking "what does
/// changing this touch" had to call three actions and merge them by hand — which is exactly
/// the "dozens of weakly documented tools" §9 warns against.
///
/// The rule is composition, not a new traversal: `impact` supplies the callers and the
/// unresolved candidates, the ADR store is searched by the CHANGED FILE's name, and test
/// files are recognised by path convention because the graph has no test label.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChangeExplanation {
    /// The symbol whose change is being explained.
    pub symbol: String,
    /// Callers the walk resolved, each with its witness path.
    pub impact: ImpactReport,
    /// Decisions whose prose names the changed file.
    pub decisions: Vec<RelevantDecision>,
    /// Resolved callers that live in a test file, so the caller knows what will catch it.
    pub tests: Vec<String>,
    /// What this answer does NOT cover, in the same shape `impact` uses.
    pub not_covered: Vec<String>,
}

/// An ADR whose text names the changed file.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RelevantDecision {
    /// Numeric id.
    pub id: i64,
    /// Title.
    pub title: String,
    /// Lifecycle status (`accepted`, `proposed`, ...).
    pub status: String,
}

/// Whether `path` looks like a test file by convention.
///
/// The graph has no test label: tests are ordinary functions in ordinary files. The
/// conventions below cover the languages the indexer extracts, and a false positive only
/// means a file is listed as "may catch this", never that a check passed.
pub(crate) fn is_test_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    // A repo-relative path has no leading separator: `tests/check.rs` used to fall through
    // every arm because each one required a `/` before the directory name.
    lower.starts_with("tests/")
        || lower.starts_with("test/")
        || lower.starts_with("benches/")
        || lower.contains("/tests/")
        || lower.contains("/test/")
        || lower.contains("_test.")
        || lower.contains("_tests.")
        || lower.contains(".test.")
        || lower.contains(".spec.")
        || lower.contains("test_")
        || lower.contains("/benches/")
}

/// Confidence at or below which a resolution is a NAME GUESS rather than a fact.
///
/// `CallStrategy::Suffix` is the resolver's own guess tier (0.55); anything at or under it
/// could be pointing at a different definition with the same name, so an impact answer must
/// list it as a candidate instead of counting it as a confirmed caller.
pub const GUESS_CONFIDENCE: f64 = 0.6;

/// One resolved caller.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImpactCaller {
    pub qualified_name: String,
    pub file_path: String,
    pub start_line: u32,
    /// BFS distance from the changed symbol (1 = direct caller).
    pub depth: usize,
    /// Product of edge confidences along the path; a guess discounts everything below it.
    pub confidence: f32,
    /// The resolution basis of the edge that reached this caller (exact/scoped/guess).
    pub strategy: String,
    /// Why this caller is affected: the qualified names from the changed symbol down to it.
    ///
    /// "C, via CALLS" says how it was reached; `victim -> direct_caller` says WHY. The
    /// review's §10 lists "почему этот модуль затронут" among the most valuable answers, and
    /// that question is about the chain, not the last edge.
    pub witness: Vec<String>,
}

/// A call site that mentions the symbol but did not resolve to it.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImpactCandidate {
    pub caller_qn: String,
    pub rel_path: String,
    pub line: u32,
    /// Why the resolver could not place it.
    pub reason: String,
}

/// The numbers that make the answer's incompleteness checkable.
#[derive(Debug, Clone, Copy, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImpactEnvelope {
    /// Resolved callers found.
    pub resolved_callers: usize,
    /// Sites naming this callee that the answer cannot vouch for: unresolved ones, plus
    /// resolved ones whose confidence marks a NAME GUESS (they may point elsewhere).
    pub unresolved_mentions: usize,
    /// Edges the walk examined (its cost).
    pub edges_examined: usize,
    /// Whether the walk hit its step or work cap.
    pub truncated: bool,
    /// Total unresolved call sites in the project — the ceiling on what could be hidden.
    pub project_unresolved_sites: usize,
}

impl ImpactEnvelope {
    /// Whether an empty caller list may be read as "nothing calls this".
    ///
    /// Only when nothing was omitted EITHER: otherwise the zero is a lower bound, and a
    /// caller must check the candidates before deleting or changing a signature.
    #[must_use]
    pub fn empty_is_conclusive(&self) -> bool {
        self.resolved_callers == 0 && self.unresolved_mentions == 0 && !self.truncated
    }
}

/// One evidence pack: the bounded, explainable answer to "what do I need to know
/// about X", assembled under a budget rather than by opening files.
///
/// Every element names where it came from and how sure it is, because a pack is
/// evidence for a decision (an edit, a review, an answer), and evidence a reader
/// cannot check is a claim. The `omitted` list is the honest half: what the budget
/// kept out, so a thin pack is attributable to cost rather than read as "there is
/// nothing there".
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EvidencePack {
    /// The question the pack answers.
    pub query: String,
    /// The symbols each stage selected, in pack order.
    pub symbols: Vec<PackSymbol>,
    /// Call edges between the pack's own symbols, with direction and confidence.
    pub links: Vec<PackLink>,
    /// Coverage of the graph itself, so a thin pack can be attributed to a thin
    /// index rather than to absent code.
    pub coverage: PackCoverage,
    /// What the budget kept out.
    pub omitted: PackOmissions,
}

/// One symbol in a pack, with both placements: current (on disk now) and last
/// indexed.
///
/// The pair is the point. A pack is built from the INDEX, and an index can be
/// behind the disk; a reader that edits from `startLine`/`endLine` alone can write
/// into the wrong place. When the two disagree, `stale` says so.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PackSymbol {
    pub qualified_name: String,
    pub name: String,
    pub label: String,
    pub file_path: String,
    pub start_line: u32,
    pub end_line: u32,
    /// `None` when the file could not be read (deleted, unreadable).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_lines: Option<(u32, u32)>,
    /// The file's signature hash when indexed; a change past the signatures leaves
    /// the body editable but tells the reader the graph row is body-stale.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub surface_sha: Option<String>,
    /// Whether the indexed placement disagrees with the file on disk.
    pub stale: bool,
    /// Why this symbol is in the pack.
    pub reason: String,
}

/// A directed relation between two symbols that are BOTH in the pack.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PackLink {
    pub from: String,
    pub to: String,
    pub edge_type: String,
    /// `None` when the edge recorded no confidence (imports, contains).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confidence: Option<f32>,
}

/// The graph's own honesty, embedded so a pack is self-describing.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PackCoverage {
    pub sites: usize,
    pub resolved: usize,
    pub unresolved: usize,
}

/// What the budget kept out, by kind.
#[derive(Debug, Clone, Default, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PackOmissions {
    /// Symbols the search ranked below the cut.
    pub symbols_below_cut: usize,
    /// Links whose endpoint fell outside the symbol budget.
    pub links_outside_pack: usize,
    /// Files that could not be read to check freshness.
    pub unreadable_files: usize,
}

/// Symbols a pack carries by default.
pub(crate) const PACK_DEFAULT_SYMBOLS: usize = 12;
/// Hard ceiling: a pack is a bounded answer, and a caller asking for a thousand
/// symbols wants a search, not a pack.
pub(crate) const PACK_MAX_SYMBOLS: usize = 64;
