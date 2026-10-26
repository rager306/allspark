//! The native `graph` tool: a thin [`pa_agent::types::AgentTool`] over the
//! `pa-graph` code-graph store (index/search/trace/architecture/coverage/
//! changes/reindex). FORK-ONLY.
//!
//! The store is SQLite-backed and single-connection (`!Sync`), so it lives
//! behind a `Mutex<Option<GraphStore>>` and opens lazily on first use; the
//! whole dispatch runs on `spawn_blocking` (SQLite, the directory walk, and
//! the git subprocess are all blocking work).
//!
//! Two connections, one file: the `store` lock guards the *writer* only. A read
//! action takes a connection of its own from `reader`, which no writer holds, so
//! a `search` against a store being indexed does not wait for the build. WAL is
//! what makes that safe: a reader sees the last committed state while a writer
//! appends, and `pa-graph::lease` already leaves readers outside the writer
//! lease for the same reason.
//!
//! The reader is opened lazily and kept rather than opened per call, because
//! opening is not free of writes: `GraphStore::open` re-applies the schema and
//! stamps `store_meta.schema_version`, measured at 20 ms median and 850 ms worst
//! case behind a live writer, against 0.3 ms for a read on an open connection. A
//! build warms it before it takes the writer (see `warm_reader`).

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

// FORK-ONLY (jev Phase 2): the optional judge re-rank of `search` results.
use crate::jev::graph_rerank::{GraphReranker, RerankCandidate, VERIFIED_MARKER};

use pa_agent::abort::AbortSignal;
use pa_agent::types::{AgentTool, AgentToolResult, AgentToolUpdateCallback};
use pa_graph::{
    ArchitectureView, Direction, EdgeType, GraphStore, IndexOptions, IndexReport, Label,
    SearchOptions, TraceOptions, TreeNode,
};

/// Project name a store rooted at `root` indexes under (its final path
/// component, or `"project"` when the root has none).
fn project_name(root: &std::path::Path) -> String {
    root.file_name()
        .and_then(|name| name.to_str())
        .map_or_else(|| "project".to_string(), str::to_string)
}

/// Parse a trace direction (`in`/`out`/`both`); anything else is outgoing.
fn parse_direction(raw: Option<&str>) -> Direction {
    match raw.map(str::trim).map(str::to_ascii_lowercase).as_deref() {
        Some("in" | "incoming" | "callers") => Direction::In,
        Some("both" | "all") => Direction::Both,
        _ => Direction::Out,
    }
}

/// Render a count map as `key=value` pairs for report lines.
fn render_counts(counts: &std::collections::BTreeMap<String, usize>) -> String {
    counts
        .iter()
        .map(|(key, count)| format!("{key}={count}"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Append one file-tree directory (and its children, recursively) to the
/// report lines.
fn render_file_tree(node: &TreeNode, depth: usize, lines: &mut Vec<String>) {
    let indent = "  ".repeat(depth);
    lines.push(format!(
        "{indent}{}: {} file(s), {} symbol(s)",
        node.name,
        node.files.len(),
        node.symbol_count,
    ));
    for file in &node.files {
        lines.push(format!("{indent}  {file}"));
    }
    for child in &node.children {
        render_file_tree(child, depth + 1, lines);
    }
}

/// Parse the comma-separated edge-type filter (`calls,imports,...`).
fn parse_edge_types(raw: Option<&str>) -> Vec<EdgeType> {
    let Some(raw) = raw else {
        return Vec::new();
    };
    raw.split(',')
        .filter_map(|name| match name.trim().to_ascii_lowercase().as_str() {
            "calls" | "call" => Some(EdgeType::Calls),
            "imports" | "import" => Some(EdgeType::Imports),
            "inherits" | "inherit" => Some(EdgeType::Inherits),
            "uses" | "use" => Some(EdgeType::Uses),
            "contains" => Some(EdgeType::Contains),
            _ => None,
        })
        .collect()
}

/// The `label` vocabulary `search` accepts, as `(request spelling, node label)`.
///
/// Lowercase and explicit rather than `Label::from_str_or_default`, which maps
/// anything unknown to `Function`: a typo would silently search functions
/// instead of failing, the same class of bug as a misspelt parameter.
const SEARCH_LABELS: &[(&str, Label)] = &[
    ("function", Label::Function),
    ("method", Label::Method),
    ("class", Label::Class),
    ("struct", Label::Struct),
    ("enum", Label::Enum),
    ("interface", Label::Interface),
    ("module", Label::Module),
    ("file", Label::File),
    ("folder", Label::Folder),
    ("branch", Label::Branch),
];

/// Read `mmr` as a diversity weight, refusing anything outside `[0, 1]`.
///
/// The range is checked here rather than clamped, because the store clamps
/// silently: a caller that asked for 2 would get 1 and never learn why the
/// result stopped looking like relevance ranking. Returned as the `f64` the
/// caller wrote, so `details.search` echoes 0.3 rather than the `f32` nearest
/// to it.
fn parse_mmr(raw: &serde_json::Value) -> anyhow::Result<f64> {
    let diversity = raw
        .as_f64()
        .ok_or_else(|| anyhow::anyhow!("`mmr` must be a number in [0, 1], got {raw}"))?;
    if !(0.0..=1.0).contains(&diversity) {
        anyhow::bail!(
            "`mmr` must be in [0, 1] (0 = no diversity reranking, 1 = maximum spread), got {diversity}"
        );
    }
    Ok(diversity)
}

/// Parse `label` into node labels, naming the accepted spellings on a miss.
fn parse_labels(raw: &str) -> anyhow::Result<(Vec<Label>, Vec<String>)> {
    let mut labels: Vec<Label> = Vec::new();
    for name in raw
        .split(',')
        .map(str::trim)
        .filter(|name| !name.is_empty())
    {
        let wanted = name.to_ascii_lowercase();
        let Some((_, label)) = SEARCH_LABELS
            .iter()
            .find(|(spelling, _)| *spelling == wanted)
        else {
            let accepted: Vec<&str> = SEARCH_LABELS
                .iter()
                .map(|(spelling, _)| *spelling)
                .collect();
            anyhow::bail!(
                "unknown label {name:?}: accepted are {}",
                accepted.join(", ")
            );
        };
        if !labels.contains(label) {
            labels.push(*label);
        }
    }
    let names = labels
        .iter()
        .map(|label| label.as_str().to_string())
        .collect();
    Ok((labels, names))
}

/// Read `respect_gitignore` as a boolean, defaulting to `false`.
///
/// `false` is [`IndexOptions::new`]'s own default — index what is on disk — so an
/// absent parameter and an explicit `false` must mean the same run. A non-boolean
/// value is an error rather than a silent `false`, because the two modes produce
/// different file sets and a caller that wrote `"true"` would otherwise index the
/// other one without learning it.
fn parse_respect_gitignore(params: &serde_json::Value) -> anyhow::Result<bool> {
    match params.get("respect_gitignore") {
        None => Ok(false),
        Some(raw) => raw
            .as_bool()
            .ok_or_else(|| anyhow::anyhow!("`respect_gitignore` must be a boolean, got {raw}")),
    }
}

/// The run report plus the scope that produced it, for `details`.
///
/// `index`/`reindex` answers used to carry the report alone, so a caller could not
/// tell which file set the numbers describe: the same tree reports different
/// `added`/`removed`/`skipped` counts in the two modes, and `removed` in particular
/// is how a lean pass over a wide store shows that it dropped the ignored files.
/// `applied` carries the other scope knobs the caller set, and is empty when it
/// set none — so a run with no knob keeps the answer it always had.
fn with_effective_ignore(
    report: &impl serde::Serialize,
    respect_gitignore: bool,
    applied: serde_json::Map<String, serde_json::Value>,
) -> serde_json::Value {
    let mut details = serde_json::to_value(report).unwrap_or(serde_json::Value::Null);
    if let Some(object) = details.as_object_mut() {
        object.insert(
            "respect_gitignore".to_string(),
            serde_json::json!(respect_gitignore),
        );
        object.extend(applied);
    }
    details
}

/// Read `confidence_floor` as a number in `[0, 1]`, or `None` when absent.
///
/// Out of range is an error naming the range, for the reason `mmr` is: a clamp
/// would answer 2 with the same listing 1 gives and never say that the value the
/// caller wrote did not run. `0` is a legitimate floor (`>= 0` keeps everything
/// the walk reached), so it is not treated as "unset".
fn parse_confidence_floor(params: &serde_json::Value) -> anyhow::Result<Option<f64>> {
    match params.get("confidence_floor") {
        None => Ok(None),
        Some(raw) => {
            let floor = raw.as_f64().ok_or_else(|| {
                anyhow::anyhow!("`confidence_floor` must be a number in [0, 1], got {raw}")
            })?;
            if !(0.0..=1.0).contains(&floor) {
                anyhow::bail!(
                    "`confidence_floor` must be in [0, 1] (0 keeps every step the walk reached, \
                     1 keeps only fully certain ones), got {floor}"
                );
            }
            Ok(Some(floor))
        }
    }
}

/// Read `lease_wait` as a millisecond count (`0` = one attempt, then give up).
///
/// The store's own contract is [`std::time::Duration`], where zero means "try
/// once": the milliseconds here are that same quantity, so `0` is not a special
/// case in the tool, it is the store's existing meaning.
fn parse_lease_wait(params: &serde_json::Value) -> anyhow::Result<Option<u64>> {
    match params.get("lease_wait") {
        None => Ok(None),
        Some(raw) => raw.as_u64().map(Some).ok_or_else(|| {
            anyhow::anyhow!(
                "`lease_wait` must be an integer number of milliseconds >= 0, got {raw}"
            )
        }),
    }
}

/// Read `extensions` as a lowercase allowlist, or `None` when absent.
///
/// A leading dot and case are normalised (`.RS` and `rs` name the same files),
/// because the walk compares the extension exactly: a caller that wrote `.rs`
/// would otherwise restrict the run to nothing and read the empty graph as an
/// empty codebase. An unknown extension is *not* an error — see
/// [`unmatched_extensions`] for how it is reported instead.
fn parse_extensions(params: &serde_json::Value) -> anyhow::Result<Option<Vec<String>>> {
    let Some(raw) = params.get("extensions") else {
        return Ok(None);
    };
    let raw = raw.as_str().ok_or_else(|| {
        anyhow::anyhow!("`extensions` must be a comma-separated string, got {raw}")
    })?;
    let mut extensions: Vec<String> = Vec::new();
    for name in raw
        .split(',')
        .map(str::trim)
        .filter(|name| !name.is_empty())
    {
        let name = name.trim_start_matches('.').to_ascii_lowercase();
        if name.is_empty() {
            anyhow::bail!("`extensions` has an empty entry in {raw:?}: write e.g. \"rs,ts\"");
        }
        if !extensions.contains(&name) {
            extensions.push(name);
        }
    }
    if extensions.is_empty() {
        anyhow::bail!("`extensions` must name at least one extension, e.g. \"rs,ts\": got {raw:?}");
    }
    Ok(Some(extensions))
}

/// Read `max_file_bytes` as a size of at least one byte.
fn parse_max_file_bytes(params: &serde_json::Value) -> anyhow::Result<Option<u64>> {
    match params.get("max_file_bytes") {
        None => Ok(None),
        Some(raw) => raw
            .as_u64()
            .filter(|bytes| *bytes >= 1)
            .map(Some)
            .ok_or_else(|| anyhow::anyhow!("`max_file_bytes` must be an integer >= 1, got {raw}")),
    }
}

/// Which requested extensions indexed no file.
///
/// The walk's allowlist is exact, so a misspelt or absent extension silently
/// yields the empty set — the answer then reads as "this codebase has no such
/// file", which is the same false negative the cold-start build exists to
/// prevent. Reporting the misses turns that into a stated fact. The set is read
/// back from the store, so it describes what this run left indexed rather than
/// what the walk was asked for.
fn unmatched_extensions(
    store: &GraphStore,
    project: &str,
    extensions: &[String],
) -> anyhow::Result<Vec<String>> {
    let files = store
        .all_file_hashes(project)
        .map_err(|err| anyhow::anyhow!("failed to read the indexed file set: {err}"))?;
    let matched: std::collections::HashSet<&str> = files
        .keys()
        .filter_map(|rel| Path::new(rel).extension().and_then(|ext| ext.to_str()))
        .collect();
    Ok(extensions
        .iter()
        .filter(|ext| !matched.contains(ext.as_str()))
        .cloned()
        .collect())
}

/// The index-scope knobs a caller set, read once for `index` and `reindex`.
///
/// All three reach the store through `IndexOptions` builders that already
/// existed; the gap was that no tool call could set them. Kept as one struct so
/// the two actions that share `discover`/the writer lease read them the same way
/// and neither drifts from the other.
#[derive(Debug, Default)]
struct IndexScope {
    /// Milliseconds to wait for another index run's writer lease.
    lease_wait: Option<u64>,
    /// Extension allowlist, normalised.
    extensions: Option<Vec<String>>,
    /// Largest file to index.
    max_file_bytes: Option<u64>,
}

impl IndexScope {
    /// Read the three knobs from a call's parameters.
    fn read(params: &serde_json::Value) -> anyhow::Result<Self> {
        Ok(Self {
            lease_wait: parse_lease_wait(params)?,
            extensions: parse_extensions(params)?,
            max_file_bytes: parse_max_file_bytes(params)?,
        })
    }

    /// Apply the knobs the caller set to `opts`.
    fn apply(&self, mut opts: IndexOptions) -> IndexOptions {
        if let Some(ms) = self.lease_wait {
            opts = opts.with_lease_wait(std::time::Duration::from_millis(ms));
        }
        if let Some(extensions) = &self.extensions {
            opts = opts.with_extensions(extensions.clone());
        }
        if let Some(bytes) = self.max_file_bytes {
            opts = opts.with_max_file_bytes(bytes);
        }
        opts
    }

    /// The `details` echo, recording only the knobs the caller set.
    fn details(&self, unmatched: &[String]) -> serde_json::Map<String, serde_json::Value> {
        let mut applied = serde_json::Map::new();
        if let Some(ms) = self.lease_wait {
            applied.insert("lease_wait".to_string(), serde_json::json!(ms));
        }
        if let Some(bytes) = self.max_file_bytes {
            applied.insert("max_file_bytes".to_string(), serde_json::json!(bytes));
        }
        if let Some(extensions) = &self.extensions {
            applied.insert("extensions".to_string(), serde_json::json!(extensions));
            applied.insert(
                "extensions_unmatched".to_string(),
                serde_json::json!(unmatched),
            );
        }
        applied
    }

    /// Extensions that matched no file, or an empty list when none were set.
    fn unmatched(&self, store: &GraphStore, project: &str) -> anyhow::Result<Vec<String>> {
        match &self.extensions {
            None => Ok(Vec::new()),
            Some(extensions) => unmatched_extensions(store, project, extensions),
        }
    }
}

/// The note a caller must see in the answer's own text, or `None`.
///
/// `details` is not what a provider forwards to the model, so an empty result
/// caused by a misspelt extension has to say so where the model reads: "0
/// files" and "no `.rs` file here" are the same answer otherwise, and the empty
/// graph then reads as an empty codebase. An unscoped run produces no note, so
/// its text stays exactly what it was.
fn unmatched_note(unmatched: &[String]) -> Option<String> {
    if unmatched.is_empty() {
        return None;
    }
    let missed = unmatched.join(", ");
    Some(format!(
        "Requested extension(s) {missed} matched no indexed file: this run left the index \
         with none of them, so an empty answer here means the allowlist excluded everything, \
         not that the code is absent."
    ))
}

/// The note that names a cold-start build, and the mode it ran in.
///
/// `search`, `trace` and `architecture` each build the graph when it is empty. The
/// note belongs to the build rather than to the action, so it is written once here
/// and says which files the build could see: an implicit build is lean, and a lean
/// build must not let a symbol under an ignore rule read as a miss.
fn cold_build_note(report: &IndexReport) -> String {
    format!(
        "The graph was empty, so this call indexed the project first: {} nodes, {} edges, \
         {} files parsed. This build is lean: files the project's gitignore rules exclude are \
         not in it (run `index` to include them).",
        report.store_nodes, report.edges, report.parsed,
    )
}

/// Read `candidate_multiplier` as a pool factor of at least 1.
fn parse_candidate_multiplier(raw: &serde_json::Value) -> anyhow::Result<usize> {
    let multiplier = raw
        .as_u64()
        .filter(|multiplier| *multiplier >= 1)
        .ok_or_else(|| {
            anyhow::anyhow!("`candidate_multiplier` must be an integer >= 1, got {raw}")
        })?;
    usize::try_from(multiplier)
        .map_err(|_| anyhow::anyhow!("`candidate_multiplier` is too large: {multiplier}"))
}

/// The parameter names each action accepts (`action` itself aside).
///
/// The tool reads parameters with `.get(..)`, so without this table a
/// misspelt key silently falls back to the default and the call reports
/// success — the `GitNexus` bug #3261, where a caller sent `depth` instead of
/// `maxDepth`, got the default, and saw no error. The same shape was worse
/// here: `depth` named two different things (the index build depth, a string,
/// and the trace BFS depth, a number), so the wrong reading was silently
/// *plausible* instead of merely wrong. `index` still accepts plain `depth`
/// for compatibility; its canonical spelling is `index_depth`.
const ACCEPTED_PARAMS: &[(&str, &[&str])] = &[
    // `depth` here is the legacy spelling of `index_depth`.
    (
        "index",
        &[
            "path",
            "root",
            "force",
            "index_depth",
            "depth",
            "respect_gitignore",
            "lease_wait",
            "extensions",
            "max_file_bytes",
        ],
    ),
    // Discovery is shared with `index` (`reindex_incremental` calls the same
    // `discover`) and so is the writer lease, so the ignore mode and the three
    // scope knobs decide this run the same way.
    (
        "reindex",
        &[
            "path",
            "root",
            "respect_gitignore",
            "lease_wait",
            "extensions",
            "max_file_bytes",
        ],
    ),
    (
        "search",
        &[
            "path",
            "root",
            "query",
            "limit",
            "mmr",
            "label",
            "regex",
            "candidate_multiplier",
        ],
    ),
    (
        "trace",
        &[
            "path",
            "root",
            "symbol",
            "direction",
            "depth",
            "limit",
            "edge_types",
            "confidence_floor",
        ],
    ),
    ("architecture", &["path", "root", "view"]),
    ("coverage", &["path", "root"]),
    // `refA`/`refB` are the camelCase spellings the schema never documented;
    // they stay accepted so an existing caller does not break.
    (
        "changes",
        &["path", "root", "ref_a", "refA", "ref_b", "refB"],
    ),
    ("status", &["path", "root"]),
    ("verify", &["path", "root"]),
    (
        "adr",
        &[
            "path",
            "root",
            "id",
            "scope",
            "title",
            "decision",
            "context",
            "consequences",
            "status",
            "supersedes",
        ],
    ),
];

/// The accepted parameter names for `action`, resolving the aliases
/// (`arch`, `detect_changes`, `reindex_incremental`) to their canonical entry.
fn accepted_params(action: &str) -> Option<&'static [&'static str]> {
    let canonical = match action {
        "arch" => "architecture",
        "detect_changes" => "changes",
        "reindex_incremental" => "reindex",
        other => other,
    };
    ACCEPTED_PARAMS
        .iter()
        .find(|(name, _)| *name == canonical)
        .map(|(_, accepted)| *accepted)
}

/// Reject any parameter `action` does not read.
///
/// The error names the offending key and lists the accepted ones: a caller
/// that gets a default back cannot tell a typo from an empty result, and the
/// next call repeats the mistake.
fn reject_unknown_params(
    action: &str,
    accepted: &[&str],
    params: &serde_json::Value,
) -> anyhow::Result<()> {
    let Some(object) = params.as_object() else {
        return Ok(());
    };
    for key in object.keys() {
        if key == "action" || accepted.contains(&key.as_str()) {
            continue;
        }
        anyhow::bail!(
            "unknown parameter {key:?} for action {action:?}: accepted are {}",
            accepted.join(", "),
        );
    }
    Ok(())
}

/// The native `graph` tool. `db_path` is the SQLite store; `cwd` is the
/// default project root index/search/trace resolve against.
pub struct GraphTool {
    db_path: PathBuf,
    cwd: PathBuf,
    /// The write connection: `index`/`reindex` and an `adr` that records.
    store: Mutex<Option<GraphStore>>,
    /// The read connection, used by every read action and by the status line
    /// that rides them. Separate from `store` so a read never queues behind a
    /// build; `Mutex<Option<_>>` for the same lazy-open reason.
    reader: Mutex<Option<GraphStore>>,
    /// single-flight flag for the cold-start build (see `ensure_indexed`).
    in_flight: std::sync::Arc<std::sync::atomic::AtomicBool>,
    schema: serde_json::Value,
    /// FORK-ONLY (jev Phase 2): when set, `search` results are re-ranked by
    /// the judge (`[jev] graphRerank = auto` behind a native judge). `None`
    /// (the default) returns the store's own order, untouched.
    reranker: Option<Arc<GraphReranker>>,
}

impl GraphTool {
    /// Build the tool over `db_path`, defaulting every project to `cwd`.
    #[must_use]
    pub fn new(db_path: PathBuf, cwd: PathBuf) -> Self {
        Self {
            db_path,
            cwd,
            store: Mutex::new(None),
            reader: Mutex::new(None),
            in_flight: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            schema: params_schema(),
            reranker: None,
        }
    }

    /// FORK-ONLY (jev Phase 2): attach the judge re-rank of `search`
    /// results.
    #[must_use]
    pub fn with_reranker(mut self, reranker: Option<Arc<GraphReranker>>) -> Self {
        self.reranker = reranker;
        self
    }

    /// The project root an action resolves against: the `path`/`root`
    /// argument when given (resolved against `cwd`), else `cwd`.
    fn resolve_root(&self, params: &serde_json::Value) -> PathBuf {
        let raw = params
            .get("path")
            .or_else(|| params.get("root"))
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|raw| !raw.is_empty());
        match raw {
            Some(raw) => {
                let candidate = PathBuf::from(raw);
                if candidate.is_absolute() {
                    candidate
                } else {
                    self.cwd.join(candidate)
                }
            }
            None => self.cwd.clone(),
        }
    }

    /// Index the project when the graph has never seen it.
    ///
    /// `search`/`trace`/`architecture` on a cold graph answered "0 hits" and
    /// looked like a completed query, which is how the agent concluded the
    /// graph had nothing to say and fell back to reading files one by one. The
    /// first call is the only one that pays the build cost; later calls hit the
    /// on-disk store, and a `force` index from the model still overrides this.
    /// Returns the report when a build happened, so the reply can name it.
    fn ensure_indexed(&self, root: &Path, project: &str) -> anyhow::Result<Option<IndexReport>> {
        // The project key is the root's BASENAME, so two checkouts named the
        // same (`/a/service`, `/b/service`) share one row set in the shared DB.
        // The write path refuses to overwrite a project with a different root
        // (`register_project_at`), but the WARM path used to answer from
        // `file_count > 0` alone — so a query for B read A's rows and returned
        // another project's symbols as its own answer. The root is validated
        // before any read below, on every path.
        let (indexed, stored_root) = self.with_reader(|store| {
            let count = store
                .file_count(project)
                .map_err(|err| anyhow::anyhow!("status failed: {err}"))?;
            let stored = store
                .project_root(project)
                .map_err(|err| anyhow::anyhow!("project root read failed: {err}"))?;
            Ok::<_, anyhow::Error>((count > 0, stored))
        })?;
        if indexed {
            let same_root = stored_root.as_deref().is_some_and(|stored| {
                stored == root.to_string_lossy()
                    || match (std::fs::canonicalize(&stored), std::fs::canonicalize(root)) {
                        (Ok(stored), Ok(requested)) => stored == requested,
                        _ => false,
                    }
            });
            if !same_root {
                anyhow::bail!(
                    "graph project {project:?} is indexed from a different root ({}), not {}:                      two checkouts with the same directory name share one project key, and an                      answer from the other tree would be wrong. Set `graph.dbPath` to a                      per-workspace database, or run the `index` action for this root.",
                    stored_root.as_deref().unwrap_or("<none>"),
                    root.display()
                );
            }
            return Ok(None);
        }
        // single-flight: one cold-start build at a time. A caller that arrives
        // while another build is running proceeds against the current store —
        // `compare_exchange` returning false means a build is already under
        // way — rather than starting a second walk over the same files.
        if self
            .in_flight
            .compare_exchange(
                false,
                true,
                std::sync::atomic::Ordering::AcqRel,
                std::sync::atomic::Ordering::Acquire,
            )
            .is_err()
        {
            return Ok(None);
        }
        let _guard = crate::tools::graph_in_flight::InFlightGuard(self.in_flight.clone());
        // Implicit builds are lean. This one runs because a query arrived, not
        // because anyone asked for an index: the answer must not carry the same
        // symbol twice, and the walk must not drag in `target/` or a scratch copy of
        // the tree. A caller who wants the ignored files in the graph runs the
        // `index` action, which defaults to them.
        let opts = IndexOptions::new(project).with_respect_gitignore(true);
        let report = self.with_store(|store| {
            store
                .index_project(root, &opts)
                .map_err(|err| anyhow::anyhow!("index failed: {err}"))
        })?;
        Ok(Some(report))
    }

    /// Open (once) and return a lock on the write store, creating the DB on
    /// demand.
    fn with_store<T>(
        &self,
        f: impl FnOnce(&mut GraphStore) -> anyhow::Result<T>,
    ) -> anyhow::Result<T> {
        let mut guard = self
            .store
            .lock()
            .map_err(|_| anyhow::anyhow!("graph store lock poisoned"))?;
        let store = Self::open_into(&mut guard, &self.db_path)?;
        f(store)
    }

    /// Run `f` on the reader connection: one no writer holds, so it runs while
    /// a build is in flight.
    fn with_reader<T>(
        &self,
        f: impl FnOnce(&GraphStore) -> anyhow::Result<T>,
    ) -> anyhow::Result<T> {
        let mut guard = self
            .reader
            .lock()
            .map_err(|_| anyhow::anyhow!("graph reader lock poisoned"))?;
        let store = Self::open_into(&mut guard, &self.db_path)?;
        f(store)
    }

    /// Run a read of the graph.
    ///
    /// Normally on the reader, so it never waits for a build: an `index` over an
    /// existing store is a measured 133 s pass and a search is 2 ms of SQLite,
    /// and nothing about a read gets better by queueing behind that.
    ///
    /// The exception is the cold-start build (`ensure_indexed`), which is
    /// running precisely because the store has nothing to read yet. There a read
    /// waits for it on the writer, as it always did: answering "0 hits" from a
    /// store the first build is still filling is the false negative that once
    /// made the agent conclude the graph had nothing to say and read files one
    /// by one.
    fn with_read<T>(&self, f: impl FnOnce(&GraphStore) -> anyhow::Result<T>) -> anyhow::Result<T> {
        if self.in_flight.load(std::sync::atomic::Ordering::Acquire) {
            return self.with_store(|store| f(store));
        }
        self.with_reader(f)
    }

    /// Open the reader before a build starts.
    ///
    /// `GraphStore::open` re-applies the schema and stamps
    /// `store_meta.schema_version`, so it is a small *write*: opening the reader
    /// for the first time while a build is running would wait for that build's
    /// write lock (measured 850 ms worst case). Opening it before the build
    /// takes the lock keeps the first read during the build as cheap as the rest.
    fn warm_reader(&self) -> anyhow::Result<()> {
        self.with_reader(|_| Ok(()))
    }

    fn open_into<'a>(
        slot: &'a mut Option<GraphStore>,
        db_path: &Path,
    ) -> anyhow::Result<&'a mut GraphStore> {
        if slot.is_none() {
            let store = GraphStore::open(db_path)
                .map_err(|err| anyhow::anyhow!("failed to open graph store: {err}"))?;
            *slot = Some(store);
        }
        slot.as_mut()
            .ok_or_else(|| anyhow::anyhow!("graph store unavailable"))
    }

    fn dispatch(&self, params: &serde_json::Value) -> anyhow::Result<AgentToolResult> {
        let action = params
            .get("action")
            .and_then(serde_json::Value::as_str)
            .map_or("", str::trim)
            .to_ascii_lowercase();
        let mut result = self.dispatch_action(&action, params)?;
        // Every answer carries the store's own state on its last line, so a
        // stale or partial graph cannot answer as confidently as a fresh one.
        // `index`/`reindex` skip it: the run they just performed defines the
        // state they would report.
        if !matches!(action.as_str(), "index" | "reindex" | "reindex_incremental") {
            if let Some(line) = self.status_line(params) {
                if let Some(pa_agent::types::ToolResultContent::Text(content)) =
                    result.content.first_mut()
                {
                    content.text.push('\n');
                    content.text.push_str(&line);
                }
            }
        }
        Ok(result)
    }

    /// The `[graph: ...]` state line for the project a call targets.
    ///
    /// Returns `None` when the store cannot be read — a status failure must
    /// never turn an answer into an error.
    fn status_line(&self, params: &serde_json::Value) -> Option<String> {
        let root = self.resolve_root(params);
        let project = project_name(&root);
        // Memoised: a status line rides every answer, and the full probe costs
        // more than the query it annotates (measured 255 ms against a 2 ms trace).
        let line = self.with_read(|store| {
            Ok::<_, anyhow::Error>(pa_graph::status::cached_report(store, &project, &root).ok())
        });
        line.ok().flatten().map(|report| report.line())
    }

    fn dispatch_action(
        &self,
        action: &str,
        params: &serde_json::Value,
    ) -> anyhow::Result<AgentToolResult> {
        // An unknown *action* is the more fundamental error, so it keeps its own
        // message: only a known action has a parameter set to check against.
        if let Some(accepted) = accepted_params(action) {
            reject_unknown_params(action, accepted, params)?;
        }
        match action {
            "index" => self.action_index(params),
            "reindex" | "reindex_incremental" => self.action_reindex(params),
            "search" => self.action_search(params),
            "trace" => self.action_trace(params),
            "architecture" | "arch" => self.action_architecture(params),
            "coverage" => self.action_coverage(params),
            "changes" | "detect_changes" => self.action_changes(params),
            "status" => self.action_status(params),
            "verify" => self.action_verify(params),
            "adr" => self.action_adr(params),
            other => anyhow::bail!(
                "unknown graph action {other:?}: expected one of index, reindex, search, \
                 trace, architecture, coverage, changes, status, verify, adr"
            ),
        }
    }

    fn action_index(&self, params: &serde_json::Value) -> anyhow::Result<AgentToolResult> {
        let root = self.resolve_root(params);
        let project = project_name(&root);
        let mut opts = IndexOptions::new(&project);
        if params
            .get("force")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false)
        {
            opts = opts.with_force(true);
        }
        // A skeleton depth lets an agent get a usable graph in about a second
        // instead of two minutes, then deepen it in the background.
        //
        // `index_depth` is the canonical spelling because plain `depth` means
        // the trace BFS depth on `trace`; keeping one name for both was the
        // ambiguity this tool now refuses to guess at. `depth` is still read
        // here, for `index` only, so an existing caller keeps working.
        let depth = params
            .get("index_depth")
            .or_else(|| params.get("depth"))
            .and_then(serde_json::Value::as_str);
        if let Some(depth) = depth {
            opts = opts.with_depth(pa_graph::indexer::Depth::from_str_or_default(depth));
        }
        // `false` (the default) indexes what is on disk, ignored files included;
        // `true` is the lean view. The mode decides the whole discovered set, so
        // switching it also widens or prunes what the run reports: a lean pass over
        // a store built with `false` drops the ignored files it can no longer see.
        let respect_gitignore = parse_respect_gitignore(params)?;
        opts = opts.with_respect_gitignore(respect_gitignore);
        // The store could already narrow the walk to an extension allowlist, cap a
        // file's size, and wait for another session's writer lease. None of the
        // three was reachable from a tool call, so the model could not scope a build
        // to one language, keep a vendored multi-megabyte file out, or ask a
        // contended build to wait instead of fail.
        let scope = IndexScope::read(params)?;
        opts = scope.apply(opts);
        self.warm_reader()?;
        let (report, unmatched) = self.with_store(|store| {
            let report = store
                .index_project(&root, &opts)
                .map_err(|err| anyhow::anyhow!("index failed: {err}"))?;
            // Read the misses back from the store: the honest answer to "which
            // extension matched nothing" is about the files this run left indexed.
            let unmatched = scope.unmatched(store, &project)?;
            Ok((report, unmatched))
        })?;
        let mut text = format!(
            "Indexed project `{project}` at {}: {} nodes in the store, {} edges, \
             {} files parsed ({} added, {} modified, {} removed, {} unchanged, {} failed, \
             {} partial, {} unusable, {} skipped; {} node(s) written this run).",
            root.display(),
            report.store_nodes,
            report.edges,
            report.parsed,
            report.added,
            report.modified,
            report.removed,
            report.unchanged,
            report.failed,
            report.partial,
            report.unusable,
            report.skipped,
            report.nodes,
        );
        if let Some(note) = unmatched_note(&unmatched) {
            text.push('\n');
            text.push_str(&note);
        }
        Ok(AgentToolResult {
            content: vec![pa_agent::types::ToolResultContent::text(text)],
            details: with_effective_ignore(&report, respect_gitignore, scope.details(&unmatched)),
            terminate: None,
            is_error: false,
        })
    }

    fn action_reindex(&self, params: &serde_json::Value) -> anyhow::Result<AgentToolResult> {
        let root = self.resolve_root(params);
        let project = project_name(&root);
        let respect_gitignore = parse_respect_gitignore(params)?;
        // The incremental path shares discovery with `index` and takes the same
        // writer lease, so it reads the same three scope knobs.
        let scope = IndexScope::read(params)?;
        let opts =
            scope.apply(IndexOptions::new(&project).with_respect_gitignore(respect_gitignore));
        self.warm_reader()?;
        let (report, unmatched) = self.with_store(|store| {
            let report = store
                .reindex_incremental(&root, &opts)
                .map_err(|err| anyhow::anyhow!("reindex failed: {err}"))?;
            let unmatched = scope.unmatched(store, &project)?;
            Ok((report, unmatched))
        })?;
        let mut text = format!(
            "Incrementally reindexed project `{project}`: {} files reindexed, {} closure \
             dependents, {} removed, {} unchanged (reason: {:?}).",
            report.files_reindexed,
            report.closure_files,
            report.removed,
            report.unchanged,
            report.reason,
        );
        if let Some(note) = unmatched_note(&unmatched) {
            text.push('\n');
            text.push_str(&note);
        }
        Ok(AgentToolResult {
            content: vec![pa_agent::types::ToolResultContent::text(text)],
            details: with_effective_ignore(&report, respect_gitignore, scope.details(&unmatched)),
            terminate: None,
            is_error: false,
        })
    }

    fn action_search(&self, params: &serde_json::Value) -> anyhow::Result<AgentToolResult> {
        let query = params
            .get("query")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|query| !query.is_empty())
            .ok_or_else(|| anyhow::anyhow!("search requires a non-empty `query`"))?;
        let root = self.resolve_root(params);
        let project = project_name(&root);
        // A cold graph answers "0 hits" for everything; building it here turns
        // the first query into a real answer instead of a false negative.
        let indexed_now = self.ensure_indexed(&root, &project)?;
        let mut opts = SearchOptions::new(&project);
        if let Some(limit) = params.get("limit").and_then(serde_json::Value::as_u64) {
            opts = opts.with_limit(limit as usize);
        }
        // The store can already diversify (MMR), restrict by label and filter by
        // regex, but nothing could ask it to: every knob was reachable only from
        // Rust. These four read the model's request into the same builder, so a
        // tool call can use what `SearchOptions` carries.
        //
        // What was asked is echoed in `details.search`, and only the keys the
        // caller actually set are recorded: a call that sets no knob produces no
        // `search` object at all, so its answer stays exactly what it was before
        // these knobs existed.
        let mut applied = serde_json::Map::new();
        if let Some(raw) = params.get("mmr") {
            let diversity = parse_mmr(raw)?;
            applied.insert("mmr".to_string(), serde_json::json!(diversity));
            // The parameter is a *diversity* weight (0 = none), while the store's
            // lambda is a *relevance* weight (1.0 = pure relevance); the two are
            // complements. `0` also leaves the store on its plain path instead of
            // reranking at lambda 1.0: "no diversity reranking" must mean the same
            // order as asking for nothing, not a re-sort that happens to ignore
            // diversity.
            if diversity > 0.0 {
                // In `[0, 1]` by `parse_mmr`, so the cosine weighting is exact
                // enough and the narrowing cast is safe.
                #[allow(clippy::cast_possible_truncation)]
                let lambda = (1.0 - diversity) as f32;
                opts = opts.with_mmr(lambda);
            }
        }
        if let Some(raw) = params.get("label") {
            let raw = raw.as_str().ok_or_else(|| {
                anyhow::anyhow!("`label` must be a comma-separated string, got {raw}")
            })?;
            let (labels, names) = parse_labels(raw)?;
            if !labels.is_empty() {
                applied.insert("labels".to_string(), serde_json::json!(names));
                opts = opts.with_labels(labels);
            }
        }
        if let Some(raw) = params.get("regex") {
            let pattern = raw
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("`regex` must be a string pattern, got {raw}"))?
                .trim();
            if !pattern.is_empty() {
                applied.insert("regex".to_string(), serde_json::json!(pattern));
                opts = opts.with_regex(pattern);
            }
        }
        if let Some(raw) = params.get("candidate_multiplier") {
            let multiplier = parse_candidate_multiplier(raw)?;
            applied.insert(
                "candidate_multiplier".to_string(),
                serde_json::json!(multiplier),
            );
            opts.candidate_multiplier = multiplier;
        }
        let (hits, cascade) = self.with_read(|store| {
            store
                .search_report(query, &opts)
                .map_err(|err| anyhow::anyhow!("search failed: {err}"))
        })?;
        let mut lines = Vec::new();
        if let Some(report) = &indexed_now {
            lines.push(cold_build_note(report));
        }
        // The cascade decides which channels may run; say so when it pruned one, so a
        // narrow result is attributable to the query's shape instead of looking like a
        // miss. A plan that runs everything is the default and stays unmentioned.
        if !cascade.skipped.is_empty() {
            let skipped: Vec<String> = cascade
                .skipped
                .iter()
                .map(|(tier, reason)| format!("{}({})", tier.as_str(), reason.as_str()))
                .collect();
            lines.push(format!(
                "[cascade: {} query, ran {}; skipped {}]",
                cascade.shape.as_str(),
                cascade
                    .running
                    .iter()
                    .map(|t| t.as_str())
                    .collect::<Vec<_>>()
                    .join("+"),
                skipped.join(","),
            ));
        }
        lines.push(format!("{} hit(s) for {query:?}:", hits.len()));
        for hit in &hits {
            lines.push(format!(
                "- {} ({}:{}) [{}] score={:.3}",
                hit.qualified_name,
                hit.file_path,
                hit.start_line,
                hit.label.as_str(),
                hit.score,
            ));
        }
        let results: Vec<serde_json::Value> = hits
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
        // `details.search` sits between the hits and the cascade: the store's
        // answer, then what was asked of it, then how the plan was chosen.
        let mut details = serde_json::Map::new();
        details.insert("hits".to_string(), serde_json::json!(hits.len()));
        details.insert("results".to_string(), serde_json::Value::Array(results));
        if !applied.is_empty() {
            // A knob was set, so the echo is useful: it leads with the effective
            // limit, which bounds every other knob, then the knobs themselves.
            let mut echo = serde_json::Map::new();
            echo.insert("limit".to_string(), serde_json::json!(opts.limit));
            echo.append(&mut applied);
            details.insert("search".to_string(), serde_json::Value::Object(echo));
        }
        details.insert(
            "cascade".to_string(),
            serde_json::json!({
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
            }),
        );
        Ok(AgentToolResult {
            content: vec![pa_agent::types::ToolResultContent::text(lines.join("\n"))],
            details: serde_json::Value::Object(details),
            terminate: None,
            is_error: false,
        })
    }

    /// FORK-ONLY (jev Phase 2): re-rank a `search` result with the judge.
    ///
    /// Fail-open by construction: without a re-ranker, with no judge answer,
    /// or with a result this code cannot read, the store's own order and
    /// rendering pass through untouched. A demoted candidate is KEPT in the
    /// list, moved below the survivors and labelled with the probability that
    /// demoted it - the model sees the judge's opinion, not a silent
    /// deletion. A candidate beyond the judged top-K keeps its place and is
    /// marked unjudged, never `kept`.
    async fn rerank_search(
        &self,
        query: &str,
        mut result: AgentToolResult,
        signal: &AbortSignal,
    ) -> AgentToolResult {
        let Some(reranker) = self.reranker.as_ref() else {
            return result;
        };
        let Some(rows) = result
            .details
            .get("results")
            .and_then(|value| value.as_array())
        else {
            return result;
        };
        let candidates: Vec<RerankCandidate> = rows
            .iter()
            .enumerate()
            .map(|(index, row)| RerankCandidate {
                index,
                qualified_name: string_field(row, "qualified_name"),
                file_path: string_field(row, "file_path"),
                label: string_field(row, "label"),
            })
            .collect();
        // A cancelled call must not wait for the judgment: the abort wins
        // and the store's own result returns (fail-open, like any failure).
        let judged = tokio::select! {
            biased;
            () = signal.aborted() => None,
            outcome = reranker.rerank(query, &candidates) => outcome,
        };
        let Some(outcome) = judged else {
            return result;
        };
        // Rebuild the rendering in the re-ranked order: survivors first (in
        // their original relative order), then the demoted ones.
        let demoted: std::collections::BTreeMap<usize, f64> =
            outcome.demoted.iter().copied().collect();
        let mut order: Vec<usize> = outcome.kept.clone();
        order.extend(outcome.demoted.iter().map(|(index, _probability)| *index));
        let mut lines = vec![format!(
            "{} hit(s) for {query:?} (re-ranked by jev: {} of {} judged, {} demoted):",
            rows.len(),
            outcome.probabilities.len(),
            rows.len(),
            outcome.demoted.len()
        )];
        let mut reordered = Vec::with_capacity(rows.len());
        for index in order {
            let Some(row) = rows.get(index) else {
                continue;
            };
            let mut row = row.clone();
            let probability = outcome.probabilities.get(&index).copied();
            let demoted_at = demoted.get(&index).copied();
            if let Some(object) = row.as_object_mut() {
                // `kept`, not `verified`: the hit cleared a 0.1 threshold, which
                // means the judge did not confidently reject it - not that it
                // confirmed it (audit F18). `judged` stays beside it so "the
                // judge never looked" is distinguishable from "the judge
                // rejected it".
                object.insert(
                    "kept".to_string(),
                    serde_json::Value::Bool(probability.is_some() && demoted_at.is_none()),
                );
                object.insert(
                    "judged".to_string(),
                    serde_json::Value::Bool(probability.is_some()),
                );
                if let Some(probability) = probability {
                    object.insert("relevance".to_string(), serde_json::json!(probability));
                }
            }
            lines.push(render_hit_line(&row, demoted_at));
            reordered.push(row);
        }
        if let Some(details) = result.details.as_object_mut() {
            details.insert(
                VERIFIED_MARKER.to_string(),
                serde_json::json!({
                    "judged": outcome.probabilities.len(),
                    "demoted": outcome.demoted.len(),
                    "threshold": crate::jev::graph_rerank::DEMOTE_BELOW,
                }),
            );
            details.insert("results".to_string(), serde_json::Value::Array(reordered));
        }
        result.content = vec![pa_agent::types::ToolResultContent::text(lines.join("\n"))];
        result
    }

    fn action_trace(&self, params: &serde_json::Value) -> anyhow::Result<AgentToolResult> {
        let symbol = params
            .get("symbol")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|symbol| !symbol.is_empty())
            .ok_or_else(|| anyhow::anyhow!("trace requires a non-empty `symbol`"))?;
        let root = self.resolve_root(params);
        let project = project_name(&root);
        let indexed_now = self.ensure_indexed(&root, &project)?;
        let mut opts = TraceOptions::new(&project, symbol);
        opts.direction =
            parse_direction(params.get("direction").and_then(serde_json::Value::as_str));
        if let Some(depth) = params.get("depth").and_then(serde_json::Value::as_u64) {
            opts.depth = depth as usize;
        }
        if let Some(limit) = params.get("limit").and_then(serde_json::Value::as_u64) {
            opts.limit = limit as usize;
        }
        let edge_types =
            parse_edge_types(params.get("edge_types").and_then(serde_json::Value::as_str));
        if !edge_types.is_empty() {
            opts.edge_types = edge_types;
        }
        // The store already computed a confidence per step and already labelled
        // the weak ones; nothing could ask it to hide them. A floor is a filter
        // over that existing number, not a new signal, so it costs one comparison
        // per step and lets a caller read only the edges worth trusting.
        let confidence_floor = parse_confidence_floor(params)?;
        if let Some(floor) = confidence_floor {
            // In `[0, 1]` by `parse_confidence_floor`, so the narrowing cast keeps
            // every value the caller can write.
            #[allow(clippy::cast_possible_truncation)]
            let floor = floor as f32;
            opts = opts.with_confidence_floor(Some(floor));
        }
        let result = self.with_read(|store| {
            store
                .trace(&opts)
                .map_err(|err| anyhow::anyhow!("trace failed: {err}"))
        })?;
        let mut lines = Vec::new();
        if let Some(report) = &indexed_now {
            lines.push(cold_build_note(report));
        }
        lines.push(format!(
            "Trace from {} ({} step(s)){}{}:",
            result.origin,
            result.steps.len(),
            if result.truncated {
                format!(
                    " — TRUNCATED at the {} node limit; more reachable nodes were not listed, \
                     raise `limit` for the full set",
                    result.steps.len()
                )
            } else {
                String::new()
            },
            // A floor makes a short listing ambiguous unless the drops are named:
            // "2 steps" from a symbol that calls twenty reads as a small symbol.
            if result.filtered > 0 {
                format!(
                    " — {} step(s) below the confidence floor were filtered out;                      lower `confidence_floor` to see them",
                    result.filtered
                )
            } else {
                String::new()
            },
        ));
        for step in &result.steps {
            lines.push(format!(
                "- [depth {}] {} ({}:{}) via {} confidence={:.2}{}",
                step.depth,
                step.qualified_name,
                step.file_path,
                step.start_line,
                step.via.as_str(),
                step.confidence,
                // An ambiguous edge is a guess among same-named definitions. Say so:
                // measured worst case here is `new` with 400 candidates. GitNexus
                // records this exact shape as its "wrong edge at full confidence" bug.
                if step.candidates > 1 {
                    format!(" (ambiguous: {} candidates)", step.candidates)
                } else {
                    String::new()
                },
            ));
        }
        let steps: Vec<serde_json::Value> = result
            .steps
            .iter()
            .map(|step| {
                serde_json::json!({
                    "id": step.id,
                    "name": step.name,
                    "qualified_name": step.qualified_name,
                    "file_path": step.file_path,
                    "start_line": step.start_line,
                    "depth": step.depth,
                    "via": step.via.as_str(),
                    "confidence": step.confidence,
                    "strategy": step.strategy,
                    "candidates": step.candidates,
                    "risk_labels": step.risk_labels,
                })
            })
            .collect();
        let mut details = serde_json::Map::new();
        details.insert("origin_id".to_string(), serde_json::json!(result.origin_id));
        details.insert("origin".to_string(), serde_json::json!(result.origin));
        details.insert("steps".to_string(), serde_json::Value::Array(steps));
        details.insert("truncated".to_string(), serde_json::json!(result.truncated));
        // Only a call that set a floor carries the echo, so a plain trace keeps
        // exactly the answer it had before the parameter existed.
        if let Some(floor) = confidence_floor {
            details.insert("confidence_floor".to_string(), serde_json::json!(floor));
            details.insert("filtered".to_string(), serde_json::json!(result.filtered));
        }
        Ok(AgentToolResult {
            content: vec![pa_agent::types::ToolResultContent::text(lines.join("\n"))],
            details: serde_json::Value::Object(details),
            terminate: None,
            is_error: false,
        })
    }

    fn action_architecture(&self, params: &serde_json::Value) -> anyhow::Result<AgentToolResult> {
        let root = self.resolve_root(params);
        let project = project_name(&root);
        let indexed_now = self.ensure_indexed(&root, &project)?;
        let view = ArchitectureView::from_str_or_default(
            params
                .get("view")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("overview"),
        );
        let arch = self.with_read(|store| {
            store
                .architecture(&project, view)
                .map_err(|err| anyhow::anyhow!("architecture failed: {err}"))
        })?;
        // Providers forward only `content` to the model; `details` stays for
        // tests and the UI. The report body therefore rides in `content`,
        // the same shape `search` and `trace` already use. Only the section
        // for the requested view is populated, so iterating every section
        // renders exactly that view.
        let mut lines = Vec::new();
        if let Some(report) = &indexed_now {
            lines.push(cold_build_note(report));
        }
        lines.push(format!(
            "Architecture view `{}` for project `{}`:",
            view.as_str(),
            arch.project,
        ));
        if let Some(overview) = &arch.overview {
            lines.push(format!(
                "overview: {} node(s), {} edge(s), {} file(s); labels [{}], edge types [{}], \
                 languages [{}]",
                overview.nodes,
                overview.edges,
                overview.files,
                render_counts(&overview.labels),
                render_counts(&overview.edge_types),
                render_counts(&overview.languages),
            ));
        }
        if !arch.structure.is_empty() {
            lines.push(format!("{} module edge(s):", arch.structure.len()));
            for edge in &arch.structure {
                lines.push(format!(
                    "- {} -> {} (weight={}, {})",
                    edge.from,
                    edge.to,
                    edge.weight,
                    edge.kinds.join("+"),
                ));
            }
        }
        if !arch.layers.is_empty() {
            lines.push(format!("{} layer(s):", arch.layers.len()));
            for layer in &arch.layers {
                lines.push(format!("layer {}: {}", layer.level, layer.files.join(", ")));
            }
        }
        if !arch.clusters.is_empty() {
            lines.push(format!("{} cluster(s):", arch.clusters.len()));
            for cluster in &arch.clusters {
                lines.push(format!(
                    "cluster {} ({} internal edge(s)): {}",
                    cluster.id,
                    cluster.internal_edges,
                    cluster.files.join(", "),
                ));
            }
        }
        if !arch.cycles.is_empty() {
            lines.push(format!("{} cycle(s):", arch.cycles.len()));
            for cycle in &arch.cycles {
                lines.push(format!("cycle: {}", cycle.files.join(" -> ")));
                for edge in &cycle.edges {
                    lines.push(format!(
                        "  {} -> {} (weight={})",
                        edge.from, edge.to, edge.weight,
                    ));
                }
            }
        }
        if !arch.hotspots.is_empty() {
            lines.push(format!("{} hotspot(s):", arch.hotspots.len()));
            for hotspot in &arch.hotspots {
                lines.push(format!(
                    "- {} [{}] fan_in={} fan_out={} degree={}",
                    hotspot.qualified_name,
                    hotspot.file_path,
                    hotspot.fan_in,
                    hotspot.fan_out,
                    hotspot.degree,
                ));
            }
        }
        if !arch.entry_points.is_empty() {
            lines.push(format!("{} entry point(s):", arch.entry_points.len()));
            for entry in &arch.entry_points {
                lines.push(format!(
                    "- {} [{}] out_degree={}",
                    entry.qualified_name, entry.file_path, entry.out_degree,
                ));
            }
        }
        if let Some(tree) = &arch.file_tree {
            render_file_tree(tree, 0, &mut lines);
        }
        // A `structure` view on a large repo is one line per module edge, so
        // the report gets the same byte ceiling as bash output and says so
        // when it is cut.
        let mut report = lines.join("\n");
        let (capped, was_truncated) = crate::resources::truncate_context_content(
            &report,
            crate::tools::truncate::DEFAULT_MAX_BYTES,
        );
        if was_truncated {
            report = format!(
                "{capped}\n[architecture report truncated at {} bytes; query a narrower view]",
                crate::tools::truncate::DEFAULT_MAX_BYTES,
            );
        }
        Ok(AgentToolResult {
            content: vec![pa_agent::types::ToolResultContent::text(report)],
            details: serde_json::to_value(&arch).unwrap_or(serde_json::Value::Null),
            terminate: None,
            is_error: false,
        })
    }

    fn action_coverage(&self, params: &serde_json::Value) -> anyhow::Result<AgentToolResult> {
        let root = self.resolve_root(params);
        let project = project_name(&root);
        let report = self.with_read(|store| {
            store
                .coverage(&project)
                .map_err(|err| anyhow::anyhow!("coverage failed: {err}"))
        })?;
        Ok(AgentToolResult {
            content: vec![pa_agent::types::ToolResultContent::text(format!(
                "Coverage for `{project}`: {}/{} files parsed cleanly ({:.0}%), {} partial, \
                 {} unusable, {} skipped; complete={}.",
                report.parsed,
                report.total,
                report.clean_ratio() * 100.0,
                report.partial,
                report.unusable,
                report.skipped,
                report.complete,
            ))],
            details: serde_json::to_value(&report).unwrap_or(serde_json::Value::Null),
            terminate: None,
            is_error: false,
        })
    }

    fn action_changes(&self, params: &serde_json::Value) -> anyhow::Result<AgentToolResult> {
        let root = self.resolve_root(params);
        let project = project_name(&root);
        let ref_a = params
            .get("ref_a")
            .or_else(|| params.get("refA"))
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or("HEAD~1");
        let ref_b = params
            .get("ref_b")
            .or_else(|| params.get("refB"))
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or("HEAD");
        let report = self.with_read(|store| {
            store
                .detect_changes(&project, ref_a, ref_b)
                .map_err(|err| anyhow::anyhow!("changes failed: {err}"))
        })?;
        Ok(AgentToolResult {
            content: vec![pa_agent::types::ToolResultContent::text(format!(
                "Changes in `{project}` ({ref_a}..{ref_b}): {} file(s), {} affected symbol(s), \
                 {} co-change pair(s); git={}.",
                report.file_count(),
                report.symbol_count(),
                report.co_changes.len(),
                report.is_git,
            ))],
            details: serde_json::to_value(&report).unwrap_or(serde_json::Value::Null),
            terminate: None,
            is_error: false,
        })
    }

    /// `adr`: record or list Architecture Decision Records.
    ///
    /// ADRs live in the same SQLite file as the graph, so a decision and the
    /// code it describes cannot drift apart between files. This is the `adr`
    /// module's first consumer; it was written in 39c66260f and never wired.
    fn action_adr(&self, params: &serde_json::Value) -> anyhow::Result<AgentToolResult> {
        let root = self.resolve_root(params);
        let project = project_name(&root);
        let record = params.get("title").and_then(serde_json::Value::as_str);
        let text = |key: &str| {
            params
                .get(key)
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string()
        };
        // The list is scoped: this project's decisions by default, the global
        // bucket with `scope: "global"`, everything with `scope: "all"`.
        let query = match params
            .get("scope")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .map(str::to_ascii_lowercase)
            .as_deref()
        {
            Some("all") => pa_graph::adr::AdrQuery::new(),
            Some("global") => pa_graph::adr::AdrQuery::new().in_project(String::new()),
            _ => pa_graph::adr::AdrQuery::new().in_project(project.clone()),
        };
        let query = match params.get("status").and_then(serde_json::Value::as_str) {
            Some(status) => {
                query.with_status(pa_graph::adr::AdrStatus::from_str_or_default(status))
            }
            None => query,
        };
        // `id` reads one decision's full body; listing stays a summary.
        if let Some(id) = params.get("id").and_then(serde_json::Value::as_i64) {
            let found = self.with_reader(|store| {
                let adrs = pa_graph::adr::AdrStore::new(store.connection())
                    .map_err(|err| anyhow::anyhow!("adr store: {err}"))?;
                adrs.get_adr(id)
                    .map_err(|err| anyhow::anyhow!("get adr: {err}"))
            })?;
            let Some(adr) = found else {
                anyhow::bail!("adr #{id} not found");
            };
            let mut body = format!(
                "#{} [{}] {}\n\nContext:\n{}\n\nDecision:\n{}\n\nConsequences:\n{}\n",
                adr.id,
                adr.status.as_str(),
                adr.title,
                adr.context.trim(),
                adr.decision.trim(),
                adr.consequences.trim()
            );
            if let Some(supersedes) = adr.supersedes {
                body.push_str(&format!("\nSupersedes: #{supersedes}\n"));
            }
            if !adr.project.is_empty() {
                body.push_str(&format!("Project: {}\n", adr.project));
            }
            return Ok(AgentToolResult {
                content: vec![pa_agent::types::ToolResultContent::text(body)],
                details: serde_json::json!({
                    "id": adr.id,
                    "title": adr.title,
                    "status": adr.status.as_str(),
                    "context": adr.context,
                    "decision": adr.decision,
                    "consequences": adr.consequences,
                    "supersedes": adr.supersedes,
                    "project": adr.project,
                }),
                terminate: None,
                is_error: false,
            });
        }
        // Recording writes, so it holds the store's lock. Listing reads only the
        // `adr` table — no graph content — so it takes the reader unconditionally
        // and can be answered during an index run.
        let rows = match record {
            Some(title) => self.with_store(|store| {
                let adrs = pa_graph::adr::AdrStore::new(store.connection())
                    .map_err(|err| anyhow::anyhow!("adr store: {err}"))?;
                let mut adr = pa_graph::adr::Adr::new(
                    title,
                    text("context"),
                    text("decision"),
                    text("consequences"),
                )
                // A decision belongs to the project it was made in: the graph DB
                // holds several projects, and one project's history is not
                // another's memory. `scope: "global"` records an install-wide
                // decision (the empty legacy bucket is only for rows written
                // before the column existed).
                .in_project(
                    if params
                        .get("scope")
                        .and_then(serde_json::Value::as_str)
                        .is_some_and(|scope| scope.eq_ignore_ascii_case("global"))
                    {
                        String::new()
                    } else {
                        project.clone()
                    },
                );
                if let Some(status) = params.get("status").and_then(serde_json::Value::as_str) {
                    adr = adr.with_status(pa_graph::adr::AdrStatus::from_str_or_default(status));
                }
                if let Some(supersedes) =
                    params.get("supersedes").and_then(serde_json::Value::as_i64)
                {
                    adr = adr.superseding(supersedes);
                }
                adrs.record_adr(&adr)
                    .map_err(|err| anyhow::anyhow!("record adr: {err}"))?;
                adrs.query_adr(&query)
                    .map_err(|err| anyhow::anyhow!("query adr: {err}"))
            })?,
            None => self.with_reader(|store| {
                let adrs = pa_graph::adr::AdrStore::new(store.connection())
                    .map_err(|err| anyhow::anyhow!("adr store: {err}"))?;
                adrs.query_adr(&query)
                    .map_err(|err| anyhow::anyhow!("query adr: {err}"))
            })?,
        };
        let mut lines = vec![format!(
            "ADRs for `{}`: {} record(s).",
            project_name(&root),
            rows.len()
        )];
        for adr in &rows {
            lines.push(format!(
                "- #{} [{}] {}",
                adr.id,
                adr.status.as_str(),
                adr.title
            ));
        }
        Ok(AgentToolResult {
            content: vec![pa_agent::types::ToolResultContent::text(lines.join("\n"))],
            details: serde_json::json!({ "count": rows.len() }),
            terminate: None,
            is_error: false,
        })
    }

    /// `verify`: check the run receipts, and the quality of the last run.
    ///
    /// The three modules this reports on — `witness` (receipts), `quality`
    /// (envelope) and `proof_gate` (decision) — were written together and had
    /// no consumer until now. Every input is a stored hash or a measured
    /// number, so the verdict is not a self-assessment.
    fn action_verify(&self, params: &serde_json::Value) -> anyhow::Result<AgentToolResult> {
        let root = self.resolve_root(params);
        let project = project_name(&root);
        let verdict = self.with_read(|store| {
            store
                .verify_run(&project)
                .map_err(|err| anyhow::anyhow!("verify failed: {err}"))
        })?;
        let decision = if verdict.decision.is_accept() {
            "accept".to_string()
        } else {
            verdict.decision.reason().map_or_else(
                || "reject (unknown)".to_string(),
                |r| format!("reject ({})", r.as_str()),
            )
        };
        let flags: Vec<&str> = verdict.envelope.flags.iter().map(|f| f.as_str()).collect();
        let flags = if flags.is_empty() {
            "none".to_string()
        } else {
            flags.join(", ")
        };
        Ok(AgentToolResult {
            content: vec![pa_agent::types::ToolResultContent::text(format!(
                "Verify `{project}`: {decision} over {} run receipt(s), chain {}; \
                 quality samples {} (min {:.2}, median {:.2}, max {:.2}), flags: {flags}.",
                verdict.receipts,
                if verdict.chain_ok { "intact" } else { "BROKEN" },
                verdict.envelope.count,
                verdict.envelope.min,
                verdict.envelope.median,
                verdict.envelope.max,
            ))],
            details: serde_json::to_value(&verdict).unwrap_or(serde_json::Value::Null),
            terminate: None,
            is_error: false,
        })
    }

    fn action_status(&self, params: &serde_json::Value) -> anyhow::Result<AgentToolResult> {
        let root = self.resolve_root(params);
        let project = project_name(&root);
        let (nodes, edges, files) = self.with_read(|store| {
            let nodes = store
                .all_nodes(&project)
                .map_err(|err| anyhow::anyhow!("status failed: {err}"))?
                .len();
            let edges = store
                .all_edges(&project)
                .map_err(|err| anyhow::anyhow!("status failed: {err}"))?
                .len();
            let files = store
                .file_count(&project)
                .map_err(|err| anyhow::anyhow!("status failed: {err}"))?;
            Ok((nodes, edges, files))
        })?;
        Ok(AgentToolResult {
            content: vec![pa_agent::types::ToolResultContent::text(format!(
                "Graph store {} — project `{project}`: {nodes} node(s), {edges} edge(s), \
                 {files} file(s).",
                self.db_path.display(),
            ))],
            details: serde_json::json!({
                "db_path": self.db_path.display().to_string(),
                "project": project,
                "nodes": nodes,
                "edges": edges,
                "files": files,
            }),
            terminate: None,
            is_error: false,
        })
    }
}

impl AgentTool for GraphTool {
    fn name(&self) -> &'static str {
        "graph"
    }

    fn description(&self) -> &'static str {
        "Query and build the on-disk code graph for the working directory. \
         Actions: index (full or incremental build of the symbol/call/import \
         graph; index_depth = skeleton|symbols|full, default full; \
         respect_gitignore = default false, true hides ignored files), reindex \
         (incremental closure repair), search (hybrid BM25/regex/degree symbol \
         search; mmr, label, regex and candidate_multiplier tune it), trace \
         (callers/callees from a symbol; depth = BFS hops, \
         default 2), architecture (overview/structure/layers/clusters/cycles/\
         hotspots/entry_points/file_tree), coverage (index honesty report), \
         changes (git-diff → affected symbols), status (store counts), verify \
         (run receipts + quality of the last index run), adr (record or list \
         architecture decisions). Each action reads only its own parameters: \
         an unknown or misspelt one is an error naming the accepted set, never \
         a silent default."
    }

    fn parameters(&self) -> &serde_json::Value {
        &self.schema
    }

    fn label(&self) -> &'static str {
        "Graph"
    }

    fn execute(
        self: Arc<Self>,
        _tool_call_id: String,
        params: serde_json::Value,
        signal: AbortSignal,
        _on_update: AgentToolUpdateCallback,
    ) -> pa_agent::BoxFut<'static, anyhow::Result<AgentToolResult>> {
        Box::pin(async move {
            let started = std::time::Instant::now();
            let tool = Arc::clone(&self);
            // The re-rank's query is read before the store work (the params
            // move into the blocking task).
            let rerank_query = (self.reranker.is_some()
                && params
                    .get("action")
                    .and_then(serde_json::Value::as_str)
                    .is_some_and(|action| action.trim().eq_ignore_ascii_case("search")))
            .then(|| {
                params
                    .get("query")
                    .and_then(serde_json::Value::as_str)
                    .map(str::trim)
                    .filter(|query| !query.is_empty())
                    .map(str::to_string)
            })
            .flatten();
            let store_params = params.clone();
            // SQLite, the directory walk, and the git subprocess are all
            // blocking; keep them off the async executor.
            let result = tokio::task::spawn_blocking(move || tool.dispatch(&store_params))
                .await
                .map_err(|join| anyhow::anyhow!("graph task join failed: {join}"))??;
            // FORK-ONLY (jev Phase 2): the judge re-rank runs on the async
            // path (it awaits the judgment); a failure inside it is
            // fail-open and returns the store's own result.
            let elapsed_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
            let Some(query) = rerank_query else {
                return Ok(result.with_duration_ms(elapsed_ms));
            };
            Ok(self
                .rerank_search(&query, result, &signal)
                .await
                .with_duration_ms(elapsed_ms))
        })
    }
}

/// One string field of a result row (`""` when absent or not a string).
fn string_field(row: &serde_json::Value, key: &str) -> String {
    row.get(key)
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// Render one re-ranked hit line: a demoted candidate carries the
/// probability that demoted it, a judged survivor is marked `kept`, and a
/// candidate the judge never looked at renders exactly like the store's own
/// line.
fn render_hit_line(row: &serde_json::Value, demoted_at: Option<f64>) -> String {
    let name = string_field(row, "qualified_name");
    let file = string_field(row, "file_path");
    let line = row
        .get("start_line")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or_default();
    let label = string_field(row, "label");
    let score = row
        .get("score")
        .and_then(serde_json::Value::as_f64)
        .unwrap_or_default();
    let judged = row
        .get("judged")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    let base = format!("- {name} ({file}:{line}) [{label}] score={score:.3}");
    match (demoted_at, judged) {
        (Some(probability), _) => format!("{base} demoted-by-jev (p={probability:.2})"),
        (None, true) => format!("{base} kept-by-jev"),
        (None, false) => base,
    }
}

/// The tool's JSON-Schema parameters.
#[must_use]
pub fn params_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "action": {
                "type": "string",
                "enum": [
                    "index", "reindex", "search", "trace",
                    "architecture", "coverage", "changes", "status",
                    "verify", "adr"
                ],
                "description": "The graph operation to run."
            },
            "path": {
                "type": "string",
                "description": "Project root to index/query, relative to the working directory (or absolute). Defaults to the working directory."
            },
            "title": {
                "type": "string",
                "description": "ADR title; when present the record is written, otherwise the list is returned."
            },
            "decision": {
                "type": "string",
                "description": "What was decided."
            },
            "context": {
                "type": "string",
                "description": "Why the decision was needed."
            },
            "consequences": {
                "type": "string",
                "description": "What follows from the decision."
            },
            "status": {
                "type": "string",
                "description": "Lifecycle status: proposed, accepted, rejected or superseded."
            },
            "query": {
                "type": "string",
                "description": "Symbol query for action=\"search\"."
            },
            "symbol": {
                "type": "string",
                "description": "Start symbol (qualified or unique short name) for action=\"trace\"."
            },
            "direction": {
                "type": "string",
                "enum": ["out", "in", "both"],
                "description": "Trace direction (default \"out\": what the symbol calls)."
            },
            "depth": {
                "type": "integer",
                "description": "Maximum trace BFS depth in hops for action=\"trace\" (default 2). Not the index build depth - that is index_depth."
            },
            "index_depth": {
                "type": "string",
                "enum": ["skeleton", "symbols", "full"],
                "description": "How deep to build on action=\"index\". skeleton records one node per file (~1 s on this repo) and resolves nothing; symbols adds definitions; full (the default) resolves calls and imports (~2 min). Plain \"depth\" is accepted as a legacy alias for index only."
            },
            "edge_types": {
                "type": "string",
                "description": "Comma-separated edge filter for trace, e.g. \"calls,imports\" (default calls)."
            },
            "confidence_floor": {
                "type": "number",
                "minimum": 0.0,
                "maximum": 1.0,
                "description": "action=\"trace\": keep only steps whose confidence is at least this, in [0, 1]. A step's confidence is the product of the edge confidences along its path from the start symbol, so it falls with depth and one guessed edge discounts everything reached through it. Each edge's baseline is set by how it was resolved: an imported callee 0.95, one in the same module 0.90, a known external 0.80, a project-wide unique name 0.75, a suffix guess 0.55. A floor of 0.6 therefore keeps direct calls and drops anything reached through a guess; because the values multiply, a deep path through only-resolved edges falls fastest, so lower the floor or check the printed per-step confidence before concluding a deep path is absent. The trace header states how many steps the floor removed. Omit to keep every step and only label the weak ones. An edge that recorded no confidence counts as fully confident and is never dropped: the indexer writes a confidence on every edge it resolves, so an absent one belongs to an edge that never claimed it."
            },
            "view": {
                "type": "string",
                "enum": [
                    "overview", "structure", "layers", "clusters",
                    "cycles", "hotspots", "entry_points", "file_tree"
                ],
                "description": "Architecture view for action=\"architecture\" (default overview)."
            },
            "limit": {
                "type": "integer",
                "description": "Result cap for search/trace."
            },
            "mmr": {
                "type": "number",
                "minimum": 0.0,
                "maximum": 1.0,
                "description": "action=\"search\": diversity reranking weight in [0, 1]. 0 keeps pure relevance and 1 keeps the most spread-out set; 0.5-0.7 is the useful band when the top hits are near-duplicate names (overloaded methods, generated wrappers) and you want one representative of each instead of five spellings of the same idea. Omit to keep the store's own order."
            },
            "label": {
                "type": "string",
                "description": "action=\"search\": comma-separated node labels to restrict the result to, e.g. \"function,method\" or \"struct\". Accepted: function, method, class, struct, enum, interface, module, file, folder, branch. Use it when the answer must be a definition of one kind (\"which structs do this\") instead of any symbol whose name matches. An unknown label is an error naming the accepted ones."
            },
            "regex": {
                "type": "string",
                "description": "action=\"search\": Rust regex applied as a filter over the hit's name, qualified name and file path, e.g. \"^render_\" or \"/crates/pa-graph/\". Use it to narrow a broad query when you know the spelling or the directory; it filters the ranked candidates rather than replacing the text query. An invalid pattern is an error carrying the compiler's message and position."
            },
            "candidate_multiplier": {
                "type": "integer",
                "minimum": 1,
                "description": "action=\"search\": how many candidates each retrieval channel fetches per requested hit before fusion (default 4, i.e. limit x 4). Raise it when a filter, regex or mmr drops candidates after ranking and the answer comes back short; it costs recall work, not accuracy."
            },
            "force": {
                "type": "boolean",
                "description": "Re-parse every file on action=\"index\", ignoring stored hashes."
            },
            "lease_wait": {
                "type": "integer",
                "minimum": 0,
                "description": "action=\"index\"/\"reindex\": how many milliseconds to wait for another session's index run to release the database's single writer before failing, instead of failing at once. A full pass over a large tree is measured at over two minutes, so a second session arriving mid-build usually wants to wait and then find the work already done. 0 means one attempt, then give up (the default). Raise it when two sessions may index the same project; the call then comes back with the finished graph instead of a timeout."
            },
            "extensions": {
                "type": "string",
                "description": "action=\"index\"/\"reindex\": comma-separated extension allowlist, e.g. \"rs,ts\". Case and a leading dot are ignored (.RS is rs). Use it to scope a build to one language in a mixed tree, so a query is not diluted by generated or vendored files in another language. An extension that matches nothing is NOT an error - the answer says which ones matched no file, because an empty graph from a misspelled extension would otherwise read as an empty codebase."
            },
            "max_file_bytes": {
                "type": "integer",
                "minimum": 1,
                "description": "action=\"index\"/\"reindex\": largest file to index, in bytes (default 2 MiB). Use it to keep a huge generated or minified file out of the graph. On `index` the file is read just far enough to measure it, then skipped: `skipped` counts it and the coverage notes name it as `oversized: N bytes`, so a symbol that is missing from the graph has a stated reason. `reindex` applies the same cap to its repair set and leaves such a file alone rather than re-parsing it, but reports no per-file skip."
            },
            "respect_gitignore": {
                "type": "boolean",
                "description": "action=\"index\"/\"reindex\": whether to hide files the project's gitignore rules exclude (.gitignore, .git/info/exclude, the global gitignore, and those of parent directories). Default false: index what is on disk, so a scratch file or deliberately ignored but authored code is visible and searchable. true is the lean view: where the ignored set is build output or copies of the same sources, it removes the duplicate hits and most of the walk. The mode decides the discovered set, so a run in the other mode adds or removes the files only it can see (`removed` counts them). .git is always skipped; `.ignore` files are honoured in both modes. On a large tree, measure both before choosing: the difference can be an order of magnitude of files."
            },
            "supersedes": {
                "type": "integer",
                "description": "action=\"adr\": id of an earlier decision this one replaces."
            },
            "ref_a": {
                "type": "string",
                "description": "Base git ref for action=\"changes\" (default HEAD~1)."
            },
            "ref_b": {
                "type": "string",
                "description": "Head git ref for action=\"changes\" (default HEAD)."
            }
        },
        "required": ["action"]
    })
}

#[cfg(test)]
mod tests {
    use super::*;

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
                "respect_gitignore",
                "status",
                "supersedes",
                "symbol",
                "title",
                "view",
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
    /// reachable from the tool: `index` defaults to what is on disk, `true` is the
    /// lean view, and `details` says which mode ran so the counts are readable.
    #[tokio::test]
    async fn index_indexes_ignored_files_unless_asked_not_to() {
        let wide_dir = tempfile::tempdir().unwrap();
        ignored_fixture(wide_dir.path());
        let wide = run_action(wide_dir.path(), "index", serde_json::json!({}))
            .await
            .expect("index");
        assert_eq!(
            wide.details["added"], 3,
            "the default indexes what is on disk: {}",
            wide.details
        );
        assert_eq!(
            wide.details["respect_gitignore"], false,
            "the effective mode is echoed: {}",
            wide.details
        );
        assert!(
            text_of(&wide).contains("3 files parsed"),
            "{}",
            text_of(&wide)
        );

        let lean_dir = tempfile::tempdir().unwrap();
        ignored_fixture(lean_dir.path());
        let lean = run_action(
            lean_dir.path(),
            "index",
            serde_json::json!({ "respect_gitignore": true }),
        )
        .await
        .expect("index");
        assert_eq!(
            lean.details["added"], 1,
            "the lean mode hides the ignored files: {}",
            lean.details
        );
        assert_eq!(lean.details["respect_gitignore"], true);
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
    }

    /// `reindex` shares `index`'s discovery, so the parameter decides its new-file
    /// set too: a file that appears under an ignore rule is picked up in the
    /// default mode and skipped in the lean one.
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

        let wide = run_action(dir.path(), "reindex", serde_json::json!({}))
            .await
            .expect("reindex");
        assert!(
            wide.details["reindexed_paths"]
                .as_array()
                .is_some_and(|paths| paths.iter().any(|p| p == "scratch/late.rs")),
            "the default mode finds the new ignored file: {}",
            wide.details
        );

        std::fs::write(
            dir.path().join("scratch/later.rs"),
            "pub fn later() -> u32 { 5 }\n",
        )
        .unwrap();
        let lean = run_action(
            dir.path(),
            "reindex",
            serde_json::json!({ "respect_gitignore": true }),
        )
        .await
        .expect("reindex");
        assert!(
            lean.details["reindexed_paths"]
                .as_array()
                .is_some_and(Vec::is_empty),
            "the lean mode does not see the new ignored file: {}",
            lean.details
        );
        assert_eq!(lean.details["respect_gitignore"], true);
        // ... and it pruned what the wide pass had indexed.
        assert_eq!(
            lean.details["removed"], 3,
            "the lean pass drops the files only it cannot see: {}",
            lean.details
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
    /// §4.1 (audit): the project key is the root's basename, so two checkouts
    /// named `service` share one row set. The write path refuses to overwrite a
    /// project with a different root, but the WARM read path answered from
    /// `file_count > 0` alone — a query for B silently read A's symbols. Each
    /// root is validated before any read now.
    #[tokio::test]
    async fn a_warm_query_for_another_root_with_the_same_basename_refuses() {
        let a = tempfile::tempdir().unwrap().path().join("service");
        let b = tempfile::tempdir().unwrap().path().join("service");
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        std::fs::write(a.join("lib.rs"), "pub fn only_in_a() -> u32 { 1 }\n").unwrap();
        std::fs::write(b.join("lib.rs"), "pub fn only_in_b() -> u32 { 2 }\n").unwrap();

        // One shared DB, as the default `graph.dbPath` gives.
        let db = tempfile::tempdir().unwrap();
        let db_path = db.path().join("graph.db");
        let tool_a = GraphTool::new(db_path.clone(), a.clone());
        tool_a
            .dispatch(&serde_json::json!({ "action": "index" }))
            .unwrap();

        // B is the same basename, a different root: the warm path must not
        // answer from A's rows.
        let tool_b = GraphTool::new(db_path, b.clone());
        // `dispatch` answers Err; the agent loop turns that into an
        // `is_error` tool result (agent_loop/tool_call.rs), so the refusal is
        // asserted at the dispatch contract here.
        let error = tool_b
            .dispatch(&serde_json::json!({ "action": "search", "query": "only_in_a" }))
            .expect_err("a query from another root must refuse, not read the other tree");
        let text = format!("{error:#}");
        assert!(
            text.contains("different root"),
            "the refusal must name the cause: {text}"
        );
        assert!(
            !text.contains("only_in_a"),
            "no symbol from the other tree may leak into the answer: {text}"
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

        let tool_a = GraphTool::new(db_path.clone(), a.clone());
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
        let tool_b = GraphTool::new(db_path.clone(), b.clone());
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
}
