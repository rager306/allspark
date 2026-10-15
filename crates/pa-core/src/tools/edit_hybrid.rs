//! The native `edit` tool: a thin [`pa_agent::types::AgentTool`] over the
//! `pa-edit` engine (fork-only). FORK-ONLY.
//!
//! `pa-edit` is the Rust port of oh-my-pi's `pi-edit`; it drives every edit
//! mode (`replace`, `patch`, `apply_patch`, `hashline`, `sloppy`) through one
//! [`pa_edit::Session`]. This tool exposes that engine as a first-class agent
//! tool so the model can edit files without the Python `skills/edit` facade.
//!
//! The Python facade (`skills/edit`) and the TS-parity `crates/pa-core/src/
//! tools/edit.rs` + `golden_replay.rs` are deliberately untouched: this is a
//! separate, additive surface.
//!
//! ## Wire shape
//!
//! One JSON argument object:
//! - `mode`: one of the engine's modes (defaults to the session default).
//! - `path` + `edits` (`[{ oldText, newText, replaceAll? }]`): the JSON form
//!   for `replace`.
//! - `payload`: the raw, non-JSON wire text for the custom-format modes
//!   (`patch`, `apply_patch`, `hashline`, `sloppy`).
//!
//! ## Store lifetime
//!
//! [`pa_edit::EditStore`] is `Clone` (it wraps `Arc<Mutex<..>>`), so the tool
//! holds one shared store and clones it per call. That keeps the seen-line and
//! no-op-loop guards alive across successive `edit` calls in a session — the
//! same continuity the TS surface had.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use pa_agent::abort::AbortSignal;
use pa_agent::types::{AgentTool, AgentToolResult, AgentToolUpdateCallback};
use pa_edit::session::SessionConfig;
use pa_edit::{
    ApplyRequest, EditMode, EditResult, EditStore, EditWriter, FileOp, PathPolicy, Session,
    WriteRequest, WriteResponse,
};

/// The modes this tool can drive, in wire order (used for the JSON-schema
/// enum and the description).
const ALL_MODES: [EditMode; 5] = [
    EditMode::Replace,
    EditMode::Patch,
    EditMode::ApplyPatch,
    EditMode::Hashline,
    EditMode::Sloppy,
];

/// The native `edit` tool.
pub struct EditHybridTool {
    /// Shared across calls so seen-line / no-op guards survive a session.
    store: Arc<Mutex<EditStore>>,
    /// Optional LSP manager for the post-write diagnostics pass.
    lsp: Option<Arc<pa_lsp::LspManager>>,
    /// Working directory relative `path`s resolve against.
    cwd: PathBuf,
    /// The mode used when a call omits `mode`.
    default_mode: EditMode,
    /// The modes a call may request; anything else is rejected.
    allow_modes: Vec<EditMode>,
    /// JSON-schema parameters.
    schema: serde_json::Value,
}

impl EditHybridTool {
    /// Build the tool. `allow_modes` is the set of requestable modes; the
    /// default mode must be a member (the caller guarantees it).
    #[must_use]
    pub fn new(
        store: Arc<Mutex<EditStore>>,
        lsp: Option<Arc<pa_lsp::LspManager>>,
        cwd: PathBuf,
        default_mode: EditMode,
        allow_modes: Vec<EditMode>,
    ) -> Self {
        let schema = params_schema(&allow_modes);
        Self {
            store,
            lsp,
            cwd,
            default_mode,
            allow_modes,
            schema,
        }
    }

    /// The mode a call requests: its `mode` argument, or the default.
    fn resolve_mode(&self, params: &serde_json::Value) -> Result<EditMode, String> {
        let Some(raw) = params.get("mode").and_then(serde_json::Value::as_str) else {
            return Ok(self.default_mode);
        };
        let raw = raw.trim();
        if raw.is_empty() {
            return Ok(self.default_mode);
        }
        let mode = EditMode::parse(raw).ok_or_else(|| format!("Unknown edit mode `{raw}`."))?;
        if !self.allow_modes.contains(&mode) {
            let allowed = self
                .allow_modes
                .iter()
                .map(|mode| mode.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            return Err(format!(
                "Edit mode `{}` is not enabled in this session. Allowed modes: {allowed}.",
                mode.as_str()
            ));
        }
        Ok(mode)
    }

    /// The `replace`-mode JSON argument object from a call's `path`/`edits`.
    #[allow(clippy::unused_self)]
    fn replace_args_json(&self, params: &serde_json::Value) -> Result<String, String> {
        let path = params
            .get("path")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|path| !path.is_empty())
            .ok_or_else(|| "`path` is required for replace mode.".to_string())?;
        let edits = params
            .get("edits")
            .and_then(serde_json::Value::as_array)
            .ok_or_else(|| "`edits` is required for replace mode.".to_string())?;
        if edits.is_empty() {
            return Err("`edits` must not be empty for replace mode.".to_string());
        }
        let mut out = Vec::with_capacity(edits.len());
        for (index, entry) in edits.iter().enumerate() {
            let old_text = entry
                .get("oldText")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| format!("edits[{index}].oldText is required."))?;
            let new_text = entry
                .get("newText")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| format!("edits[{index}].newText is required."))?;
            let replace_all = entry
                .get("replaceAll")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false);
            out.push(serde_json::json!({
                "old_string": old_text,
                "new_string": new_text,
                "replace_all": replace_all,
            }));
        }
        let object = serde_json::json!({ "path": path, "edits": out });
        Ok(object.to_string())
    }

    /// Build the per-call [`SessionConfig`].
    fn session_config(&self, mode: EditMode) -> SessionConfig {
        let home_dir = pa_types::platform::home_dir().unwrap_or_else(|| self.cwd.clone());
        SessionConfig {
            mode,
            policy: PathPolicy {
                cwd: self.cwd.clone(),
                home_dir,
                url_schemes: Vec::new(),
                url_alias_schemes: Vec::new(),
                plan_writable_roots: Vec::new(),
                plan_active: false,
                block_auto_generated: false,
            },
            allow_fuzzy: mode == EditMode::Sloppy,
            fuzzy_threshold: 0.8,
            enforce_seen_lines: mode == EditMode::Hashline,
            raw_input: mode != EditMode::Replace,
        }
    }

    /// Run one edit call end to end.
    async fn run(&self, params: &serde_json::Value) -> AgentToolResult {
        let mode = match self.resolve_mode(params) {
            Ok(mode) => mode,
            Err(message) => return AgentToolResult::error(message),
        };
        let config = self.session_config(mode);
        let store = self
            .store
            .lock()
            .map_or_else(|_| EditStore::new(), |guard| guard.clone());
        let mut session = Session::new(config, store);
        if mode == EditMode::Replace {
            match self.replace_args_json(params) {
                Ok(json) => session.set_args_json(&json),
                Err(message) => return AgentToolResult::error(message),
            }
        } else {
            let Some(payload) = params.get("payload").and_then(serde_json::Value::as_str) else {
                return AgentToolResult::error(format!(
                    "`payload` is required for {} mode.",
                    mode.as_str()
                ));
            };
            session.push(payload);
        }
        session.finish();

        let writer = FsWriter {
            lsp: self.lsp.clone(),
        };
        // A single-file apply flushes LSP on the last write; only ask for it
        // when a manager is present (otherwise the writer skips it anyway).
        let request = ApplyRequest {
            lsp_batch_id: None,
            lsp_flush: self.lsp.is_some(),
        };
        match session.apply(request, &writer).await {
            Ok(outcome) => outcome_result(outcome),
            Err(err) => AgentToolResult::error(err.to_string()),
        }
    }
}

/// Turn an [`pa_edit::ApplyOutcome`] into the agent result: model-facing text
/// plus a structured `diffs` detail per file.
fn outcome_result(outcome: pa_edit::ApplyOutcome) -> AgentToolResult {
    let mut text = outcome.text;
    let mut diffs = Vec::with_capacity(outcome.files.len());
    for file in &outcome.files {
        if file.parse_regressed {
            let warning = format!(
                "WARNING: parse regression after edit (file {})",
                file.display
            );
            if !text.contains(&warning) {
                if !text.is_empty() {
                    text.push_str("\n\n");
                }
                text.push_str(&warning);
            }
        }
        diffs.push(serde_json::json!({
            "path": file.display,
            "diff": file.diff,
            "firstChangedLine": file.first_changed_line,
        }));
    }
    AgentToolResult {
        content: vec![pa_agent::types::ToolResultContent::text(text)],
        details: serde_json::json!({ "diffs": diffs }),
        terminate: None,
        is_error: false,
    }
}

/// The JSON-Schema parameters. `required` stays empty (mode-dependent) and
/// `execute` validates the shape.
fn params_schema(allow_modes: &[EditMode]) -> serde_json::Value {
    let enum_values: Vec<&str> = ALL_MODES
        .iter()
        .filter(|mode| allow_modes.contains(mode))
        .map(|mode| mode.as_str())
        .collect();
    serde_json::json!({
        "type": "object",
        "properties": {
            "mode": {
                "type": "string",
                "enum": enum_values,
                "description": "The edit mode. Defaults to the session's default mode."
            },
            "path": {
                "type": "string",
                "description": "Target file (relative to the working directory). Required for `replace`."
            },
            "edits": {
                "type": "array",
                "description": "Replace-mode edits: [{ oldText, newText, replaceAll? }].",
                "items": {
                    "type": "object",
                    "properties": {
                        "oldText": { "type": "string" },
                        "newText": { "type": "string" },
                        "replaceAll": { "type": "boolean" }
                    },
                    "required": ["oldText", "newText"]
                }
            },
            "payload": {
                "type": "string",
                "description": "Raw wire payload for the custom-format modes (patch, apply_patch, hashline, sloppy)."
            }
        },
        "required": []
    })
}

impl AgentTool for EditHybridTool {
    fn name(&self) -> &'static str {
        "edit"
    }

    fn description(&self) -> &'static str {
        // A short, static headline; the mode list is appended at runtime via
        // `label`/schema. The static form keeps the `&'static str` contract.
        "Edit files through the pa-edit engine (replace, patch, apply_patch, hashline, sloppy). \
         `replace` uses `path` + `edits`; the other modes use a raw `payload` string."
    }

    fn parameters(&self) -> &serde_json::Value {
        &self.schema
    }

    fn label(&self) -> &'static str {
        "Edit"
    }

    fn execute(
        self: Arc<Self>,
        _tool_call_id: String,
        params: serde_json::Value,
        _signal: AbortSignal,
        _on_update: AgentToolUpdateCallback,
    ) -> pa_agent::BoxFut<'static, anyhow::Result<AgentToolResult>> {
        Box::pin(async move { Ok(self.run(&params).await) })
    }
}

/// The local-filesystem [`EditWriter`]. Writes go through `std::fs`; when a
/// flush is requested and an LSP manager is present, a write-through pass runs
/// and its diagnostics ride the response.
struct FsWriter {
    lsp: Option<Arc<pa_lsp::LspManager>>,
}

#[async_trait]
impl EditWriter for FsWriter {
    async fn write(&self, request: WriteRequest) -> EditResult<WriteResponse> {
        let write_err = |path: &std::path::Path, err: std::io::Error| {
            pa_edit::EditError::Writer(format!("write {}: {err}", path.display()))
        };
        match request.op {
            FileOp::Delete => {
                std::fs::remove_file(&request.absolute)
                    .map_err(|err| write_err(&request.absolute, err))?;
            }
            FileOp::Create | FileOp::Update => {
                if let Some(content) = &request.content {
                    if let Some(parent) = request.absolute.parent() {
                        std::fs::create_dir_all(parent).map_err(|err| write_err(parent, err))?;
                    }
                    std::fs::write(&request.absolute, content)
                        .map_err(|err| write_err(&request.absolute, err))?;
                }
                if let Some(dest) = &request.move_to {
                    if let Some(parent) = dest.parent() {
                        std::fs::create_dir_all(parent).map_err(|err| write_err(parent, err))?;
                    }
                    std::fs::rename(&request.absolute, dest).map_err(|err| write_err(dest, err))?;
                }
            }
            FileOp::Noop => {}
        }

        let written = request.content.clone().unwrap_or_default();
        let mut diagnostics_json = None;
        if let (true, Some(lsp)) = (request.flush_lsp, &self.lsp) {
            let target = request.move_to.as_ref().unwrap_or(&request.absolute);
            let options = pa_lsp::WriteThroughOptions {
                created: request.op == FileOp::Create,
                ..pa_lsp::WriteThroughOptions::default()
            };
            match lsp.write_through(target, &options).await {
                Ok(Some(report)) => {
                    diagnostics_json =
                        Some(serde_json::json!({ "writethrough": report.labels() }).to_string());
                }
                Ok(None) => {}
                Err(err) => {
                    tracing::warn!(
                        "edit: LSP write-through failed for {}: {err}",
                        target.display()
                    );
                }
            }
        }

        Ok(WriteResponse {
            written,
            diagnostics_json,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a tool over a fresh temp workspace; returns (tempdir, tool, dir).
    fn tool_for(
        dir: &std::path::Path,
        default_mode: EditMode,
        allow_modes: Vec<EditMode>,
    ) -> EditHybridTool {
        let store = Arc::new(Mutex::new(EditStore::new()));
        EditHybridTool::new(store, None, dir.to_path_buf(), default_mode, allow_modes)
    }

    fn all_modes() -> Vec<EditMode> {
        ALL_MODES.to_vec()
    }

    async fn call(tool: &EditHybridTool, params: serde_json::Value) -> AgentToolResult {
        tool.run(&params).await
    }

    fn text_of(result: &AgentToolResult) -> String {
        result
            .content
            .iter()
            .filter_map(|content| match content {
                pa_agent::types::ToolResultContent::Text(text) => Some(text.text.clone()),
                pa_agent::types::ToolResultContent::Image(_) => None,
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[tokio::test]
    async fn replace_round_trip_edits_the_file_and_reports_a_diff() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "hello world\n").unwrap();
        let tool = tool_for(dir.path(), EditMode::Replace, all_modes());
        let result = call(
            &tool,
            serde_json::json!({
                "mode": "replace",
                "path": "a.txt",
                "edits": [{ "oldText": "hello", "newText": "goodbye" }],
            }),
        )
        .await;
        assert!(!result.is_error, "text: {}", text_of(&result));
        let written = std::fs::read_to_string(dir.path().join("a.txt")).unwrap();
        assert_eq!(written, "goodbye world\n");
        let diffs = result.details["diffs"].as_array().unwrap();
        assert_eq!(diffs.len(), 1);
        assert!(diffs[0]["diff"].as_str().unwrap().contains("goodbye"));
    }

    #[tokio::test]
    async fn replace_defaults_to_the_session_mode() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "one\n").unwrap();
        let tool = tool_for(dir.path(), EditMode::Replace, all_modes());
        // No `mode` key: the default (replace) is used.
        let result = call(
            &tool,
            serde_json::json!({
                "path": "a.txt",
                "edits": [{ "oldText": "one", "newText": "two" }],
            }),
        )
        .await;
        assert!(!result.is_error, "text: {}", text_of(&result));
        assert_eq!(
            std::fs::read_to_string(dir.path().join("a.txt")).unwrap(),
            "two\n"
        );
    }

    #[tokio::test]
    async fn mode_gating_rejects_a_disallowed_mode() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "x\n").unwrap();
        let tool = tool_for(dir.path(), EditMode::Replace, vec![EditMode::Replace]);
        let result = call(
            &tool,
            serde_json::json!({ "mode": "hashline", "payload": "[a.txt#0000]\nPUT 1.=1:\n+y" }),
        )
        .await;
        assert!(result.is_error, "expected an error");
        let text = text_of(&result);
        assert!(text.contains("hashline"), "text: {text}");
        assert!(text.contains("replace"), "lists allowed modes: {text}");
        // The file was not touched.
        assert_eq!(
            std::fs::read_to_string(dir.path().join("a.txt")).unwrap(),
            "x\n"
        );
    }

    #[tokio::test]
    async fn unknown_mode_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let tool = tool_for(dir.path(), EditMode::Replace, all_modes());
        let result = call(&tool, serde_json::json!({ "mode": "bogus" })).await;
        assert!(result.is_error);
        assert!(text_of(&result).contains("bogus"));
    }

    #[tokio::test]
    async fn replace_requires_path_and_edits() {
        let dir = tempfile::tempdir().unwrap();
        let tool = tool_for(dir.path(), EditMode::Replace, all_modes());
        let no_path = call(&tool, serde_json::json!({ "mode": "replace" })).await;
        assert!(no_path.is_error);
        assert!(text_of(&no_path).contains("path"));
        let no_edits = call(
            &tool,
            serde_json::json!({ "mode": "replace", "path": "a.txt" }),
        )
        .await;
        assert!(no_edits.is_error);
        assert!(text_of(&no_edits).contains("edits"));
    }

    #[tokio::test]
    async fn custom_mode_requires_a_payload() {
        let dir = tempfile::tempdir().unwrap();
        let tool = tool_for(dir.path(), EditMode::Hashline, all_modes());
        let result = call(&tool, serde_json::json!({ "mode": "hashline" })).await;
        assert!(result.is_error);
        assert!(text_of(&result).contains("payload"));
    }

    #[tokio::test]
    async fn hashline_round_trip_with_a_live_tag() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "before\n").unwrap();
        let tag = pa_edit::store::file_hash("before\n");
        let tool = tool_for(dir.path(), EditMode::Hashline, all_modes());
        let payload = format!("[a.txt#{tag}]\nPUT 1.=1:\n+after");
        let result = call(
            &tool,
            serde_json::json!({ "mode": "hashline", "payload": payload }),
        )
        .await;
        assert!(!result.is_error, "text: {}", text_of(&result));
        assert_eq!(
            std::fs::read_to_string(dir.path().join("a.txt")).unwrap(),
            "after\n"
        );
    }

    #[tokio::test]
    async fn hashline_stale_tag_is_reported_as_an_error() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "before\n").unwrap();
        let tool = tool_for(dir.path(), EditMode::Hashline, all_modes());
        // A tag that matches neither the live content nor any recorded snapshot.
        let payload = "[a.txt#DEAD]\nPUT 1.=1:\n+after";
        let result = call(
            &tool,
            serde_json::json!({ "mode": "hashline", "payload": payload }),
        )
        .await;
        assert!(result.is_error, "text: {}", text_of(&result));
        // The file is untouched.
        assert_eq!(
            std::fs::read_to_string(dir.path().join("a.txt")).unwrap(),
            "before\n"
        );
    }

    #[tokio::test]
    async fn replace_noop_is_rejected_by_the_engine() {
        // A replace whose `oldText` and `newText` are byte-identical leaves the
        // file unchanged; the replace engine rejects that with
        // "resulted in no changes being made".
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "same\n").unwrap();
        let tool = tool_for(dir.path(), EditMode::Replace, all_modes());
        let result = call(
            &tool,
            serde_json::json!({
                "mode": "replace",
                "path": "a.txt",
                "edits": [{ "oldText": "same", "newText": "same" }],
            }),
        )
        .await;
        assert!(result.is_error, "text: {}", text_of(&result));
        assert!(text_of(&result).contains("no changes"));
    }

    #[tokio::test]
    async fn shared_store_escalates_a_repeated_hashline_noop() {
        // The tool holds one shared store across calls, so the hashline
        // no-op-loop guard (NOOP_HARD_LIMIT = 3) escalates on the third
        // byte-identical no-op payload for the same file.
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "before\n").unwrap();
        let tag = pa_edit::store::file_hash("before\n");
        let tool = tool_for(dir.path(), EditMode::Hashline, all_modes());
        // A payload that parses but changes nothing (body == existing line).
        let payload = format!("[a.txt#{tag}]\nPUT 1.=1:\n+before");
        let mut last = None;
        for _ in 0..3 {
            last = Some(
                call(
                    &tool,
                    serde_json::json!({ "mode": "hashline", "payload": payload }),
                )
                .await,
            );
        }
        let result = last.unwrap();
        assert!(
            result.is_error,
            "third no-op should escalate: {}",
            text_of(&result)
        );
        assert!(
            text_of(&result).contains("no-op"),
            "text: {}",
            text_of(&result)
        );
    }

    #[tokio::test]
    async fn fs_writer_deletes_and_moves() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src.txt");
        std::fs::write(&src, "content\n").unwrap();
        let writer = FsWriter { lsp: None };

        // Delete (content None).
        writer
            .write(WriteRequest {
                absolute: src.clone(),
                display: "src.txt".to_string(),
                op: FileOp::Delete,
                move_to: None,
                content: None,
                flush_lsp: false,
                lsp_batch_id: None,
            })
            .await
            .unwrap();
        assert!(!src.exists());

        // Create + move.
        let created = dir.path().join("created.txt");
        let dest = dir.path().join("moved.txt");
        writer
            .write(WriteRequest {
                absolute: created.clone(),
                display: "created.txt".to_string(),
                op: FileOp::Create,
                move_to: Some(dest.clone()),
                content: Some("moved\n".to_string()),
                flush_lsp: false,
                lsp_batch_id: None,
            })
            .await
            .unwrap();
        assert!(!created.exists());
        assert_eq!(std::fs::read_to_string(&dest).unwrap(), "moved\n");
    }

    #[tokio::test]
    async fn fs_writer_without_lsp_does_not_error_on_a_flush_request() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.txt");
        let writer = FsWriter { lsp: None };
        let response = writer
            .write(WriteRequest {
                absolute: path.clone(),
                display: "a.txt".to_string(),
                op: FileOp::Create,
                move_to: None,
                content: Some("x\n".to_string()),
                flush_lsp: true,
                lsp_batch_id: None,
            })
            .await
            .unwrap();
        assert_eq!(response.written, "x\n");
        assert!(response.diagnostics_json.is_none());
    }

    #[test]
    fn schema_lists_the_allowed_modes() {
        let schema = params_schema(&[EditMode::Replace, EditMode::Hashline]);
        let enum_values = schema["properties"]["mode"]["enum"].as_array().unwrap();
        let names: Vec<&str> = enum_values.iter().filter_map(|v| v.as_str()).collect();
        assert_eq!(names, vec!["replace", "hashline"]);
    }
}
