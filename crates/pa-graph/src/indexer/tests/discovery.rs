//! Which files a pass discovers.
//!
//! `discover` has three inputs that can each add or drop a file: the two ignore
//! modes, ripgrep's `.ignore`, and the extension allowlist. Each test below
//! pins one of them, and each pair is checked against the other so flipping one
//! cannot smuggle a file through the other.

use super::super::options::IndexOptions;
use crate::store::GraphStore;

use super::fixtures::*;

/// The ignore mode decides the discovered set, and both modes are worth
/// reaching: `false` (the default) indexes what is on disk, `true` hides the
/// ignored files. A lean run over a wide store drops what it can no longer
/// see and reports it as `removed` rather than leaving orphans behind.
#[test]
fn the_ignore_mode_decides_the_discovered_file_set() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_ignored_tree(dir.path());
    let mut store = GraphStore::open_in_memory().expect("store");

    let wide = store
        .index_project(dir.path(), &IndexOptions::new("p"))
        .expect("wide index");
    assert_eq!(
        wide.added, 3,
        "the default mode walks what is on disk: {wide:?}"
    );
    assert_eq!(
        indexed_files(&store, "p"),
        vec!["a.rs", "gen/c.rs", "scratch/b.rs"],
    );

    let lean = store
        .index_project(
            dir.path(),
            &IndexOptions::new("p").with_respect_gitignore(true),
        )
        .expect("lean index");
    assert_eq!(lean.parsed, 0, "a.rs itself did not change: {lean:?}");
    assert_eq!(
        lean.removed, 2,
        "the ignored files leave the store: {lean:?}"
    );
    assert_eq!(indexed_files(&store, "p"), vec!["a.rs"]);
    // A lean pass must not leave the dropped files' symbols searchable.
    let hits = crate::search::search(
        store.connection(),
        "generated",
        &crate::search::SearchOptions::new("p"),
    )
    .expect("search");
    assert!(
        hits.is_empty(),
        "pruned symbols stayed searchable: {hits:?}"
    );
}

/// The extension allowlist is orthogonal to the ignore mode: a file the
/// options exclude stays out in both, so flipping the mode cannot smuggle
/// something past the filter.
/// `.ignore` is ripgrep's own ignore format, not git's, and the mode only
/// speaks about git's rules. It stays honoured in both: a caller flipping
/// `respect_gitignore` must not also start indexing a directory the project
/// marked with `.ignore`.
#[test]
fn the_ripgrep_ignore_file_is_honoured_in_both_modes() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join(".ignore"), "vendor/\n").expect(".ignore");
    std::fs::write(dir.path().join("a.rs"), "pub fn tracked() {}\n").expect("a.rs");
    std::fs::create_dir(dir.path().join("vendor")).expect("vendor");
    std::fs::write(dir.path().join("vendor/v.rs"), "pub fn vendored() {}\n").expect("v.rs");
    for respect in [false, true] {
        let mut store = GraphStore::open_in_memory().expect("store");
        store
            .index_project(
                dir.path(),
                &IndexOptions::new("p").with_respect_gitignore(respect),
            )
            .expect("index");
        assert_eq!(
            indexed_files(&store, "p"),
            vec!["a.rs"],
            "respect_gitignore={respect} dropped the `.ignore` rule",
        );
    }
}

/// A walk that FAILS is not a walk that found nothing.
///
/// The removal sweep deletes every stored file the pass did not see, so a
/// directory the walker could not traverse used to become silent data loss: the
/// pass reported success and the store lost the files it could not reach. The
/// error belongs to the pass, which must abort before it removes anything.
#[test]
fn a_failed_walk_aborts_the_pass_instead_of_emptying_the_store() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_tree(dir.path());
    let mut store = GraphStore::open_in_memory().expect("store");
    store
        .index_project(dir.path(), &IndexOptions::new("p"))
        .expect("first index");
    assert_eq!(indexed_files(&store, "p"), vec!["lib.rs"]);

    // The root is gone, so the walker reports an error for it and every stored
    // path is "not seen". Both write paths must refuse to act on that.
    let root = dir.path().to_path_buf();
    std::fs::remove_dir_all(&root).expect("remove the tree");
    let full = store.index_project(&root, &IndexOptions::new("p"));
    assert!(full.is_err(), "a failed walk must abort the pass: {full:?}");
    let incremental = store.reindex_incremental(&root, &IndexOptions::new("p"));
    assert!(
        incremental.is_err(),
        "a failed walk must abort the incremental pass: {incremental:?}"
    );

    assert_eq!(
        indexed_files(&store, "p"),
        vec!["lib.rs"],
        "the sweep removed files a failed walk simply did not see"
    );
    assert!(
        store
            .node_by_qn("p", "lib::parse_config")
            .expect("qn")
            .is_some(),
        "the symbols are gone with the file rows"
    );
}

#[test]
fn the_extension_filter_applies_in_both_ignore_modes() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_ignored_tree(dir.path());
    std::fs::write(dir.path().join("visible.py"), "def tracked():\n    pass\n").expect("py");
    std::fs::write(dir.path().join("gen/c.py"), "def generated():\n    pass\n").expect("py");
    let rust_only = || IndexOptions::new("p").with_extensions(vec!["rs".to_string()]);
    for respect in [false, true] {
        let mut store = GraphStore::open_in_memory().expect("store");
        store
            .index_project(dir.path(), &rust_only().with_respect_gitignore(respect))
            .expect("index");
        let files = indexed_files(&store, "p");
        assert!(
            !files
                .iter()
                .any(|file| std::path::Path::new(file).extension() == Some("py".as_ref())),
            "respect_gitignore={respect} let a filtered extension through: {files:?}"
        );
    }
}

/// A file that LEAVES the tree must not leave a coverage note behind.
///
/// `coverage` subtracts every noted path from `parsed`, so a note for a file that no
/// longer exists is not inert bookkeeping: it keeps `complete` false forever and makes
/// the printed ratio wrong in the direction a reader cannot see. Both sweeps ran
/// `clear_file` + `clear_call_sites` + the hash/surface deletes and neither touched
/// `index_coverage` — measured on a live store, 73 notes for files that were gone.
///
/// The fixture uses a SKIP (an oversized file), because that is the note the drift was
/// found on: a skipped file is recorded during the batch read, before any node exists.
#[test]
fn a_removed_file_takes_its_coverage_note_with_it() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("a.rs"), "pub fn kept() {}\n").expect("a.rs");
    std::fs::write(dir.path().join("big.json"), "x".repeat(4096)).expect("big");
    let tiny = || IndexOptions::new("p").with_max_file_bytes(64);
    // The note is written by the FULL pass alone: the incremental one skips an
    // oversized file before recording anything. Restore the tree each round so both
    // sweeps start from the same store.
    let oversized_source = "x".repeat(4096);

    for full_pass in [true, false] {
        std::fs::write(dir.path().join("big.json"), &oversized_source).expect("big");
        let mut store = GraphStore::open_in_memory().expect("store");
        store
            .index_project(dir.path(), &tiny())
            .expect("first index");
        let before = crate::coverage::coverage(&store, "p").expect("coverage");
        assert_eq!(
            before.skipped, 1,
            "the fixture must record the note it loses (full_pass={full_pass}): {before:?}"
        );

        std::fs::remove_file(dir.path().join("big.json")).expect("remove");
        if full_pass {
            store
                .index_project(dir.path(), &tiny())
                .expect("second index");
        } else {
            store
                .reindex_incremental(dir.path(), &tiny())
                .expect("incremental");
        }

        let after = crate::coverage::coverage(&store, "p").expect("coverage");
        assert_eq!(
            after.notes.len(),
            0,
            "a removed file's coverage note must not outlive it (full_pass={full_pass}): {:?}",
            after.notes
        );
        assert_eq!(after.total, 1, "only a.rs is left: {after:?}");
        assert_eq!(after.parsed, 1, "and it parsed cleanly: {after:?}");
        assert!(
            after.complete,
            "a clean tree must read as complete (full_pass={full_pass}): {after:?}"
        );
    }
}

/// The FULL pass must repair a caller whose target file disappeared.
///
/// `#35` was found in the incremental pass, and the full pass has the same hole for a
/// different reason: it re-parses only files whose content CHANGED, so an unchanged caller
/// keeps a verdict about a file that is gone. Measured before the fix: the full pass
/// reported `removed=1, parsed=0` and the site still said `resolved`, while a fresh index of
/// the same tree said `unresolved` — the two write paths disagreed about one tree.
///
/// The assertion is the COMPARISON (a fresh index of the same tree), because "resolved" on
/// its own is not the defect: the defect is the disagreement.
#[test]
fn the_full_pass_re_verdicts_a_caller_of_a_removed_file() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("a.rs"), "pub fn caller() { helper(); }\n").expect("a.rs");
    std::fs::write(dir.path().join("b.rs"), "pub fn helper() {}\n").expect("b.rs");
    let mut store = GraphStore::open_in_memory().expect("store");
    store
        .index_project(dir.path(), &IndexOptions::new("p"))
        .expect("first index");

    std::fs::remove_file(dir.path().join("b.rs")).expect("remove the target");
    let report = store
        .index_project(dir.path(), &IndexOptions::new("p"))
        .expect("second index");
    assert_eq!(report.removed, 1, "{report:?}");
    assert_eq!(
        report.parsed, 0,
        "the caller did not change, so nothing was re-parsed: {report:?}"
    );

    let stale = verdicts(&store);
    let mut fresh = GraphStore::open_in_memory().expect("store");
    fresh
        .index_project(dir.path(), &IndexOptions::new("p"))
        .expect("a fresh index of the same tree");
    assert_eq!(
        stale,
        verdicts(&fresh),
        "a second pass must not leave a verdict a fresh index does not"
    );
    assert_eq!(stale.get("helper").map(String::as_str), Some("unresolved"));
}

/// `callee -> resolution` for the fixture's one project.
fn verdicts(store: &GraphStore) -> std::collections::BTreeMap<String, String> {
    store
        .call_sites("p")
        .expect("sites")
        .into_iter()
        .map(|site| (site.callee.clone(), site.resolution))
        .collect()
}

/// A FORCED re-parse of an UNCHANGED file must not be skippable after a crash.
///
/// The hash is committed only with the resolution transaction, so a run that rewrote the
/// file and then died left the PREVIOUS hash behind. For a changed file that costs nothing:
/// the old digest no longer matches the bytes, so the retry re-parses. For an UNCHANGED
/// file — the forced or closure-triggered rewrite — it still matched, so the retry classified
/// the file as `Unchanged`, skipped it, and never rebuilt the rows the clear had removed
/// (review #13's surviving half).
///
/// The failure is injected where the real one happens: `call_sites` refuses a row, so the
/// resolution transaction rolls back whole. The files were re-parsed and their rows are gone;
/// the question is what the store now claims.
#[test]
fn an_interrupted_rewrite_of_an_unchanged_file_is_re_parsed() {
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

    store
        .connection()
        .execute_batch(
            "CREATE TRIGGER refuse_resolution BEFORE INSERT ON call_sites
             WHEN NEW.caller_qn = 'a::caller'
             BEGIN SELECT RAISE(ABORT, 'injected: resolution fails mid-run'); END",
        )
        .expect("trigger");
    assert!(
        store
            .index_project(dir.path(), &IndexOptions::new("p").with_force(true))
            .is_err(),
        "the injected failure must abort the pass"
    );

    // Nothing may claim the re-parsed file: the run produced rows and threw them away.
    assert!(
        store.file_hash("p", "a.rs").expect("hash").is_none(),
        "the failed run must not leave a hash over a graph it rolled back"
    );

    store
        .connection()
        .execute_batch("DROP TRIGGER refuse_resolution")
        .expect("drop the trigger");
    let retry = store
        .index_project(dir.path(), &IndexOptions::new("p"))
        .expect("retry");
    assert!(
        retry.parsed > 0,
        "the retry must re-parse what the failed run withdrew: {retry:?}"
    );
    assert!(
        store
            .call_sites("p")
            .expect("sites")
            .iter()
            .any(|site| site.callee == "helper" && site.resolution == "resolved"),
        "the retry restored the call sites"
    );
}
