//! The split's own invariant: the indexer stays one module per responsibility.
//!
//! `indexer.rs` was a 2 206-line file with 83 top-level items: the builder, the
//! walk, the pass, the resolver, the free entry points, and 36 tests in one
//! scope. It is now a facade over seven subject modules plus a test module.
//!
//! Nothing in the compiler stops the monolith from growing back — a new helper
//! lands beside its caller and the file drifts. So this test reads the crate's
//! own sources and fails when the facade starts holding code again, or when a
//! subject module grows past the budget the split was made for.

use std::path::{Path, PathBuf};

/// The facade: module declarations and re-exports only.
const FACADE: &str = "indexer.rs";

/// A facade that needs more than this is holding code, not wiring. It is 46
/// lines today; the margin covers a new re-export block, not a new function.
const FACADE_MAX_LINES: usize = 140;

/// A subject module's budget. Every module is under 280 lines today, so a file
/// reaching this ceiling is a new responsibility asking for its own module.
const MODULE_MAX_LINES: usize = 700;

/// Item heads that make a facade stop being a facade.
const DEFINITION_HEADS: [&str; 9] = [
    "fn ",
    "struct ",
    "enum ",
    "impl ",
    "trait ",
    "const ",
    "static ",
    "type ",
    "macro_rules! ",
];

/// The crate's `src/` directory.
fn src_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src")
}

/// Violations in one file: its line count and, for the facade, any definition.
fn check_file(rel: &str, text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let lines = text.lines().count();
    let is_facade = rel == FACADE;
    let budget = if is_facade {
        FACADE_MAX_LINES
    } else {
        MODULE_MAX_LINES
    };
    if lines > budget {
        out.push(format!("{rel}: {lines} lines, budget {budget}"));
    }
    if is_facade {
        for (idx, raw) in text.lines().enumerate() {
            // Top level means column 0: an indented `fn` is inside an `impl` or a
            // function body, which a facade does not have either, but the line
            // that names the item is the unindented one.
            if raw.starts_with(char::is_whitespace) || raw.starts_with("//") {
                continue;
            }
            // Visibility is a prefix on the definition, not a separate item.
            let head = raw
                .trim_start_matches("pub(crate) ")
                .trim_start_matches("pub(super) ")
                .trim_start_matches("pub ")
                .trim_start_matches("async ")
                .trim_start_matches("unsafe ");
            if DEFINITION_HEADS.iter().any(|h| head.starts_with(h)) {
                out.push(format!("{rel}:{}: the facade defines `{raw}`", idx + 1));
            }
        }
    }
    out
}

/// Violations across `src/`: the facade plus every file under `indexer/`.
pub(crate) fn check_dir(src: &Path) -> Vec<String> {
    let facade = src.join(FACADE);
    let text = std::fs::read_to_string(&facade).expect("read the indexer facade");
    let mut out = check_file(FACADE, &text);
    let modules = src.join("indexer");
    let mut files = Vec::new();
    collect_rs(&modules, &mut files);
    files.sort();
    for path in files {
        let rel = path
            .strip_prefix(src)
            .unwrap_or(&path)
            .to_string_lossy()
            .replace('\\', "/");
        let text = std::fs::read_to_string(&path).expect("read an indexer module");
        out.extend(check_file(&rel, &text));
    }
    out
}

fn collect_rs(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries {
        let path = entry.expect("dir entry").path();
        if path.is_dir() {
            collect_rs(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            out.push(path);
        }
    }
}

#[test]
fn the_indexer_stays_split_into_subject_modules() {
    let violations = check_dir(&src_dir());
    assert!(
        violations.is_empty(),
        "the indexer is drifting back into a monolith:\n{}",
        violations.join("\n")
    );
}
