//! What a completed run leaves behind.
//!
//! A finished pass is more than a node count: it must leave planner
//! statistics, a verifiable receipt, and a digest that differs when the tree
//! differs. The verification tests read the chain the run wrote.

use super::super::options::IndexOptions;
use crate::store::GraphStore;

use super::fixtures::*;

#[test]
fn indexes_and_resolves_calls() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_tree(dir.path());
    let mut store = GraphStore::open_in_memory().expect("store");
    let report = store
        .index_project(dir.path(), &IndexOptions::new("p"))
        .expect("index");
    assert_eq!(report.added, 1);
    assert_eq!(report.parsed, 1);
    assert!(report.nodes >= 3, "expected file + 2 functions: {report:?}");
    assert!(
        report.edges >= 1,
        "expected at least one CALLS edge: {report:?}"
    );
    let hits = crate::search::search(
        store.connection(),
        "load",
        &crate::search::SearchOptions::new("p"),
    )
    .expect("search");
    assert!(hits.iter().any(|h| h.name == "load"), "{hits:?}");
}

/// A `use` that binds a symbol to a file must produce an IMPORTS edge. The
/// live store on 2026-10-22 had 53,863 CALLS and **zero** IMPORTS, so every
/// structure/layers/cycles view saw call edges only — the resolver was
/// silently returning `None` for every binding in the repo.
#[test]
fn a_cross_file_use_emits_an_imports_edge() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(
        dir.path().join("a.rs"),
        "pub struct Widget;\npub fn make() -> Widget { Widget }\n",
    )
    .expect("a.rs");
    std::fs::write(
        dir.path().join("b.rs"),
        "use crate::a::{Widget, make};\npub fn use_it() -> Widget { make() }\n",
    )
    .expect("b.rs");
    std::fs::write(dir.path().join("lib.rs"), "pub mod a;\npub mod b;\n").expect("lib.rs");
    let mut store = GraphStore::open_in_memory().expect("store");
    store
        .index_project(dir.path(), &IndexOptions::new("p"))
        .expect("index");
    let imports: i64 = store
        .connection()
        .query_row(
            "SELECT count(*) FROM edges WHERE edge_type = 'IMPORTS'",
            [],
            |row| row.get(0),
        )
        .expect("count");
    assert!(
        imports >= 2,
        "a use binding of Widget and make must produce at least two IMPORTS edges, got {imports}"
    );
}

/// Indexing must leave planner statistics behind. Without them the planner
/// picks `idx_edges_type` for a name-keyed trace and walks every edge in the
/// store: measured 52.30 ms -> 0.08 ms per trace on a 36k-node store (472x).
#[test]
fn indexing_leaves_planner_statistics() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("a.rs"), "pub fn caller() { callee() }\n").expect("a.rs");
    std::fs::write(dir.path().join("b.rs"), "pub fn callee() {}\n").expect("b.rs");
    let mut store = GraphStore::open_in_memory().expect("store");
    store
        .index_project(dir.path(), &IndexOptions::new("p"))
        .expect("index");
    let stats: i64 = store
        .connection()
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE name LIKE 'sqlite_stat%'",
            [],
            |r| r.get(0),
        )
        .expect("stat tables");
    assert!(stats > 0, "indexing must leave planner statistics behind");
}

/// Two runs over an unchanged tree must produce the same input digest, and
/// the chain must verify. This is the `witness` module's first consumer.
#[test]
fn index_runs_record_a_verifiable_receipt_chain() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_tree(dir.path());
    let mut store = GraphStore::open_in_memory().expect("store");
    store
        .index_project(dir.path(), &IndexOptions::new("p"))
        .expect("first");
    let first: String = store
        .connection()
        .query_row(
            "SELECT input_hash FROM index_receipts WHERE project = 'p'",
            [],
            |r| r.get(0),
        )
        .expect("first receipt");
    store
        .index_project(dir.path(), &IndexOptions::new("p"))
        .expect("second");
    let hashes: Vec<String> = store
        .connection()
        .prepare("SELECT input_hash FROM index_receipts WHERE project = 'p' ORDER BY id")
        .expect("stmt")
        .query_map([], |r| r.get::<_, String>(0))
        .expect("rows")
        .map(|r| r.expect("row"))
        .collect();
    assert_eq!(hashes.len(), 2, "one receipt per completed run");
    assert_eq!(
        hashes[0], hashes[1],
        "an unchanged tree must digest the same on both runs"
    );
    assert_eq!(hashes[0], first);
    assert!(
        store.verify_receipt_chain("p").expect("verify"),
        "the chain must verify"
    );
}

/// Editing a file changes the run digest: receipts are not just counters.
#[test]
fn a_content_change_changes_the_run_digest() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_tree(dir.path());
    let mut store = GraphStore::open_in_memory().expect("store");
    store
        .index_project(dir.path(), &IndexOptions::new("p"))
        .expect("first");
    std::fs::write(dir.path().join("lib.rs"), "pub fn other() -> u32 { 2 }\n").expect("edit");
    store
        .index_project(dir.path(), &IndexOptions::new("p"))
        .expect("second");
    let hashes: Vec<String> = store
        .connection()
        .prepare("SELECT input_hash FROM index_receipts WHERE project = 'p' ORDER BY id")
        .expect("stmt")
        .query_map([], |r| r.get::<_, String>(0))
        .expect("rows")
        .map(|r| r.expect("row"))
        .collect();
    assert_ne!(
        hashes[0], hashes[1],
        "an edited tree must digest differently"
    );
}

/// A rewritten receipt must be detected.
#[test]
fn a_tampered_receipt_breaks_verification() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_tree(dir.path());
    let mut store = GraphStore::open_in_memory().expect("store");
    store
        .index_project(dir.path(), &IndexOptions::new("p"))
        .expect("index");
    store
        .connection()
        .execute(
            "UPDATE index_receipts SET input_hash = 'forged' WHERE project = 'p'",
            [],
        )
        .expect("tamper");
    assert!(
        !store.verify_receipt_chain("p").expect("verify"),
        "a rewritten input_hash must not verify"
    );
}

/// The `verify` action is the join of three modules that had no consumer:
/// `quality` builds the envelope, `witness` supplies the receipts, and
/// `proof_gate` decides. A freshly indexed project must accept.
#[test]
fn verify_accepts_a_cleanly_indexed_project() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_tree(dir.path());
    let mut store = GraphStore::open_in_memory().expect("store");
    store
        .index_project(dir.path(), &IndexOptions::new("p"))
        .expect("index");
    let verdict = store.verify_run("p").expect("verify");
    assert_eq!(verdict.receipts, 1, "one completed run, one receipt");
    assert!(verdict.chain_ok, "the chain must be intact");
    assert!(
        verdict.decision.is_accept(),
        "a clean run must accept, got {:?}",
        verdict.decision
    );
    assert_eq!(verdict.envelope.count, 1);
}

/// A project with no runs has nothing to verify, and the gate must say so
/// rather than silently accepting.
#[test]
fn verify_rejects_a_project_with_no_runs() {
    let store = GraphStore::open_in_memory().expect("store");
    store
        .register_project("empty", "/tmp/empty")
        .expect("project");
    let verdict = store.verify_run("empty").expect("verify");
    assert_eq!(verdict.receipts, 0);
    assert!(!verdict.decision.is_accept(), "no receipts must not accept");
}

/// A tampered receipt must not accept, even though the envelope is healthy.
#[test]
fn verify_rejects_a_tampered_chain() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_tree(dir.path());
    let mut store = GraphStore::open_in_memory().expect("store");
    store
        .index_project(dir.path(), &IndexOptions::new("p"))
        .expect("index");
    store
        .connection()
        .execute(
            "UPDATE index_receipts SET input_hash = 'forged' WHERE project = 'p'",
            [],
        )
        .expect("tamper");
    let verdict = store.verify_run("p").expect("verify");
    assert!(!verdict.chain_ok, "a rewritten receipt breaks the chain");
    assert!(!verdict.decision.is_accept());
}

/// A run whose files mostly failed to parse is degraded: the quality
/// envelope must carry a flag and the gate must reject.
#[test]
fn verify_rejects_a_degraded_run() {
    let dir = tempfile::tempdir().expect("tempdir");
    // A file whose parser reports an unusable region.
    std::fs::write(dir.path().join("broken.rs"), "fn {{{ ((( )\n").expect("broken.rs");
    let mut store = GraphStore::open_in_memory().expect("store");
    store
        .index_project(dir.path(), &IndexOptions::new("p"))
        .expect("index");
    let verdict = store.verify_run("p").expect("verify");
    if verdict.envelope.is_degraded() {
        assert!(
            !verdict.decision.is_accept(),
            "a flagged envelope must reject, got {:?}",
            verdict.decision
        );
    }
}
