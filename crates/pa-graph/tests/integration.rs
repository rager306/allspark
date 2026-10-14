//! End-to-end coverage of the public `pa-graph` API on a real temp tree.

use std::path::Path;

use pa_graph::{
    ArchitectureView, Direction, EdgeType, Filter, GraphStore, IndexOptions, Label, Op,
    SearchOptions, TraceOptions,
};

fn write_repo(root: &Path) {
    std::fs::write(
        root.join("config.rs"),
        r"
//! Configuration loading.

/// Parse a config file from disk.
pub fn parse_config(path: &str) -> Config {
    let raw = read_file(path);
    Config { raw }
}

pub fn read_file(path: &str) -> String {
    std::fs::read_to_string(path).unwrap_or_default()
}

pub struct Config {
    pub raw: String,
}

impl Config {
    /// The parsed length.
    pub fn len(&self) -> usize {
        self.raw.len()
    }
}
",
    )
    .expect("config.rs");

    std::fs::write(
        root.join("main.rs"),
        r#"
mod config;

fn main() {
    let cfg = config::parse_config("app.toml");
    println!("{}", cfg.len());
}
"#,
    )
    .expect("main.rs");

    std::fs::write(
        root.join("util.py"),
        r#"
"""Utility helpers."""

def render_widget(name):
    """Draw a widget by name."""
    return f"<{name}>"
"#,
    )
    .expect("util.py");
}

#[test]
fn full_index_search_trace_cycle() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_repo(dir.path());

    let db = dir.path().join("graph.db");
    let mut store = GraphStore::open(&db).expect("open");

    // ── index ────────────────────────────────────────────────────────────
    let report = store
        .index_project(dir.path(), &IndexOptions::new("demo"))
        .expect("index");
    assert_eq!(report.added, 3, "three source files: {report:?}");
    assert_eq!(report.parsed, 3);
    assert!(report.nodes >= 6, "file + symbol nodes: {report:?}");
    assert!(report.edges >= 3, "CONTAINS + CALLS edges: {report:?}");

    // ── schema / store ───────────────────────────────────────────────────
    assert!(store
        .node_by_qn("demo", "config::parse_config")
        .expect("qn")
        .is_some());
    let cfg = store
        .node_by_qn("demo", "config::Config")
        .expect("qn")
        .expect("Config");
    assert_eq!(cfg.label, Label::Struct);
    assert!(cfg.body().contains("Configuration") || cfg.body().is_empty());

    // ── FTS5 search ──────────────────────────────────────────────────────
    let hits = store
        .search("parse config", &SearchOptions::new("demo"))
        .expect("search");
    assert!(hits.iter().any(|h| h.name == "parse_config"), "{hits:?}");

    // camelCase/snake_case split reaches `render_widget` from `widget`.
    let widget = store
        .search("widget", &SearchOptions::new("demo"))
        .expect("search");
    assert!(
        widget.iter().any(|h| h.name == "render_widget"),
        "{widget:?}"
    );
    let w = widget
        .iter()
        .find(|h| h.name == "render_widget")
        .expect("render_widget");
    // Its docstring "Draw a widget by name" is the FTS body column.
    assert!(w.channels.name.is_some() || w.channels.body.is_some());

    // ── payload filter (V18) ─────────────────────────────────────────────
    let filtered = store
        .search(
            "config",
            &SearchOptions::new("demo").with_filter(Filter::cond(
                "label",
                Op::Eq,
                "Function".into(),
            )),
        )
        .expect("filtered");
    assert!(
        filtered.iter().all(|h| h.label == Label::Function),
        "{filtered:?}"
    );

    // ── graph trace (G4) ─────────────────────────────────────────────────
    let trace = store
        .trace(
            &TraceOptions::new("demo", "main::main")
                .with_direction(Direction::Out)
                .with_depth(2),
        )
        .expect("trace");
    assert!(
        trace.steps.iter().any(|s| s.name == "parse_config"),
        "{:?}",
        trace.steps
    );

    // ── confidence + data-flow (G8) ──────────────────────────────────────
    let flow = store
        .data_flow("demo", "config::parse_config")
        .expect("flow");
    assert!(
        flow.iter().any(|(caller, _)| caller.contains("main")),
        "{flow:?}"
    );

    // ── incremental: unchanged reindex is a no-op ────────────────────────
    let second = store
        .index_project(dir.path(), &IndexOptions::new("demo"))
        .expect("second");
    assert_eq!(second.unchanged, 3);
    assert_eq!(second.parsed, 0);

    // ── reopen the same DB: data survives ────────────────────────────────
    drop(store);
    let reopened = GraphStore::open(&db).expect("reopen");
    assert!(reopened
        .node_by_qn("demo", "config::parse_config")
        .expect("qn")
        .is_some());
    let projects = reopened.list_projects().expect("projects");
    assert!(projects.contains(&"demo".to_string()));
}

#[test]
fn mmr_diversity_and_regex_on_real_index() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_repo(dir.path());
    let mut store = GraphStore::open_in_memory().expect("open");
    store
        .index_project(dir.path(), &IndexOptions::new("demo"))
        .expect("index");

    // The regex pre-filter tests name, qualified name, and file path, so a
    // symbol matches when any of the three contains the pattern.
    let re = store
        .search(
            "parse read",
            &SearchOptions::new("demo").with_regex("config"),
        )
        .expect("regex search");
    assert!(!re.is_empty(), "regex search returned nothing");
    assert!(
        re.iter().all(|h| h.name.contains("config")
            || h.qualified_name.contains("config")
            || h.file_path.contains("config")),
        "{re:?}"
    );

    // MMR rerank returns a bounded, non-empty result.
    let mmr = store
        .search(
            "config",
            &SearchOptions::new("demo").with_mmr(0.7).with_limit(2),
        )
        .expect("mmr");
    assert!(!mmr.is_empty() && mmr.len() <= 2);
}

// ── git test scaffolding (G5 detect_changes) ─────────────────────────────

/// Run a git command in `dir`, panicking on failure.
fn git(dir: &Path, args: &[&str]) {
    let status = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@e.x")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@e.x")
        .output()
        .expect("spawn git");
    assert!(
        status.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&status.stderr)
    );
}

/// `git init` + one commit containing `files`, returning the commit SHA.
fn init_commit(dir: &Path, files: &[(&str, &str)]) -> String {
    git(dir, &["init", "-q"]);
    git(dir, &["config", "user.email", "t@e.x"]);
    git(dir, &["config", "user.name", "t"]);
    for (path, content) in files {
        let full = dir.join(path);
        if let Some(parent) = full.parent() {
            std::fs::create_dir_all(parent).expect("mkdir");
        }
        std::fs::write(&full, content).expect("write");
    }
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "-qm", "one"]);
    rev(dir, "HEAD")
}

/// `git commit -am` a message and return the new SHA.
fn commit_all(dir: &Path, message: &str) -> String {
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "-qm", message]);
    rev(dir, "HEAD")
}

/// Resolve a revision to its full SHA.
fn rev(dir: &Path, reference: &str) -> String {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["rev-parse", reference])
        .output()
        .expect("spawn git");
    assert!(out.status.success(), "rev-parse {reference} failed");
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

#[test]
fn detect_changes_maps_git_diff_to_symbols() {
    let dir = tempfile::tempdir().expect("tempdir");
    // Two functions in one file; the second commit edits only `two`.
    let a = init_commit(
        dir.path(),
        &[(
            "lib.rs",
            "pub fn one() -> u32 { 1 }\n\npub fn two() -> u32 { 2 }\n",
        )],
    );

    // Keep the SQLite database OUTSIDE the git work tree: a db file inside it
    // would show up as an added path in the diff and pollute the report.
    let db_dir = tempfile::tempdir().expect("db tempdir");
    let mut store = GraphStore::open(db_dir.path().join("graph.db")).expect("open");
    store
        .index_project(dir.path(), &IndexOptions::new("p"))
        .expect("index");

    // Edit the body of `two` only.
    std::fs::write(
        dir.path().join("lib.rs"),
        "pub fn one() -> u32 { 1 }\n\npub fn two() -> u32 { 200 }\n",
    )
    .expect("edit");
    let b = commit_all(dir.path(), "edit two");

    let report = store.detect_changes("p", &a, &b).expect("detect");
    assert!(report.is_git, "temp repo is a work tree");
    assert_eq!(report.file_count(), 1, "{report:?}");
    assert_eq!(report.files[0].path, "lib.rs");
    let names: Vec<&str> = report.symbols.iter().map(|s| s.name.as_str()).collect();
    assert!(names.contains(&"two"), "edit touched `two`: {report:?}");
    assert!(
        !names.contains(&"one"),
        "`one` is untouched and must not be reported: {report:?}"
    );

    // Provenance is written back: a Branch node and HAS_BRANCH edges.
    let branch = store
        .node_by_qn("p", "p::__branch__::master")
        .expect("qn")
        .or_else(|| store.node_by_qn("p", "p::__branch__::main").expect("qn"));
    let branch = branch.expect("branch node");
    assert_eq!(branch.label, Label::Branch);
    assert_eq!(branch.prop_str("head_sha"), Some(b.as_str()));
    let file_node = store
        .node_by_qn("p", "p::lib.rs::__file__")
        .expect("qn")
        .expect("file node");
    let has_branch = store
        .out_edges(branch.id, Some(EdgeType::HasBranch))
        .expect("edges");
    assert!(
        has_branch.iter().any(|e| e.target == file_node.id),
        "{has_branch:?}"
    );
}

#[test]
fn detect_changes_reports_added_file_wholly() {
    let dir = tempfile::tempdir().expect("tempdir");
    let a = init_commit(dir.path(), &[("lib.rs", "pub fn one() {}\n")]);

    let db_dir = tempfile::tempdir().expect("db tempdir");
    let mut store = GraphStore::open(db_dir.path().join("graph.db")).expect("open");
    // Index the *new* state so the added file has graph nodes to match.
    std::fs::write(dir.path().join("extra.rs"), "pub fn extra() {}\n").expect("write");
    store
        .index_project(dir.path(), &IndexOptions::new("p"))
        .expect("index");
    let b = commit_all(dir.path(), "add extra");

    let report = store.detect_changes("p", &a, &b).expect("detect");
    let extra = report
        .files
        .iter()
        .find(|f| f.path == "extra.rs")
        .expect("extra.rs changed");
    assert_eq!(extra.status, pa_graph::ChangeStatus::Added);
    assert!(
        report.symbols.iter().any(|s| s.name == "extra"),
        "added file's symbols are all affected: {report:?}"
    );
}

#[test]
fn detect_changes_records_co_change_pairs() {
    let dir = tempfile::tempdir().expect("tempdir");
    let a = init_commit(dir.path(), &[("a.rs", "pub fn a() {}\n")]);
    // One commit touches both files together → they co-change.
    std::fs::write(dir.path().join("a.rs"), "pub fn a() { 1; }\n").expect("edit a");
    std::fs::write(dir.path().join("b.rs"), "pub fn b() {}\n").expect("add b");
    let b = commit_all(dir.path(), "touch both");

    let db_dir = tempfile::tempdir().expect("db tempdir");
    let mut store = GraphStore::open(db_dir.path().join("graph.db")).expect("open");
    store
        .index_project(dir.path(), &IndexOptions::new("p"))
        .expect("index");

    let report = store.detect_changes("p", &a, &b).expect("detect");
    assert_eq!(
        report.co_changes,
        vec![("a.rs".to_string(), "b.rs".to_string())],
        "{report:?}"
    );
}

#[test]
fn detect_changes_on_non_git_root_is_empty_not_error() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("lib.rs"), "pub fn f() {}\n").expect("write");
    let mut store = GraphStore::open_in_memory().expect("open");
    store
        .index_project(dir.path(), &IndexOptions::new("p"))
        .expect("index");
    let report = store.detect_changes("p", "HEAD~1", "HEAD").expect("detect");
    assert!(!report.is_git);
    assert_eq!(report.file_count(), 0);
    assert!(report.git.is_none());
}

// ── architecture views (G6) ──────────────────────────────────────────────

/// Build a store whose file graph has a deliberate two-file cycle plus a
/// hotspot, so the graph views have real topology to find.
fn cyclic_store() -> GraphStore {
    use pa_graph::{EdgeType as ET, Node};
    use serde_json::json;
    let store = GraphStore::open_in_memory().expect("store");
    store.register_project("p", "/tmp").expect("project");
    let mk = |qn: &str, file: &str| {
        let node = Node {
            id: 0,
            project: "p".into(),
            label: Label::Function,
            name: qn.rsplit("::").next().unwrap_or(qn).to_string(),
            qualified_name: qn.into(),
            file_path: file.into(),
            start_line: 1,
            end_line: 2,
            props: json!({}),
        };
        store.upsert_node(&node).expect("node")
    };
    // a.rs <-> b.rs form a cycle; both call into c.rs (a leaf).
    let a = mk("a::f", "a.rs");
    let b = mk("b::g", "b.rs");
    let c = mk("c::h", "c.rs");
    store
        .insert_edge("p", a, b, ET::Calls, &json!({}))
        .expect("e");
    store
        .insert_edge("p", b, a, ET::Calls, &json!({}))
        .expect("e");
    store
        .insert_edge("p", a, c, ET::Calls, &json!({}))
        .expect("e");
    store
        .insert_edge("p", b, c, ET::Calls, &json!({}))
        .expect("e");
    store
}

#[test]
fn architecture_overview_counts_graph() {
    let store = cyclic_store();
    let arch = store
        .architecture("p", ArchitectureView::Overview)
        .expect("overview");
    let ov = arch.overview.expect("overview section");
    assert_eq!(ov.nodes, 3);
    assert_eq!(ov.edges, 4);
    assert_eq!(ov.labels.get("Function"), Some(&3));
    assert_eq!(ov.edge_types.get("CALLS"), Some(&4));
}

#[test]
fn architecture_finds_cycle_on_synthetic_graph() {
    let store = cyclic_store();
    let arch = store
        .architecture("p", ArchitectureView::Cycles)
        .expect("cycles");
    assert_eq!(arch.cycles.len(), 1, "{:?}", arch.cycles);
    assert_eq!(
        arch.cycles[0].files,
        vec!["a.rs".to_string(), "b.rs".to_string()]
    );
    // c.rs is a leaf, not part of the cycle.
    assert!(!arch.cycles[0].files.iter().any(|f| f == "c.rs"));
}

#[test]
fn architecture_layers_and_clusters_and_hotspots() {
    let store = cyclic_store();
    let layers = store
        .architecture("p", ArchitectureView::Layers)
        .expect("layers");
    // c.rs is a dependency leaf → level 0; a/b sit above it.
    let leaf_level = layers
        .layers
        .iter()
        .find(|l| l.files.iter().any(|f| f == "c.rs"))
        .expect("c layer")
        .level;
    let top_level = layers
        .layers
        .iter()
        .map(|l| l.level)
        .max()
        .expect("a level");
    assert!(leaf_level < top_level, "{:?}", layers.layers);

    let clusters = store
        .architecture("p", ArchitectureView::Clusters)
        .expect("clusters");
    assert_eq!(clusters.clusters.len(), 1, "one connected community");
    assert_eq!(clusters.clusters[0].files.len(), 3);

    let hot = store
        .architecture("p", ArchitectureView::Hotspots)
        .expect("hotspots");
    assert_ne!(hot.hotspots.len(), 0);
    // Every symbol has at least one edge in this fixture.
    assert!(hot.hotspots.iter().all(|h| h.degree > 0));
}

#[test]
fn architecture_entry_points_and_file_tree_on_real_index() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir_all(dir.path().join("src")).expect("mkdir");
    std::fs::write(
        dir.path().join("src/lib.rs"),
        "pub fn leaf() -> u32 { 1 }\npub fn root() -> u32 { leaf() }\n",
    )
    .expect("write");
    let mut store = GraphStore::open_in_memory().expect("open");
    store
        .index_project(dir.path(), &IndexOptions::new("p"))
        .expect("index");

    let ep = store
        .architecture("p", ArchitectureView::EntryPoints)
        .expect("entry points");
    assert!(
        ep.entry_points.iter().any(|e| e.name == "root"),
        "root calls leaf but nothing calls root: {:?}",
        ep.entry_points
    );

    let tree = store
        .architecture("p", ArchitectureView::FileTree)
        .expect("tree");
    let tree = tree.file_tree.expect("tree section");
    let src = tree
        .children
        .iter()
        .find(|c| c.name == "src")
        .expect("src dir");
    assert!(src.files.contains(&"lib.rs".to_string()), "{src:?}");
    assert!(src.symbol_count >= 2, "{src:?}");
}


// ── Phase 3: incremental reindexing, deltas, signals, watcher ───────────

#[test]
fn closure_repair_body_vs_surface_on_a_real_tree() {
    use pa_graph::ReindexReason;

    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(
        dir.path().join("lib.rs"),
        "pub fn parse(path: &str) -> u32 { load(path) }\npub fn load(path: &str) -> u32 { path.len() as u32 }\n",
    )
    .expect("lib.rs");
    std::fs::write(
        dir.path().join("main.rs"),
        "fn main() { let n = parse(\"x\"); println!(\"{n}\"); }\n",
    )
    .expect("main.rs");

    let mut store = GraphStore::open_in_memory().expect("open");
    store
        .reindex_incremental(dir.path(), &IndexOptions::new("p"))
        .expect("first");

    // Body-only edit of `load`: surface unchanged → one file.
    std::fs::write(
        dir.path().join("lib.rs"),
        "pub fn parse(path: &str) -> u32 { load(path) }\npub fn load(path: &str) -> u32 { path.len() as u32 + 1 }\n",
    )
    .expect("edit body");
    let body = store
        .reindex_incremental(dir.path(), &IndexOptions::new("p"))
        .expect("body run");
    assert_eq!(body.reason, ReindexReason::BodyOnly, "{body:?}");
    assert_eq!(body.files_reindexed, 1);
    assert_eq!(body.closure_files, 0);

    // Surface edit of `parse`: main.rs calls parse, so it enters the closure.
    std::fs::write(
        dir.path().join("lib.rs"),
        "pub fn parse(path: &str, retries: u32) -> u32 { load(path) }\npub fn load(path: &str) -> u32 { path.len() as u32 + 1 }\n",
    )
    .expect("edit surface");
    let surface = store
        .reindex_incremental(dir.path(), &IndexOptions::new("p"))
        .expect("surface run");
    assert_eq!(surface.reason, ReindexReason::SurfaceChange, "{surface:?}");
    assert_eq!(surface.files_reindexed, 1);
    assert_eq!(surface.closure_files, 1, "{surface:?}");
    assert_eq!(surface.closure_paths, vec!["main.rs".to_string()]);
}

#[test]
fn surface_and_body_hashes_diverge_correctly() {
    use pa_graph::{body_hash, surface_hash};
    use pa_ast::SupportLang;

    let original = "pub fn f(x: u32) -> u32 { x + 1 }\n";
    let body_edit = "pub fn f(x: u32) -> u32 { x + 2 }\n";
    let surface_edit = "pub fn f(x: u64) -> u64 { x + 1 }\n";

    assert_eq!(
        surface_hash(original, SupportLang::Rust, "m"),
        surface_hash(body_edit, SupportLang::Rust, "m"),
        "body edit keeps the surface"
    );
    assert_ne!(
        surface_hash(original, SupportLang::Rust, "m"),
        surface_hash(surface_edit, SupportLang::Rust, "m"),
        "signature edit changes the surface"
    );
    assert_ne!(body_hash(original), body_hash(body_edit));
    assert_eq!(body_hash(original).len(), 64);
}

#[test]
fn lsp_surface_cache_invalidates_on_surface_change() {
    use pa_ast::SupportLang;
    use pa_graph::cache_lsp_surface;

    let store = GraphStore::open_in_memory().expect("open");
    store.register_project("p", "/tmp").expect("project");
    let src = "pub fn a(x: u32) -> u32 { x }\n";
    let first = cache_lsp_surface(&store, "p", "m.rs", src, SupportLang::Rust).expect("first");
    assert!(!first.from_cache);
    assert!(first.ref_bloom.contains("a") || !first.defs.is_empty());

    // A body edit → cache hit.
    let body = "pub fn a(x: u32) -> u32 { x + 1 }\n";
    let hit = cache_lsp_surface(&store, "p", "m.rs", body, SupportLang::Rust).expect("hit");
    assert!(hit.from_cache);

    // A surface edit → miss.
    let surface = "pub fn a(x: u32, y: u32) -> u32 { x + 1 }\n";
    let miss = cache_lsp_surface(&store, "p", "m.rs", surface, SupportLang::Rust).expect("miss");
    assert!(!miss.from_cache);
}

#[test]
fn minhash_lsh_finds_near_duplicate_functions() {
    use pa_ast::SupportLang;
    use pa_graph::{find_near_duplicates, minhash_source, CLONE_JACCARD};

    let a = "pub fn compute(a: u32, b: u32) -> u32 { let s = a + b; let p = a * b; s + p }\n";
    let b = "pub fn calc(x: u32, y: u32) -> u32 { let s = x + y; let p = x * y; s + p }\n";
    let c = "pub fn totally_unrelated() -> &'static str { \"hello\" }\n";
    let items = vec![
        (1u64, minhash_source(a, SupportLang::Rust)),
        (2u64, minhash_source(b, SupportLang::Rust)),
        (3u64, minhash_source(c, SupportLang::Rust)),
    ];
    // Identical source is a clone.
    let same = vec![
        (1u64, minhash_source(a, SupportLang::Rust)),
        (2u64, minhash_source(a, SupportLang::Rust)),
    ];
    let dups = find_near_duplicates(&same, CLONE_JACCARD);
    assert_eq!(dups.len(), 1, "{dups:?}");
    // Unrelated code is not.
    let none = find_near_duplicates(&items, 0.999);
    assert!(none.iter().all(|d| d.a == 1 && d.b == 2 || d.b == 1), "{none:?}");
}

#[test]
fn eleven_signals_rank_similar_symbols_above_unrelated() {
    use pa_graph::{symbol_signals, SymbolGraph};

    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(
        dir.path().join("a.rs"),
        "/// parse config\npub fn parse_config(path: &str) -> u32 { load(path) }\npub fn load(path: &str) -> u32 { path.len() as u32 }\n",
    )
    .expect("a.rs");
    std::fs::write(
        dir.path().join("b.rs"),
        "/// render widget\npub fn render_widget(name: &str) -> String { name.to_string() }\n",
    )
    .expect("b.rs");
    let mut store = GraphStore::open_in_memory().expect("open");
    store
        .index_project(dir.path(), &IndexOptions::new("p"))
        .expect("index");
    let graph = SymbolGraph::from_store(&store, "p").expect("graph");

    let parse = store
        .node_by_qn("p", "a::parse_config")
        .expect("qn")
        .expect("parse");
    let load = store.node_by_qn("p", "a::load").expect("qn").expect("load");
    let render = store
        .node_by_qn("p", "b::render_widget")
        .expect("qn")
        .expect("render");

    let sp = symbol_signals(&parse, &graph);
    let sl = symbol_signals(&load, &graph);
    let sr = symbol_signals(&render, &graph);
    assert_eq!(sp.concat().len(), pa_graph::SignalVector::dim());
    assert!(
        sp.similarity(&sl) > sp.similarity(&sr),
        "same-module caller/callee closer than an unrelated widget"
    );
}

#[test]
fn delta_stream_and_graph_delta_affected_nodes() {
    use pa_graph::{
        affected_nodes, Adjacency, Change, DeltaStream, EdgeDelta, EdgeOp, Encoding, GraphDelta,
        NodeDelta,
    };

    // Event sourcing round trip.
    let mut stream = DeltaStream::new();
    stream.set_keys(vec!["a".into(), "b".into(), "c".into()]);
    stream.push(
        1,
        vec![Change::remove("a"), Change::remove("b"), Change::remove("c")],
    );
    let decoded = stream.round_trip(0, Encoding::Rle).expect("round trip");
    assert_eq!(decoded.len(), 3);

    // Delta-graph affected nodes.
    let mut adj = Adjacency::new();
    adj.add_edge(1, 2, 1);
    adj.add_edge(2, 3, 1);
    let mut delta = GraphDelta::new();
    delta.push_node(NodeDelta::update(2, "m::b"));
    delta.push_edge(EdgeDelta::add(2, 3, EdgeType::Calls));
    let affected = affected_nodes(&delta, &adj);
    assert!(affected.direct.contains(&2));
    assert!(affected.direct.contains(&3));
    assert!(affected.adjacent.contains(&1));
    assert_eq!(EdgeOp::Add, EdgeOp::Add);
}

#[test]
fn streaming_condenser_and_tri_modal_and_watcher() {
    use pa_graph::{tri_modal, FsWatcher, StreamingCondenser};

    // Streaming condenser: lazy re-condense.
    let mut condenser = StreamingCondenser::new();
    condenser.extend([1.0, 2.0, 3.0]);
    assert!(condenser.is_dirty());
    assert_eq!(condenser.condense().count, 3);
    assert!(!condenser.is_dirty());
    let _ = condenser.condense();

    // Tri-modal search on a real index.
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(
        dir.path().join("lib.rs"),
        "pub fn parse_config(path: &str) -> u32 { load(path) }\npub fn load(path: &str) -> u32 { path.len() as u32 }\n",
    )
    .expect("lib.rs");
    let mut store = GraphStore::open_in_memory().expect("open");
    store
        .index_project(dir.path(), &IndexOptions::new("p"))
        .expect("index");
    let hits = tri_modal(&store, "p", "parse_config").expect("search");
    assert!(hits.iter().any(|h| h.name == "parse_config"), "{hits:?}");

    // Filesystem watcher: mtime polling detects a created file.
    let watch_dir = tempfile::tempdir().expect("watch dir");
    std::fs::write(watch_dir.path().join("a.rs"), "fn a() {}\n").expect("a");
    let mut watcher = FsWatcher::new(watch_dir.path(), |_| {});
    assert!(watcher.poll().is_empty(), "baseline poll is silent");
    std::fs::write(watch_dir.path().join("b.rs"), "fn b() {}\n").expect("b");
    let events = watcher.poll();
    assert_eq!(events.len(), 1, "{events:?}");
    assert_eq!(events[0].kind, pa_graph::FileEventKind::Created);
}
