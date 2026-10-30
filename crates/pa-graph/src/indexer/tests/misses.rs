//! A call site that does not resolve must leave a record, with the reason.
//!
//! A miss is a fact: discarding it is why the tool could not answer "how many
//! call sites in this file failed, and why". These tests pin the reason labels
//! the resolver emits, so a new catch-all cannot silently absorb the split.

use super::super::options::IndexOptions;
use crate::store::GraphStore;

/// A call site that does not resolve must leave a record with a reason.
/// Discarding it is why the tool could not answer "how many calls here failed,
/// and why" — the metric codebase-memory-mcp publishes as `unresolved`.
#[test]
fn an_unresolved_call_site_is_recorded_with_a_reason() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(
        dir.path().join("a.rs"),
        "pub fn caller() { totally_unknown_fn(); }\npub fn known() {}\n",
    )
    .expect("a.rs");
    let mut store = GraphStore::open_in_memory().expect("store");
    store
        .index_project(dir.path(), &IndexOptions::new("p"))
        .expect("index");
    let rows: Vec<(String, String)> = store
        .connection()
        .prepare(
            "SELECT json_extract(props_json,'$.callee'), json_extract(props_json,'$.reason')
             FROM edges WHERE project='p' AND edge_type='USAGE'",
        )
        .expect("stmt")
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
        .expect("rows")
        .map(|r| r.expect("row"))
        .collect();
    assert_eq!(rows.len(), 1, "one unresolved call site: {rows:?}");
    assert_eq!(rows[0].0, "totally_unknown_fn");
    // A plain lowercase identifier: it reads like a method call, and what a
    // resolver needs (the receiver's type) is not recorded, so this is
    // `type_unknown` rather than the old catch-all.
    assert_eq!(rows[0].1, "type_unknown");

    // A resolved call is NOT also recorded as a miss.
    let calls: i64 = store
        .connection()
        .query_row(
            "SELECT count(*) FROM edges WHERE project='p' AND edge_type='CALLS'",
            [],
            |r| r.get(0),
        )
        .expect("count");
    assert_eq!(calls, 0, "nothing in this fixture resolves");
}

/// The split, on the two shape groups that made it necessary: a trait
/// method (`clone`) and a constructor (`Some`), both of which call a
/// definition that lives outside the indexed tree.
///
/// Measured on this repository before the split: 17,395 of 18,206 misses
/// shared one `no_definition_with_that_name` label, so the count could not
/// say whether the resolver was broken or the code called `clone`.
#[test]
fn a_std_method_and_a_constructor_are_classified_external() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(
        dir.path().join("a.rs"),
        // One unresolved call per caller: a miss is a self-loop `USAGE`
        // edge and the store dedupes on (source, target, type), so two
        // misses from one function collapse into one row.
        "pub fn with_method(s: String) { let _t = s.clone(); }\n\
         pub fn with_ctor() -> Option<u32> { Some(1) }\n\
         pub fn with_from(s: String) -> String { String::from(s) }\n\
         pub fn with_parse(s: String) -> Result<u32, std::num::ParseIntError> { s.parse() }\n",
    )
    .expect("a.rs");
    let mut store = GraphStore::open_in_memory().expect("store");
    store
        .index_project(dir.path(), &IndexOptions::new("p"))
        .expect("index");
    let rows: Vec<(String, String)> = store
        .connection()
        .prepare(
            "SELECT json_extract(props_json,'$.callee'), json_extract(props_json,'$.reason')
             FROM edges WHERE project='p' AND edge_type='USAGE' ORDER BY 1",
        )
        .expect("stmt")
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
        .expect("rows")
        .map(|r| r.expect("row"))
        .collect();
    let reasons: std::collections::BTreeMap<String, String> = rows.into_iter().collect();
    // `clone` is a std trait method; `Some` is a std variant; `from` is a
    // conversion trait reached through a path. All of them name definitions
    // outside the indexed tree, so all are external rather than "no
    // definition with that name".
    assert_eq!(
        reasons.get("clone").map(String::as_str),
        Some("external_or_std"),
        "{reasons:?}"
    );
    assert_eq!(
        reasons.get("Some").map(String::as_str),
        Some("external_or_std"),
        "{reasons:?}"
    );
    assert_eq!(
        reasons.get("from").map(String::as_str),
        Some("external_or_std"),
        "{reasons:?}"
    );
    assert_eq!(
        reasons.get("parse").map(String::as_str),
        Some("external_or_std"),
        "a FromStr trait method reached through a path: {reasons:?}"
    );
}

/// A plain unknown `helper_fn()` is `type_unknown`, not `no_definition`:
/// a bare lowercase name cannot be told apart from `receiver.method()`
/// without the receiver's type, so the honest label names what is missing.
/// The fixture also shows the boundary — `Some` is external, `helper_fn` is
/// not, and they sit in the same function.
#[test]
fn a_plain_unknown_name_is_type_unknown() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(
        dir.path().join("a.rs"),
        "pub fn caller() { helper_fn(); }\n",
    )
    .expect("a.rs");
    let mut store = GraphStore::open_in_memory().expect("store");
    store
        .index_project(dir.path(), &IndexOptions::new("p"))
        .expect("index");
    let reason: String = store
        .connection()
        .query_row(
            "SELECT json_extract(props_json,'$.reason') FROM edges
             WHERE project='p' AND edge_type='USAGE'",
            [],
            |r| r.get(0),
        )
        .expect("reason");
    assert_eq!(reason, "type_unknown");
}

/// A name the tree knows, but a call shape resolution refuses, stays
/// `shape_not_modelled` — the one bucket that points back at our resolver
/// rather than at the code or at the standard library.
///
/// A callee this resolver cannot name AND cannot classify stays in the one
/// bucket that points back at our resolver rather than at the code or at the
/// standard library: a capitalised name with no definition in the tree is
/// read as a constructor/type-call shape we do not model.
///
/// The fixture used self-recursion until the recursion verdict existed: that
/// WAS the miss `resolve_call` created on purpose, and this test pinned the
/// defect as the specification. A recursive call has a fully known target
/// (the caller), so recording it as "we could not resolve this name" made
/// recursion indistinguishable from a name the resolver never handled — and
/// it was the only route into this bucket for a KNOWN name, which is why
/// `miss_reason` no longer has that arm.
#[test]
fn a_known_name_with_an_unmodelled_shape_stays_a_shape_problem() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("a.rs"), "pub fn caller() { Foo(); }\n").expect("a.rs");
    let mut store = GraphStore::open_in_memory().expect("store");
    store
        .index_project(dir.path(), &IndexOptions::new("p"))
        .expect("index");
    let reason: String = store
        .connection()
        .query_row(
            "SELECT json_extract(props_json,'$.reason') FROM edges
             WHERE project='p' AND edge_type='USAGE'",
            [],
            |r| r.get(0),
        )
        .expect("reason");
    assert_eq!(reason, "shape_not_modelled");
}

/// A miss must be distinguishable from a hit in a count, and the wire name
/// must be the one the schema writes (`USAGE`, not the enum variant's spelling).
#[test]
fn the_miss_edge_uses_the_stored_wire_name() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("a.rs"), "pub fn caller() { nope_fn(); }\n").expect("a.rs");
    let mut store = GraphStore::open_in_memory().expect("store");
    store
        .index_project(dir.path(), &IndexOptions::new("p"))
        .expect("index");
    let usage: i64 = store
        .connection()
        .query_row(
            "SELECT count(*) FROM edges WHERE project='p' AND edge_type='USAGE'",
            [],
            |r| r.get(0),
        )
        .expect("count");
    assert_eq!(usage, 1, "the miss is stored under USAGE");
    let mut stmt = store
        .connection()
        .prepare("SELECT DISTINCT edge_type FROM edges WHERE project='p'")
        .expect("stmt");
    let kinds: Vec<String> = stmt
        .query_map([], |r| r.get::<_, String>(0))
        .expect("rows")
        .map(|r| r.expect("row"))
        .collect();
    assert!(
        !kinds.iter().any(|k| k == "USES"),
        "USES is the enum variant, not the wire name: {kinds:?}"
    );
}

/// An edge whose callee name had many candidates is a guess, and the line must
/// say so. Measured worst case on a real repository: `new` with 400 candidates.
#[test]
fn an_ambiguous_edge_names_its_candidate_count() {
    let dir = tempfile::tempdir().expect("tempdir");
    // `shared` is defined in b and c but not in a, so the call from a has no
    // same-module candidate and must fall to the suffix rung.
    std::fs::write(dir.path().join("a.rs"), "pub fn caller() { shared(); }\n").expect("a.rs");
    std::fs::write(dir.path().join("b.rs"), "pub fn shared() {}\n").expect("b.rs");
    std::fs::write(dir.path().join("c.rs"), "pub fn shared() {}\n").expect("c.rs");
    let mut store = GraphStore::open_in_memory().expect("store");
    store
        .index_project(dir.path(), &IndexOptions::new("p"))
        .expect("index");
    let rows: Vec<(String, i64)> = store
        .connection()
        .prepare(
            "SELECT json_extract(props_json,'$.strategy'),
                    json_extract(props_json,'$.candidates')
             FROM edges WHERE project='p' AND edge_type='CALLS'",
        )
        .expect("stmt")
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))
        .expect("rows")
        .map(|r| r.expect("row"))
        .collect();
    assert_eq!(rows.len(), 1, "one call site: {rows:?}");
    assert_eq!(rows[0].0, "suffix", "two same-named defs is a suffix guess");
    assert_eq!(rows[0].1, 2, "the edge records how many candidates existed");
}

/// A call whose path is rooted at a workspace dependency names a definition
/// outside the tree. It must be a CALLS edge with the `dependency` strategy,
/// not a miss: the reader needs to know where it goes, not that we failed.
#[test]
fn a_call_rooted_at_a_dependency_is_a_dependency_edge() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(
        dir.path().join("Cargo.toml"),
        "[package]\nname = \"t\"\nversion = \"0.1.0\"\n\n[dependencies]\ntempdir = \"3\"\n",
    )
    .expect("Cargo.toml");
    std::fs::write(
        dir.path().join("a.rs"),
        "pub fn caller() { tempdir::TempDir::new(); }\n",
    )
    .expect("a.rs");
    let mut store = GraphStore::open_in_memory().expect("store");
    store
        .index_project(dir.path(), &IndexOptions::new("p"))
        .expect("index");
    let rows: Vec<(String, String, String)> = store
        .connection()
        .prepare(
            "SELECT edge_type, json_extract(props_json,'$.strategy'),
                    json_extract(props_json,'$.crate')
             FROM edges WHERE project='p' AND edge_type IN ('CALLS','USAGE')",
        )
        .expect("stmt")
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .expect("rows")
        .map(|r| r.expect("row"))
        .collect();
    assert_eq!(rows.len(), 1, "one call site: {rows:?}");
    assert_eq!(rows[0].0, "CALLS", "a dependency call is not a miss");
    assert_eq!(rows[0].1, "dependency");
    assert_eq!(rows[0].2, "tempdir");
}

/// A `crate::`/`self::` path is in-tree by definition and must stay a miss:
/// excusing it as a dependency would hide a real resolver gap.
#[test]
fn a_crate_local_path_is_not_a_dependency() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(
        dir.path().join("Cargo.toml"),
        "[package]\nname = \"t\"\nversion = \"0.1.0\"\n\n[dependencies]\ncrate = \"1\"\n",
    )
    .expect("Cargo.toml");
    std::fs::write(
        dir.path().join("a.rs"),
        "pub fn caller() { crate::elsewhere::thing(); }\n",
    )
    .expect("a.rs");
    let mut store = GraphStore::open_in_memory().expect("store");
    store
        .index_project(dir.path(), &IndexOptions::new("p"))
        .expect("index");
    let strategies: Vec<String> = store
        .connection()
        .prepare("SELECT json_extract(props_json,'$.strategy') FROM edges WHERE project='p'")
        .expect("stmt")
        .query_map([], |r| r.get::<_, Option<String>>(0))
        .expect("rows")
        .filter_map(|r| r.expect("row"))
        .collect();
    assert!(
        !strategies.iter().any(|s| s == "dependency"),
        "a crate-local path must not be excused as a dependency: {strategies:?}"
    );
}
