//! A second pass: what it reuses, what it redoes, what it removes.
//!
//! The indexer is incremental by file hash. These tests pin the four
//! consequences: an unchanged file costs nothing, a modified one is re-parsed
//! without losing incoming edges, a removed one leaves no orphan rows, and a
//! forced pass re-parses everything.

use super::super::options::IndexOptions;
use crate::store::GraphStore;
use crate::types::EdgeType;

use super::super::batch::{TEST_BATCHES, TEST_SOURCE_BATCH_BYTES};
use super::fixtures::*;

#[test]
fn second_index_is_incremental() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_tree(dir.path());
    let mut store = GraphStore::open_in_memory().expect("store");
    let first = store
        .index_project(dir.path(), &IndexOptions::new("p"))
        .expect("first");
    assert_eq!(first.added, 1);
    let second = store
        .index_project(dir.path(), &IndexOptions::new("p"))
        .expect("second");
    assert_eq!(second.unchanged, 1);
    assert_eq!(second.parsed, 0);
    assert_eq!(second.added, 0);
}

#[test]
fn modification_is_detected_and_reindexed() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_tree(dir.path());
    let mut store = GraphStore::open_in_memory().expect("store");
    store
        .index_project(dir.path(), &IndexOptions::new("p"))
        .expect("first");
    std::fs::write(dir.path().join("lib.rs"), "pub fn only_one() {}\n").expect("rewrite");
    let report = store
        .index_project(dir.path(), &IndexOptions::new("p"))
        .expect("second");
    assert_eq!(report.modified, 1);
    // The old symbols are gone; the new one is present.
    assert!(store
        .node_by_qn("p", "lib::only_one")
        .expect("qn")
        .is_some());
    assert!(store
        .node_by_qn("p", "lib::parse_config")
        .expect("qn")
        .is_none());
}

#[test]
fn reindex_removes_stale_fts_rows() {
    // Regression: a contentless FTS table has no cascade, so a re-parsed
    // file must delete its old rows or a renamed symbol stays searchable.
    let dir = tempfile::tempdir().expect("tempdir");
    write_tree(dir.path());
    let mut store = GraphStore::open_in_memory().expect("store");
    store
        .index_project(dir.path(), &IndexOptions::new("p"))
        .expect("first");
    std::fs::write(dir.path().join("lib.rs"), "pub fn renamed_only() {}\n").expect("rewrite");
    store
        .index_project(dir.path(), &IndexOptions::new("p"))
        .expect("second");
    let stale = crate::search::search(
        store.connection(),
        "parse_config",
        &crate::search::SearchOptions::new("p"),
    )
    .expect("search");
    assert!(
        stale.is_empty(),
        "stale FTS rows survived reindex: {stale:?}"
    );
    let fresh = crate::search::search(
        store.connection(),
        "renamed_only",
        &crate::search::SearchOptions::new("p"),
    )
    .expect("search");
    assert!(fresh.iter().any(|h| h.name == "renamed_only"), "{fresh:?}");
}

/// Batching bounds MEMORY and must not change a single fact. A batching bug shows up
/// as a file silently missing from the graph — the class `coverage` exists to make
/// visible, and the one a green index run would not report.
#[test]
fn many_small_batches_produce_the_same_graph_as_one_large_batch() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_tree(dir.path());
    for i in 0..12 {
        std::fs::write(
            dir.path().join(format!("m{i}.rs")),
            format!("pub fn m{i}_one() {{}}\npub fn m{i}_two() {{ m{i}_one() }}\n"),
        )
        .expect("module");
    }

    let mut single = GraphStore::open_in_memory().expect("store");
    single
        .index_project(dir.path(), &IndexOptions::new("p"))
        .expect("one batch");
    let expected = (
        single.node_count("p").expect("nodes"),
        single.edge_count("p").expect("edges"),
        single.all_file_hashes("p").expect("hashes"),
    );

    let mut many = GraphStore::open_in_memory().expect("store");
    TEST_SOURCE_BATCH_BYTES.with(|slot| slot.set(Some(60)));
    TEST_BATCHES.store(0, std::sync::atomic::Ordering::Relaxed);
    let result = many.index_project(dir.path(), &IndexOptions::new("p"));
    TEST_SOURCE_BATCH_BYTES.with(|slot| slot.set(None));
    let result = result.expect("many batches");
    assert!(
        TEST_BATCHES.load(std::sync::atomic::Ordering::Relaxed) > 1,
        "the fixture must force several batches, or it proves nothing: {result:?}"
    );
    assert_eq!(
        (
            many.node_count("p").expect("nodes"),
            many.edge_count("p").expect("edges"),
            many.all_file_hashes("p").expect("hashes"),
        ),
        expected,
        "the graph must be identical however the files were grouped"
    );

    // A removal is still detected once, not once per batch.
    std::fs::remove_file(dir.path().join("m3.rs")).expect("remove");
    let report = many
        .index_project(dir.path(), &IndexOptions::new("p"))
        .expect("removal");
    assert_eq!(report.removed, 1, "removal is a pass property: {report:?}");
}

#[test]
fn removal_is_detected() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_tree(dir.path());
    let mut store = GraphStore::open_in_memory().expect("store");
    store
        .index_project(dir.path(), &IndexOptions::new("p"))
        .expect("first");
    std::fs::remove_file(dir.path().join("lib.rs")).expect("remove");
    let report = store
        .index_project(dir.path(), &IndexOptions::new("p"))
        .expect("second");
    assert_eq!(report.removed, 1);
}

#[test]
fn force_reparses_everything() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_tree(dir.path());
    let mut store = GraphStore::open_in_memory().expect("store");
    store
        .index_project(dir.path(), &IndexOptions::new("p"))
        .expect("first");
    let report = store
        .index_project(dir.path(), &IndexOptions::new("p").with_force(true))
        .expect("forced");
    assert_eq!(report.parsed, 1);
    assert_eq!(report.unchanged, 0);
}

#[test]
fn broken_file_is_indexed_and_flagged_partial() {
    // A file with an ERROR region still yields the symbols it does contain,
    // but the run must record honest coverage for it.
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(
        dir.path().join("broken.rs"),
        "pub fn good() {}\nfn bad( {\n    let x = );\n",
    )
    .expect("write");
    let mut store = GraphStore::open_in_memory().expect("store");
    let report = store
        .index_project(dir.path(), &IndexOptions::new("p"))
        .expect("index");
    assert_eq!(report.parsed, 1);
    assert!(
        report.partial + report.unusable >= 1,
        "broken file must be flagged: {report:?}"
    );
    let cov = store.coverage("p").expect("coverage");
    assert!(!cov.complete);
    assert!(cov.partial + cov.unusable >= 1, "{cov:?}");
}

#[test]
fn coverage_note_clears_when_file_is_fixed() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("m.rs"), "fn bad( {\n").expect("write");
    let mut store = GraphStore::open_in_memory().expect("store");
    store
        .index_project(dir.path(), &IndexOptions::new("p"))
        .expect("first");
    assert!(!store.coverage("p").expect("cov").complete);
    std::fs::write(dir.path().join("m.rs"), "fn good() {}\n").expect("fix");
    store
        .index_project(dir.path(), &IndexOptions::new("p"))
        .expect("second");
    let cov = store.coverage("p").expect("cov");
    assert!(cov.complete, "fixed file must read clean: {cov:?}");
    assert_eq!(cov.partial, 0);
    assert_eq!(cov.unusable, 0);
}

#[test]
fn report_edges_matches_the_store_after_indexing() {
    // R07: `report.edges` used to count insert ATTEMPTS, but the store
    // dedupes (`INSERT OR IGNORE` on a UNIQUE key), so the report
    // overstated the graph by a quarter on a real project. The
    // user-visible number must be the store's truth.
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(
        dir.path().join("lib.rs"),
        "pub fn caller() -> u32 { helper() + helper() }\npub fn helper() -> u32 { 1 }\n",
    )
    .expect("lib.rs");
    let mut store = GraphStore::open_in_memory().expect("store");
    let report = store
        .index_project(dir.path(), &IndexOptions::new("p"))
        .expect("index");
    // Two call sites of the same callee collapse into one stored edge;
    // the report must agree with what the store actually holds.
    assert_eq!(
        report.edges,
        store.edge_count("p").expect("count"),
        "report counts attempts, store dedupes"
    );
}

#[test]
fn modified_file_keeps_incoming_call_edges() {
    // R08: re-parsing a modified file used to delete its nodes, and the
    // schema's ON DELETE CASCADE silently dropped every CALLS edge from
    // unchanged files into it. Each edit permanently thinned the graph.
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("a.rs"), "pub fn foo() -> u32 { 1 }\n").expect("a.rs");
    std::fs::write(
        dir.path().join("b.rs"),
        "pub fn caller() -> u32 { foo() }\n",
    )
    .expect("b.rs");
    std::fs::write(dir.path().join("c.rs"), "pub fn other() -> u32 { foo() }\n").expect("c.rs");
    let mut store = GraphStore::open_in_memory().expect("store");
    store
        .index_project(dir.path(), &IndexOptions::new("p"))
        .expect("first");
    let before = store.edge_count("p").expect("count");

    // Body-only edit of a.rs; b.rs and c.rs are untouched.
    std::fs::write(dir.path().join("a.rs"), "pub fn foo() -> u32 { 1 + 1 }\n").expect("edit");
    store
        .index_project(dir.path(), &IndexOptions::new("p"))
        .expect("second");

    assert_eq!(
        store.edge_count("p").expect("count"),
        before,
        "a body edit must not drop incoming CALLS edges"
    );
    let foo = store
        .node_by_qn("p", "a::foo")
        .expect("qn")
        .expect("foo survived");
    let calls = store
        .in_edges(foo.id, Some(EdgeType::Calls))
        .expect("in edges");
    assert_eq!(calls.len(), 2, "b::caller and c::other still call a::foo");
}

/// A removed file must take its call sites with it (review #38).
///
/// `call_sites` keys on the file, not on a node id, so `clear_file` — which walks
/// the node, edge, and FTS tables — leaves the rows behind. They then answer
/// impact questions about call sites whose file no longer exists.
#[test]
fn a_removed_file_takes_its_call_sites_with_it() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_tree(dir.path());
    let mut store = GraphStore::open_in_memory().expect("store");
    store
        .index_project(dir.path(), &IndexOptions::new("p"))
        .expect("first");
    let before = store.call_sites("p").expect("sites").len();
    assert!(before > 0, "the fixture records a call site to lose");

    std::fs::remove_file(dir.path().join("lib.rs")).expect("remove");
    store
        .index_project(dir.path(), &IndexOptions::new("p"))
        .expect("second");

    assert_eq!(
        store.call_sites("p").expect("sites").len(),
        0,
        "a removed file's call sites must not survive the removal sweep"
    );
}

/// The same for a re-parse: the file stays, its old sites must not.
///
/// `clear_file_for_reparse` keeps the nodes so other files' incoming edges
/// survive, and the new extraction writes fresh sites — but the old ones were
/// keyed on the file and stayed, so a call the edit deleted kept answering.
#[test]
fn a_reparse_replaces_the_files_call_sites() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_tree(dir.path());
    let mut store = GraphStore::open_in_memory().expect("store");
    store
        .index_project(dir.path(), &IndexOptions::new("p"))
        .expect("first");
    // Two: `lib::parse_config -> load` and `lib::load -> len`.
    assert_eq!(
        store.call_sites("p").expect("sites").len(),
        2,
        "the fixture records two call sites to lose"
    );

    std::fs::write(
        dir.path().join("lib.rs"),
        "pub fn load(path: &str) -> u32 { path.len() as u32 }\n",
    )
    .expect("rewrite");
    store
        .index_project(dir.path(), &IndexOptions::new("p"))
        .expect("second");

    let sites = store.call_sites("p").expect("sites");
    eprintln!("SITES AFTER REPARSE: {sites:?}");
    assert!(
        sites
            .iter()
            .all(|site| !site.caller_qn.contains("parse_config")),
        "the call site of the removed function outlived the re-parse: {sites:?}"
    );
}

/// A caller whose target file DISAPPEARED must not keep a `resolved` verdict.
///
/// The sweep deleted the removed file's nodes and edges, then nothing knew who had called
/// into it: the caller kept `resolved` for a definition that no longer exists, so `impact`
/// answered "0 callers" for the gone symbol while the graph still claimed the call was
/// fine (review #35). The dependents are now collected BEFORE the sweep — the only moment
/// the edges still name them — and pulled into the closure.
#[test]
fn a_caller_of_a_removed_file_is_re_resolved() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("a.rs"), "pub fn caller() { helper(); }\n").expect("a.rs");
    std::fs::write(dir.path().join("b.rs"), "pub fn helper() {}\n").expect("b.rs");
    let mut store = GraphStore::open_in_memory().expect("store");
    store
        .index_project(dir.path(), &IndexOptions::new("p"))
        .expect("index");
    let before = store.call_sites("p").expect("sites");
    assert!(
        before
            .iter()
            .any(|site| site.callee == "helper" && site.resolution == "resolved"),
        "the fixture must start resolved: {before:?}"
    );

    std::fs::remove_file(dir.path().join("b.rs")).expect("remove the target");
    let report = store
        .reindex_incremental(dir.path(), &IndexOptions::new("p"))
        .expect("reindex");

    let after = store.call_sites("p").expect("sites");
    let site = after
        .iter()
        .find(|site| site.callee == "helper")
        .unwrap_or_else(|| panic!("the caller's site survives as a MISS: {after:?}"));
    assert_eq!(
        site.resolution, "unresolved",
        "the call now names a definition that does not exist: {after:?}"
    );
    assert!(
        report.closure_paths.iter().any(|path| path == "a.rs"),
        "the caller must be repaired through the closure: {report:?}"
    );
}

/// A call written through an ALIAS, in a file the run did not touch, must repair too.
///
/// `resolve_newly_satisfiable` covers the unaliased case. With an alias the repair looked
/// up the LOCAL name (`alias`) among the definition names and found nothing, because the
/// resolver it was handed knew only the imports of the files this run re-parsed — and the
/// caller had not changed (review #36). The binding cannot come from the `IMPORTS` edges
/// either: an edge exists only for an import that already resolved, and the whole point is
/// that it did not.
///
/// The assertion is a COMPARISON with a full index of the same tree, which is what the
/// divergence means: the two write paths must produce the same verdicts.
#[test]
fn an_aliased_caller_of_a_new_definition_is_repaired() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(
        dir.path().join("a.rs"),
        "use crate::c::thing as alias;\npub fn caller() { alias(); }\n",
    )
    .expect("a.rs");
    let mut store = GraphStore::open_in_memory().expect("store");
    store
        .index_project(dir.path(), &IndexOptions::new("p"))
        .expect("first index");
    assert!(
        store
            .call_sites("p")
            .expect("sites")
            .iter()
            .all(|site| site.resolution == "unresolved"),
        "the fixture starts with the alias unresolved"
    );

    // The definition appears; `a.rs` is untouched, so nothing re-parses it directly.
    std::fs::write(dir.path().join("c.rs"), "pub fn thing() {}\n").expect("c.rs");
    store
        .reindex_incremental(dir.path(), &IndexOptions::new("p"))
        .expect("reindex");

    let incremental = verdicts(&store);
    let mut full = GraphStore::open_in_memory().expect("store");
    full.index_project(dir.path(), &IndexOptions::new("p"))
        .expect("full index");
    assert_eq!(
        incremental,
        verdicts(&full),
        "an incremental pass must not leave a verdict a full index does not"
    );
    assert_eq!(
        incremental.get("alias").map(String::as_str),
        Some("resolved"),
        "the alias resolves once its target exists: {incremental:?}"
    );
}

/// `callee -> resolution` for one project, the comparable shape.
fn verdicts(store: &GraphStore) -> std::collections::BTreeMap<String, String> {
    store
        .call_sites("p")
        .expect("sites")
        .into_iter()
        .map(|site| (site.callee.clone(), site.resolution))
        .collect()
}

/// Two incremental passes in a row must agree with each other, not just with a full index.
///
/// The #35/#36 fixes repair a caller when its target vanishes or appears. A repair that runs
/// only once would leave the SECOND pass looking at a store its own first pass made
/// inconsistent — and "run reindex twice" is a normal operator action after a bad merge.
#[test]
fn a_second_incremental_pass_changes_nothing() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(
        dir.path().join("a.rs"),
        "use crate::c::thing as alias;\npub fn caller() { alias(); helper(); }\n",
    )
    .expect("a.rs");
    std::fs::write(dir.path().join("b.rs"), "pub fn helper() {}\n").expect("b.rs");
    let mut store = GraphStore::open_in_memory().expect("store");
    store
        .index_project(dir.path(), &IndexOptions::new("p"))
        .expect("first index");

    // An arrival and a removal in one pass: the alias target appears, the plain target goes.
    std::fs::write(dir.path().join("c.rs"), "pub fn thing() {}\n").expect("c.rs");
    std::fs::remove_file(dir.path().join("b.rs")).expect("remove b.rs");
    store
        .reindex_incremental(dir.path(), &IndexOptions::new("p"))
        .expect("reindex");
    let after_first = verdicts(&store);

    let second = store
        .reindex_incremental(dir.path(), &IndexOptions::new("p"))
        .expect("second reindex");
    assert_eq!(
        second.reindexed_paths,
        Vec::<String>::new(),
        "a settled store has nothing to reindex: {second:?}"
    );
    assert_eq!(
        verdicts(&store),
        after_first,
        "the second pass must not move a verdict"
    );
}

/// Every binding of a brace group must repair, not just the first.
///
/// `use crate::c::{thing as first, other as second};` is ONE line and TWO bindings. The scan
/// behind the repair stopped at the first `as`, so `first` resolved and `second` stayed
/// unresolved — while a full index resolved both. Found by the fourth reverify wave; the
/// assertion is the comparison with a full index, which is what the divergence means.
#[test]
fn a_grouped_alias_repairs_every_binding() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(
        dir.path().join("a.rs"),
        "use crate::c::{thing as first, other as second};\npub fn caller() { first(); second(); }\n",
    )
    .expect("a.rs");
    let mut store = GraphStore::open_in_memory().expect("store");
    store
        .index_project(dir.path(), &IndexOptions::new("p"))
        .expect("first index");

    std::fs::write(
        dir.path().join("c.rs"),
        "pub fn thing() {}\npub fn other() {}\n",
    )
    .expect("c.rs");
    store
        .reindex_incremental(dir.path(), &IndexOptions::new("p"))
        .expect("reindex");

    let incremental = verdicts(&store);
    let mut full = GraphStore::open_in_memory().expect("store");
    full.index_project(dir.path(), &IndexOptions::new("p"))
        .expect("full index");
    assert_eq!(
        incremental,
        verdicts(&full),
        "every binding of the group must repair"
    );
    assert_eq!(
        incremental.get("second").map(String::as_str),
        Some("resolved"),
        "the SECOND binding is the one the first scan version dropped: {incremental:?}"
    );
}

/// The INCREMENTAL in-place rewrite must withdraw its hash claim too.
///
/// The full path's `clear_file_for_reparse` does it (review #13); this path clears its rows
/// in `reindex_file_in_place` and left the previous hash, so a failed resolution after a
/// body-only re-parse kept a digest that still matched the bytes. The retry then took the
/// unchanged skip and the cleared call sites were gone for good — the same defect, reached
/// through the other writer.
#[test]
fn an_interrupted_incremental_rewrite_withdraws_its_hash() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(
        dir.path().join("a.rs"),
        "pub fn helper() {}\npub fn caller() { helper(); }\n",
    )
    .expect("a.rs");
    std::fs::write(dir.path().join("b.rs"), "pub fn other() {}\n").expect("b.rs");
    let mut store = GraphStore::open_in_memory().expect("store");
    store
        .index_project(dir.path(), &IndexOptions::new("p"))
        .expect("first index");
    assert!(
        store.file_hash("p", "a.rs").expect("hash").is_some(),
        "the fixture starts with a hash row"
    );

    // A BODY-only edit: the content changes, the surface does not, so the pass re-parses in
    // place. The failure is injected where a real one happens — the call-site writer.
    std::fs::write(
        dir.path().join("a.rs"),
        "pub fn helper() {}\npub fn caller() { helper(); helper(); }\n",
    )
    .expect("a.rs");
    store
        .connection()
        .execute_batch(
            "CREATE TRIGGER refuse_incremental_resolution BEFORE INSERT ON call_sites
             BEGIN SELECT RAISE(ABORT, 'injected: resolution fails mid-run'); END",
        )
        .expect("trigger");
    let _ = store.reindex_incremental(dir.path(), &IndexOptions::new("p"));
    store
        .connection()
        .execute_batch("DROP TRIGGER refuse_incremental_resolution")
        .expect("drop trigger");

    assert!(
        store.file_hash("p", "a.rs").expect("hash").is_none(),
        "an interrupted in-place rewrite must not leave a hash over the rows it withdrew"
    );
}
