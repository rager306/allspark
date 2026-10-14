//! Hermetic tests for the Phase-5-A LSP glue: the write-through pipeline (L10),
//! the diagnostics delta (E6), the freshness gate (L13), the workspace checker
//! selection (L20), and the code-action selector (L19).
//!
//! None of these need a language server: every decision under test is a pure
//! function or a state machine fed by a mock clock, so the suite stays
//! dependency-free. The live server paths are covered by `live_smoke.rs` under
//! `PA_LSP_LIVE=1`.

#![forbid(unsafe_code)]

use std::path::Path;

use lsp_types::{
    CodeAction, CodeActionKind, CodeActionOrCommand, Command, Diagnostic, DiagnosticSeverity,
    NumberOrString, Position, Range, TextEdit, WorkspaceEdit,
};

use pa_lsp::diagnostics_flow::{
    detect_languages, diagnostics_delta, filter_orphan_project_diagnostics, parse_tsc,
    select_checkers, snapshot, DiagnosticsAfterEdit, FreshnessGate, WorkspaceLanguage,
    DIAGNOSTICS_AFTER_EDIT_DEFAULT, TS_ROOT_MARKERS,
};
use pa_lsp::ledger::DiagnosticsSnapshot;
use pa_lsp::tool::{select_code_action, ActionSelection};
use pa_lsp::writethrough::{apply_text_edits, plan, WriteThroughOptions, WriteThroughStep};

fn diag(message: &str, line: u32) -> Diagnostic {
    Diagnostic {
        range: Range::new(Position::new(line, 0), Position::new(line, 5)),
        severity: Some(DiagnosticSeverity::ERROR),
        code: None,
        code_description: None,
        source: Some("test".into()),
        message: message.into(),
        related_information: None,
        tags: None,
        data: None,
    }
}

fn code_action(
    title: &str,
    kind: Option<CodeActionKind>,
    edit: bool,
    preferred: bool,
) -> CodeActionOrCommand {
    CodeActionOrCommand::CodeAction(CodeAction {
        title: title.to_string(),
        kind,
        diagnostics: None,
        edit: edit.then(WorkspaceEdit::default),
        command: None,
        is_preferred: Some(preferred),
        disabled: None,
        data: None,
    })
}

// ─────────────────────────────── E6 ───────────────────────────────

#[test]
fn e6_delta_surfaces_only_the_introduced_diagnostics() {
    let before = vec![diag("pre-existing", 1)];
    let snap = snapshot(&before);
    let after = vec![diag("pre-existing", 1), diag("introduced", 2)];
    let delta = diagnostics_delta(&snap, &after);
    assert_eq!(delta.len(), 1);
    assert_eq!(delta[0].message, "introduced");
}

#[test]
fn e6_is_off_by_default() {
    const { assert!(!DIAGNOSTICS_AFTER_EDIT_DEFAULT) };
    let mut ctx = DiagnosticsAfterEdit::new();
    assert!(!ctx.enabled());
    ctx.begin(&[diag("a", 1)]);
    assert_eq!(ctx.diff(&[diag("a", 1), diag("b", 2)]).len(), 0);
}

#[test]
fn e6_snapshot_generation_gate_rejects_stale() {
    let snap = DiagnosticsSnapshot::capture(&[diag("a", 1)], 5);
    let after = vec![diag("a", 1), diag("b", 2)];
    assert_eq!(snap.diff(&after, 5).len(), 1);
    assert_eq!(snap.diff(&after, 4).len(), 0, "an older generation is stale");
}

// ─────────────────────────────── L13 ──────────────────────────────

#[test]
fn l13_freshness_gate_accepts_current_and_newer_versions() {
    let gate = FreshnessGate::new(7);
    assert!(gate.accepts(None));
    assert!(gate.accepts(Some(7)));
    assert!(gate.accepts(Some(8)));
    assert!(!gate.accepts(Some(6)));
}

#[test]
fn l13_orphan_project_diagnostics_are_filtered_for_stray_files() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("tsconfig.json"), "{}").unwrap();
    let stray = tempfile::tempdir().unwrap();
    let stray_file = stray.path().join("stray.ts");
    std::fs::write(&stray_file, "").unwrap();

    let orphan = Diagnostic {
        code: Some(NumberOrString::Number(2307)),
        ..diag("Cannot find module 'x'", 1)
    };
    let kept = filter_orphan_project_diagnostics(
        &[orphan],
        &stray_file,
        root.path(),
        TS_ROOT_MARKERS,
    );
    assert_eq!(kept.len(), 0, "the orphan is dropped");
}

// ─────────────────────────────── L20 ──────────────────────────────

#[test]
fn l20_detects_languages_and_caps_checkers() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("Cargo.toml"), "").unwrap();
    std::fs::write(root.path().join("go.mod"), "").unwrap();
    std::fs::write(root.path().join("package.json"), "").unwrap();
    let langs = detect_languages(root.path());
    assert_eq!(
        langs,
        vec![
            WorkspaceLanguage::Rust,
            WorkspaceLanguage::TypeScript,
            WorkspaceLanguage::Go
        ]
    );
    let checkers = select_checkers(root.path(), |_| true);
    assert_eq!(checkers.len(), 2, "at most two checkers run");
    assert_eq!(checkers[0].language, WorkspaceLanguage::Rust);
}

#[test]
fn l20_parses_a_tsc_diagnostic() {
    let parsed = parse_tsc(
        "src/a.ts(3,5): error TS2322: Type 'number' is not assignable to 'string'.",
        "tsc",
        Path::new("/proj"),
    );
    assert_eq!(parsed.len(), 1);
    assert_eq!(parsed[0].code.as_deref(), Some("TS2322"));
}

// ─────────────────────────────── L10 ──────────────────────────────

#[test]
fn l10_plan_shapes_the_pipeline() {
    let default = plan(&WriteThroughOptions::default(), false, false);
    assert_eq!(
        default,
        vec![WriteThroughStep::SyncContent, WriteThroughStep::NotifySaved]
    );
    let formatted = plan(&WriteThroughOptions::with_format_on_save(), true, false);
    assert!(formatted.contains(&WriteThroughStep::Format));
    let created_ts = WriteThroughOptions {
        created: true,
        ..WriteThroughOptions::default()
    };
    assert!(plan(&created_ts, false, true).contains(&WriteThroughStep::ReloadProjects));
    assert!(!plan(&created_ts, false, false).contains(&WriteThroughStep::ReloadProjects));
}

#[test]
fn l10_apply_text_edits_is_order_independent() {
    let text = "alpha\nbeta\n";
    let edits = vec![
        TextEdit {
            range: Range::new(Position::new(1, 0), Position::new(1, 4)),
            new_text: "BETA".into(),
        },
        TextEdit {
            range: Range::new(Position::new(0, 0), Position::new(0, 5)),
            new_text: "ALPHA".into(),
        },
    ];
    assert_eq!(apply_text_edits(text, &edits).unwrap(), "ALPHA\nBETA\n");
}

// ─────────────────────────── tool routing ─────────────────────────

/// The `diagnostics` action with `file:"*"` routes to the workspace runner;
/// with no project markers and no checkers it degrades to a clear message
/// rather than erroring.
#[test]
fn workspace_diagnostics_with_no_project_reports_cleanly() {
    use std::sync::Arc;

    use pa_agent::abort::AbortSignal;
    use pa_agent::types::AgentTool as _;
    use pa_lsp::{LspManager, LspTool};

    let dir = tempfile::tempdir().unwrap();
    let manager = Arc::new(LspManager::new());
    let tool = Arc::new(LspTool::new(manager, dir.path().to_path_buf()));
    let result = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async move {
            tool.execute(
                "call".to_string(),
                serde_json::json!({ "action": "diagnostics", "file": "*" }),
                AbortSignal::default(),
                Arc::new(|_| {}),
            )
            .await
        })
        .expect("workspace diagnostics must not error");
    assert!(!result.is_error);
    let text = match &result.content[0] {
        pa_agent::types::ToolResultContent::Text(text) => text.text.clone(),
        other @ pa_agent::types::ToolResultContent::Image(_) => {
            panic!("expected text content, got {other:?}")
        }
    };
    // No markers under the temp dir: the message names the empty detection.
    assert!(text.contains("no workspace checker available"), "{text}");
}

/// `rename_file` requires a `dest`; the missing-dest error is surfaced.
#[test]
fn rename_file_requires_a_destination() {
    use std::sync::Arc;

    use pa_agent::abort::AbortSignal;
    use pa_agent::types::AgentTool as _;
    use pa_lsp::{LspManager, LspTool};

    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.rs"), "").unwrap();
    let manager = Arc::new(LspManager::new());
    let tool = Arc::new(LspTool::new(manager, dir.path().to_path_buf()));
    let error = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async move {
            tool.execute(
                "call".to_string(),
                serde_json::json!({ "action": "rename_file", "file": "a.rs" }),
                AbortSignal::default(),
                Arc::new(|_| {}),
            )
            .await
        })
        .expect_err("a missing dest is an error");
    assert!(error.to_string().contains("dest"), "{error}");
}

// ─────────────────────────────── L19 ──────────────────────────────

#[test]
fn l19_selector_by_index() {
    let actions = vec![
        code_action("Fix it", Some(CodeActionKind::QUICKFIX), true, false),
        code_action("Organize imports", Some(CodeActionKind::SOURCE), true, false),
    ];
    assert_eq!(
        select_code_action(&actions, Some("2")),
        ActionSelection::Selected(1)
    );
    assert_eq!(select_code_action(&actions, Some("9")), ActionSelection::None);
}

#[test]
fn l19_selector_by_substring_is_case_insensitive() {
    let actions = vec![
        code_action("Fix it", Some(CodeActionKind::QUICKFIX), true, false),
        code_action("Organize imports", Some(CodeActionKind::SOURCE), true, false),
    ];
    assert_eq!(
        select_code_action(&actions, Some("organize")),
        ActionSelection::Selected(1)
    );
    // A substring matching the kind also works.
    assert_eq!(
        select_code_action(&actions, Some("quickfix")),
        ActionSelection::Selected(0)
    );
}

#[test]
fn l19_selector_reports_ambiguity() {
    let actions = vec![
        code_action("Fix all lint", None, true, false),
        code_action("Fix all type", None, true, false),
    ];
    match select_code_action(&actions, Some("fix all")) {
        ActionSelection::Ambiguous(candidates) => assert_eq!(candidates.len(), 2),
        other => panic!("expected ambiguity, got {other:?}"),
    }
}

#[test]
fn l19_selector_prefers_the_server_preferred_action() {
    let actions = vec![
        code_action("First", None, true, false),
        code_action("Second", None, true, true),
    ];
    assert_eq!(
        select_code_action(&actions, None),
        ActionSelection::Selected(1)
    );
}

#[test]
fn l19_selector_ignores_a_command_only_action_with_no_edit() {
    let actions = vec![CodeActionOrCommand::Command(Command {
        title: "Run it".into(),
        command: "do.thing".into(),
        arguments: None,
    })];
    assert_eq!(select_code_action(&actions, None), ActionSelection::None);
}
