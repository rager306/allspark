//! Argument parsers and the scope/notes helpers they feed.

use pa_graph::{Direction, EdgeType, GraphStore, IndexOptions, IndexReport, Label, TreeNode};
use std::path::Path;
/// Parse a trace direction (`in`/`out`/`both`); anything else is outgoing.
pub(crate) fn parse_direction(raw: Option<&str>) -> Direction {
    match raw.map(str::trim).map(str::to_ascii_lowercase).as_deref() {
        Some("in" | "incoming" | "callers") => Direction::In,
        Some("both" | "all") => Direction::Both,
        _ => Direction::Out,
    }
}

/// Render a count map as `key=value` pairs for report lines.
pub(crate) fn render_counts(counts: &std::collections::BTreeMap<String, usize>) -> String {
    counts
        .iter()
        .map(|(key, count)| format!("{key}={count}"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Append one file-tree directory (and its children, recursively) to the
/// report lines.
pub(crate) fn render_file_tree(node: &TreeNode, depth: usize, lines: &mut Vec<String>) {
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
pub(crate) fn parse_edge_types(raw: Option<&str>) -> Vec<EdgeType> {
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
pub(crate) const SEARCH_LABELS: &[(&str, Label)] = &[
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
pub(crate) fn parse_mmr(raw: &serde_json::Value) -> anyhow::Result<f64> {
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
pub(crate) fn parse_labels(raw: &str) -> anyhow::Result<(Vec<Label>, Vec<String>)> {
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

/// Read `respect_gitignore` as a boolean, defaulting to `true`.
///
/// The LEAN view is the default for every path a user did not spell out: the
/// implicit cold-start build already runs `respect_gitignore(true)`, and an
/// explicit `index` that defaults the other way re-admits exactly the files the
/// implicit build refuses — the tools report measured a first index swallowing
/// 763 files of vendored packages, a docker copy and gitignored state. `false`
/// stays one parameter away for a caller who wants ignored-but-authored code.
/// A non-boolean value is an error rather than a silent default, because the two
/// modes produce different file sets and a caller that wrote `"true"` would
/// otherwise index the other one without learning it.
pub(crate) fn parse_respect_gitignore(params: &serde_json::Value) -> anyhow::Result<bool> {
    match params.get("respect_gitignore") {
        None => Ok(true),
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
pub(crate) fn with_effective_ignore(
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
pub(crate) fn parse_confidence_floor(params: &serde_json::Value) -> anyhow::Result<Option<f64>> {
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
pub(crate) fn parse_lease_wait(params: &serde_json::Value) -> anyhow::Result<Option<u64>> {
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
pub(crate) fn parse_extensions(params: &serde_json::Value) -> anyhow::Result<Option<Vec<String>>> {
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
pub(crate) fn parse_max_file_bytes(params: &serde_json::Value) -> anyhow::Result<Option<u64>> {
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
pub(crate) fn unmatched_extensions(
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
pub(crate) struct IndexScope {
    /// Milliseconds to wait for another index run's writer lease.
    lease_wait: Option<u64>,
    /// Extension allowlist, normalised.
    extensions: Option<Vec<String>>,
    /// Largest file to index.
    max_file_bytes: Option<u64>,
}

impl IndexScope {
    /// Read the three knobs from a call's parameters.
    pub(crate) fn read(params: &serde_json::Value) -> anyhow::Result<Self> {
        Ok(Self {
            lease_wait: parse_lease_wait(params)?,
            extensions: parse_extensions(params)?,
            max_file_bytes: parse_max_file_bytes(params)?,
        })
    }

    /// Apply the knobs the caller set to `opts`.
    pub(crate) fn apply(&self, mut opts: IndexOptions) -> IndexOptions {
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
    pub(crate) fn details(
        &self,
        unmatched: &[String],
    ) -> serde_json::Map<String, serde_json::Value> {
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
    pub(crate) fn unmatched(
        &self,
        store: &GraphStore,
        project: &str,
    ) -> anyhow::Result<Vec<String>> {
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
pub(crate) fn unmatched_note(unmatched: &[String]) -> Option<String> {
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
pub(crate) fn cold_build_note(report: &IndexReport) -> String {
    format!(
        "The graph was empty, so this call indexed the project first: {} nodes, {} edges, \
         {} files parsed. This build is lean: files the project's gitignore rules exclude are \
         not in it (run `index` to include them).",
        report.store_nodes, report.edges, report.parsed,
    )
}

/// Whether the store's file set diverges from the working tree.
///
/// The cheap pair (`mtime_ns`, `size`) decides first; a file whose pair matches is
/// assumed unchanged, which is the same rule the incremental pass uses.
/// Missing/unreadable files count as stale (the walk will drop them).
pub(crate) fn root_is_stale(
    root: &Path,
    stored: &std::collections::HashMap<String, pa_graph::hash::FileHash>,
) -> bool {
    for (rel, hash) in stored {
        let Ok(meta) = std::fs::metadata(root.join(rel)) else {
            return true;
        };
        let mtime_ns = meta
            .modified()
            .ok()
            .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|delta| delta.as_nanos() as i64)
            .unwrap_or_default();
        if meta.len() as i64 != hash.size || mtime_ns != hash.mtime_ns {
            return true;
        }
    }
    false
}

/// Most candidates one search may fuse, however large the multiplier asks.
///
/// The pool directly sizes twice as many FTS `LIMIT`s, and a saturated pool casts to a
/// negative `i64` that SQLite reads as `LIMIT -1` — unbounded. Same precedent as
/// `PACK_MAX_SYMBOLS`: a model-facing knob needs a ceiling the model cannot raise.
pub const SEARCH_MAX_POOL: usize = 512;

/// Most results one search may return.
pub const SEARCH_MAX_LIMIT: usize = 200;

/// Most matches one structural (`pattern`) scan returns, and its default.
///
/// A structural pattern is broader than a text query by nature (`$A.foo($$$)` matches
/// every call site of one method), and each hit is a whole source snippet, so the ceiling
/// is the same as `search`'s: a model-facing knob needs a bound it cannot raise.
pub const PATTERN_DEFAULT_LIMIT: usize = SEARCH_MAX_LIMIT;

/// Read `candidate_multiplier` as a pool factor, clamped so `limit * multiplier` fits.
///
/// The multiplier is model-controlled and sizes real work, so it is clamped rather than
/// trusted: a multiplier that would push the pool past [`SEARCH_MAX_POOL`] is reduced to the
/// largest one that fits. The `bool` says the request was reduced, so the answer can report
/// it instead of silently doing less than asked.
pub(crate) fn parse_candidate_multiplier(
    raw: &serde_json::Value,
    limit: usize,
) -> anyhow::Result<(usize, bool)> {
    let multiplier = raw
        .as_u64()
        .filter(|multiplier| *multiplier >= 1)
        .ok_or_else(|| {
            anyhow::anyhow!("`candidate_multiplier` must be an integer >= 1, got {raw}")
        })?;
    let asked = usize::try_from(multiplier)
        .map_err(|_| anyhow::anyhow!("`candidate_multiplier` is too large: {multiplier}"))?;
    let ceiling = (SEARCH_MAX_POOL / limit.max(1)).max(1);
    let applied = asked.min(ceiling);
    Ok((applied, applied != asked))
}

/// One string field of a result row (`""` when absent or not a string).
pub(crate) fn string_field(row: &serde_json::Value, key: &str) -> String {
    row.get(key)
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// Render one re-ranked hit line: a demoted candidate carries the
/// probability that demoted it, a judged survivor is marked `kept`, and a
/// candidate the judge never looked at renders exactly like the store's own
/// line.
pub(crate) fn render_hit_line(row: &serde_json::Value, demoted_at: Option<f64>) -> String {
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
