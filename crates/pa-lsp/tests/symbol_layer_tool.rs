//! Hermetic tests for the symbol-layer tool actions (S9/S10/S13/S14).
//!
//! These actions build their symbol tree from tree-sitter (`pa-ast`), so they
//! need no language server. The test drives the real `LspTool::execute`
//! contract against a temp project.

#![forbid(unsafe_code)]

use std::sync::Arc;

use pa_agent::abort::AbortSignal;
use pa_agent::types::AgentTool as _;
use pa_lsp::{LspManager, LspTool};

fn run(tool: &Arc<LspTool>, params: serde_json::Value) -> pa_agent::types::AgentToolResult {
    let tool = Arc::clone(tool);
    let result = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async move {
            tool.execute(
                "call".to_string(),
                params,
                AbortSignal::default(),
                Arc::new(|_| {}),
            )
            .await
        });
    result.expect("tool call should not error")
}

fn text_of(result: &pa_agent::types::AgentToolResult) -> String {
    match &result.content[0] {
        pa_agent::types::ToolResultContent::Text(text) => text.text.clone(),
        other @ pa_agent::types::ToolResultContent::Image(_) => {
            panic!("expected text content, got {other:?}")
        }
    }
}

const RUST: &str = "pub struct Foo {\n    x: i32,\n}\n\
impl Foo {\n    pub fn bar(&self) -> i32 {\n        self.x\n    }\n}\n\
pub fn top() {}\n";

fn project() -> (tempfile::TempDir, Arc<LspTool>) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("lib.rs"), RUST).unwrap();
    let manager = Arc::new(LspManager::new());
    let tool = Arc::new(LspTool::new(manager, dir.path().to_path_buf()));
    (dir, tool)
}

#[test]
fn symbols_overview_lists_top_level_and_nested() {
    let (_dir, tool) = project();
    let result = run(
        &tool,
        serde_json::json!({ "action": "symbols", "file": "lib.rs" }),
    );
    let text = text_of(&result);
    assert!(text.contains("Foo"), "{text}");
    assert!(text.contains("Foo/bar"), "{text}");
    assert!(text.contains("top"), "{text}");
    assert!(!result.is_error);
}

#[test]
fn symbols_overview_honours_depth() {
    let (_dir, tool) = project();
    let result = run(
        &tool,
        serde_json::json!({ "action": "symbols", "file": "lib.rs", "depth": 0 }),
    );
    let text = text_of(&result);
    assert!(text.contains("Foo"), "{text}");
    // Depth 0 hides the impl methods.
    assert!(!text.contains("Foo/bar"), "{text}");
}

#[test]
fn find_symbol_resolves_a_name_path() {
    let (_dir, tool) = project();
    let result = run(
        &tool,
        serde_json::json!({ "action": "find_symbol", "file": "lib.rs", "symbol": "Foo/bar" }),
    );
    let text = text_of(&result);
    assert!(text.contains("Foo/bar"), "{text}");
    assert!(text.contains("1 match"), "{text}");
}

#[test]
fn find_symbol_kinds_filter_narrows_results() {
    let (_dir, tool) = project();
    let result = run(
        &tool,
        serde_json::json!({
            "action": "find_symbol", "file": "lib.rs", "symbol": "",
            "kinds": "function"
        }),
    );
    let text = text_of(&result);
    assert!(text.contains("top"), "{text}");
    assert!(!text.contains("Foo/bar"), "{text}");
}

#[test]
fn find_symbol_max_matches_caps_output() {
    let (_dir, tool) = project();
    let result = run(
        &tool,
        serde_json::json!({
            "action": "find_symbol", "file": "lib.rs", "symbol": "", "max_matches": 1
        }),
    );
    let text = text_of(&result);
    assert!(
        text.contains("and 3 more") || text.contains("and 2 more"),
        "{text}"
    );
}

#[test]
fn replace_symbol_body_previews_and_applies() {
    let (dir, tool) = project();
    // Preview first: the file is untouched.
    let preview = run(
        &tool,
        serde_json::json!({
            "action": "replace_symbol_body", "file": "lib.rs",
            "symbol": "top", "body": "pub fn top() { let y = 2; }"
        }),
    );
    assert!(
        text_of(&preview).contains("apply=false"),
        "{}",
        text_of(&preview)
    );
    assert!(std::fs::read_to_string(dir.path().join("lib.rs"))
        .unwrap()
        .contains("pub fn top() {}"));

    // Apply: the file changes.
    let applied = run(
        &tool,
        serde_json::json!({
            "action": "replace_symbol_body", "file": "lib.rs",
            "symbol": "top", "body": "pub fn top() { let y = 2; }", "apply": true
        }),
    );
    assert!(
        text_of(&applied).contains("applied edit"),
        "{}",
        text_of(&applied)
    );
    let after = std::fs::read_to_string(dir.path().join("lib.rs")).unwrap();
    assert!(after.contains("pub fn top() { let y = 2; }"), "{after}");
}

#[test]
fn insert_after_symbol_places_content_below() {
    let (dir, tool) = project();
    let result = run(
        &tool,
        serde_json::json!({
            "action": "insert_after_symbol", "file": "lib.rs",
            "symbol": "Foo", "body": "pub fn extra() {}", "apply": true
        }),
    );
    assert!(!result.is_error, "{:?}", text_of(&result));
    let after = std::fs::read_to_string(dir.path().join("lib.rs")).unwrap();
    let foo = after.find("struct Foo").unwrap();
    let extra = after.find("fn extra").unwrap();
    assert!(extra > foo, "{after}");
}

/// An unavailable reference lookup must NOT be read as "nothing references
/// this". Serena shipped that bug: `safe_delete_symbol` deleted symbols with
/// live callers whenever the server was cold, restarting, or timed out, because
/// an empty `references` reply looked like proof of no callers. Deletion is
/// irreversible, so the default is to refuse and name the escape hatch.
///
/// The fixture must have NO language server, or the lookup really does answer:
/// a bare `.rs` file spawns the rust-analyzer that `pa-lsp` tests install, and
/// its `references` reply for this symbol is a truthful "none", which the tool
/// is right to delete on. Java parses with the native tree-sitter grammar
/// (`pa-ast`) but the server table has no `java` entry, so the lookup is
/// unavailable on every machine and this test proves the refusal itself.
#[test]
fn safe_delete_refuses_when_references_cannot_be_checked() {
    const JAVA: &str = "public class Foo {\n    public int bar() {\n        return 1;\n    }\n}\n";
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("Foo.java"), JAVA).unwrap();
    let tool = Arc::new(LspTool::new(
        Arc::new(LspManager::new()),
        dir.path().to_path_buf(),
    ));
    let result = run(
        &tool,
        serde_json::json!({
            "action": "safe_delete_symbol", "file": "Foo.java",
            "symbol": "Foo/bar", "apply": true
        }),
    );
    let text = text_of(&result);
    assert!(text.contains("refusing to delete"), "{text}");
    assert!(
        text.contains("force: true"),
        "the escape hatch is named: {text}"
    );
    let after = std::fs::read_to_string(dir.path().join("Foo.java")).unwrap();
    assert_eq!(after, JAVA, "the file must be untouched: {after}");
}

/// `force: true` keeps the old behaviour available for a caller that has
/// established the symbol is unreferenced by other means.
#[test]
fn safe_delete_with_force_removes_an_unreferenced_symbol() {
    let (dir, tool) = project();
    let result = run(
        &tool,
        serde_json::json!({
            "action": "safe_delete_symbol", "file": "lib.rs",
            "symbol": "Foo/bar", "apply": true, "force": true
        }),
    );
    assert!(!result.is_error, "{:?}", text_of(&result));
    let after = std::fs::read_to_string(dir.path().join("lib.rs")).unwrap();
    assert!(!after.contains("fn bar"), "{after}");
    assert!(after.contains("struct Foo"), "{after}");
}

#[test]
fn find_symbol_include_body_appends_the_text() {
    let (_dir, tool) = project();
    let result = run(
        &tool,
        serde_json::json!({
            "action": "find_symbol", "file": "lib.rs", "symbol": "top", "include_body": true
        }),
    );
    let text = text_of(&result);
    assert!(text.contains("--- top ---"), "{text}");
    assert!(text.contains("pub fn top() {}"), "{text}");
}

#[test]
fn find_symbol_include_info_degrades_without_a_server() {
    // A `.json` file parses with tree-sitter but has no LSP server entry, so the
    // info pass reports unavailability *without spawning a process* — keeping
    // this test hermetic — rather than failing the whole call.
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("data.json"), "{\"a\": 1}\n").unwrap();
    let manager = Arc::new(LspManager::new());
    let tool = Arc::new(LspTool::new(manager, dir.path().to_path_buf()));
    let result = run(
        &tool,
        serde_json::json!({
            "action": "find_symbol", "file": "data.json", "symbol": "", "include_info": true
        }),
    );
    let text = text_of(&result);
    assert!(text.contains("info unavailable"), "{text}");
    assert!(!result.is_error);
}

#[test]
fn unknown_symbol_is_an_error() {
    let (_dir, tool) = project();
    let tool2 = Arc::clone(&tool);
    let result = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async move {
            tool2
                .execute(
                    "call".to_string(),
                    serde_json::json!({
                        "action": "replace_symbol_body", "file": "lib.rs",
                        "symbol": "Nope", "body": "x"
                    }),
                    AbortSignal::default(),
                    Arc::new(|_| {}),
                )
                .await
        });
    assert!(result.is_err(), "an unknown symbol must error: {result:?}");
}
