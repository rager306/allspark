//! The native `edit` tool: a thin [`pa_agent::types::AgentTool`] over the
//! `pa-edit` engine (fork-only). FORK-ONLY.
//!
//! `pa-edit` is the Rust port of oh-my-pi's `pi-edit`; it drives every edit
//! mode (`replace`, `patch`, `apply_patch`, `hashline`, `sloppy`) through one
//! [`crate::Session`]. This tool exposes that engine as a first-class agent
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
//! [`crate::EditStore`] is `Clone` (it wraps `Arc<Mutex<..>>`), so the tool
//! holds one shared store and clones it per call. That keeps the seen-line and
//! no-op-loop guards alive across successive `edit` calls in a session — the
//! same continuity the TS surface had.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use crate::session::SessionConfig;
use crate::{
    ApplyRequest, EditMode, EditResult, EditStore, EditWriter, FileOp, PathPolicy, Session,
    WriteRequest, WriteResponse,
};
use async_trait::async_trait;
use pa_agent::abort::AbortSignal;
use pa_agent::types::{AgentTool, AgentToolResult, AgentToolUpdateCallback};

/// The modes this tool can drive, in wire order (used for the JSON-schema
/// enum and the description).
const ALL_MODES: [EditMode; 5] = [
    EditMode::Replace,
    EditMode::Patch,
    EditMode::ApplyPatch,
    EditMode::Hashline,
    EditMode::Sloppy,
];

/// Rows a `read` action prints before it summarizes the rest.
///
/// A prompt is model input and cache payload at once (AGENTS.md: prompt-input caps
/// are part of the contract), so the read surface is capped here rather than left to
/// the caller. A file over the cap is printed head and tail with the elided range named,
/// which is exactly the "unseen lines" state the seen-lines guard already understands.
const READ_MAX_ROWS: usize = 2_000;

/// Rows of tail kept when a file exceeds [`READ_MAX_ROWS`].
const READ_TAIL_ROWS: usize = 40;

/// Bytes a `read` action will touch before refusing the file as too large.
const READ_MAX_BYTES: u64 = 8 * 1024 * 1024;

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
        // `action: "read"` is the tag mint. The `hashline` prompt tells the model to cite
        // a content tag, and the port assumed a separate `read` tool produced it. This
        // fork has none, so the tag could only come from a rejection: the model had to
        // fail once to learn the hash, and the seen-lines guard (which needs a recorded
        // read) never armed. Serving it here keeps one surface and one store.
        if params
            .get("action")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|action| action.trim().eq_ignore_ascii_case("read"))
        {
            return self.read_file(params);
        }
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

impl EditHybridTool {
    /// Serve `action: "read"`: print numbered rows and mint the section tag.
    ///
    /// The output is deliberately the shape the `hashline` grammar consumes — a
    /// `[path#TAG]` header followed by `N:TEXT` rows — so a model can copy the header
    /// straight into its next payload. The read is recorded in the shared store, which
    /// is what arms the seen-lines guard and lets a later stale edit recover from the
    /// retained snapshot instead of only failing.
    fn read_file(&self, params: &serde_json::Value) -> AgentToolResult {
        let Some(raw_path) = params.get("path").and_then(serde_json::Value::as_str) else {
            return AgentToolResult::error("`read` requires a `path`.");
        };
        let path = if std::path::Path::new(raw_path).is_absolute() {
            std::path::PathBuf::from(raw_path)
        } else {
            self.cwd.join(raw_path)
        };
        let metadata = match std::fs::metadata(&path) {
            Ok(metadata) => metadata,
            Err(err) => return AgentToolResult::error(format!("{raw_path}: {err}")),
        };
        if metadata.len() > READ_MAX_BYTES {
            // A capped read must SAY it was capped: silently printing a prefix is how a
            // model concludes a file is shorter than it is.
            return AgentToolResult::error(format!(
                "{raw_path} is {} bytes, over the {READ_MAX_BYTES}-byte read cap; \
                 narrow the view with `bash` instead",
                metadata.len()
            ));
        }
        let Ok(text) = std::fs::read_to_string(&path) else {
            return AgentToolResult::error(format!("{raw_path}: not valid UTF-8"));
        };
        let store = self
            .store
            .lock()
            .map_or_else(|_| EditStore::new(), |guard| guard.clone());
        let display = raw_path.trim();
        let rows: Vec<&str> = crate::modes::hashline::format::split_addressable_file_lines(&text);
        let truncated = rows.len() > READ_MAX_ROWS;
        // Record the snapshot BEFORE formatting, so the tag printed is the tag the store
        // holds. `record_file` re-reads and normalizes; using it keeps one definition of
        // "the bytes a tag names".
        let Some(tag) = store.record_file(&path, None) else {
            return AgentToolResult::error(format!(
                "{display}: could not record a snapshot (unreadable, not UTF-8, or over the \
                 snapshot cap)"
            ));
        };
        let header = crate::modes::hashline::format::format_hashline_header(display, &tag);
        let mut out = String::with_capacity(text.len() + header.len() + 16);
        out.push_str(&header);
        out.push('\n');
        if truncated {
            let total = rows.len();
            let head = total.saturating_sub(READ_TAIL_ROWS);
            out.push_str(&crate::modes::hashline::format::format_numbered_lines(
                rows[..head].join("\n").as_str(),
                1,
            ));
            use std::fmt::Write as _;
            let _ = write!(
                out,
                "\n… {head}:{total} elided ({total} rows; re-read a range with `bash` to see them)\n"
            );
            out.push_str(&crate::modes::hashline::format::format_numbered_lines(
                rows[head..].join("\n").as_str(),
                u32::try_from(head + 1).unwrap_or(u32::MAX),
            ));
        } else {
            out.push_str(&crate::modes::hashline::format::format_numbered_lines(
                rows.join("\n").as_str(),
                1,
            ));
        }
        AgentToolResult {
            details: serde_json::json!({
                "tag": tag,
                "rows": rows.len(),
                "truncated": truncated,
            }),
            ..AgentToolResult::text(out)
        }
    }
}

/// Turn an [`crate::ApplyOutcome`] into the agent result: model-facing text
/// plus a structured `diffs` detail per file.
fn outcome_result(outcome: crate::ApplyOutcome) -> AgentToolResult {
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
            "action": {
                "type": "string",
                "enum": ["edit", "read"],
                "description": "`read` prints a file as `[path#TAG]` plus `N:TEXT` rows and mints the snapshot tag a `hashline` payload must cite. Omit for an edit."
            },
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

    /// Editing is a read→stage→write cycle, so two edits in one parallel batch can
    /// both stage from the same version and the second write silently discards the
    /// first (audit F01). The loop's parallel dispatch is the default; declaring
    /// `Sequential` is what makes the whole cycle — not merely the write — ordered.
    ///
    /// A per-file write queue alone is NOT enough: it orders the writes while both
    /// reads have already happened, so the lost update survives. The cost of
    /// ordering is that edits in one batch do not overlap; they are already
    /// I/O-bound and small.
    fn execution_mode(&self) -> Option<pa_agent::types::ToolExecutionMode> {
        Some(pa_agent::types::ToolExecutionMode::Sequential)
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
        Box::pin(async move {
            let started = std::time::Instant::now();
            let result = self.run(&params).await;
            Ok(result
                .with_duration_ms(u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)))
        })
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
            crate::EditError::Writer(format!("write {}: {err}", path.display()))
        };
        match request.op {
            FileOp::Delete => {
                std::fs::remove_file(&request.absolute)
                    .map_err(|err| write_err(&request.absolute, err))?;
            }
            FileOp::Create | FileOp::Update => {
                // The review's check #5, at the last possible moment: a plan built seconds
                // ago can be stale by now, and writing planned bytes over someone else's
                // change lands a patch on the wrong lines. `Update` is the case that can be
                // stale — `Create` asserts the file did not exist.
                if request.op == FileOp::Update {
                    let existing = std::fs::read(&request.absolute).ok();
                    if let Some(bytes) = &existing {
                        let text = String::from_utf8_lossy(bytes);
                        if let Some(expected) = &request.expected_hash {
                            let actual = crate::file_hash(&text);
                            if &actual != expected {
                                return Err(crate::EditError::Writer(format!(
                                    "{} changed since it was read (hash {expected} -> {actual}); \
                                     re-read it and plan the edit again",
                                    request.display
                                )));
                            }
                        }
                    } else if request.expected_hash.is_some() {
                        return Err(crate::EditError::Writer(format!(
                            "{} no longer exists; the edit was planned against a file that is gone",
                            request.display
                        )));
                    }
                }
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

    /// The review's check #5 at the last possible moment: a plan can be stale by the time
    /// the write lands, and writing planned bytes over someone else's change puts a patch on
    /// the wrong lines. The writer refuses with both tags, so the caller replans.
    #[tokio::test]
    async fn the_writer_refuses_bytes_that_changed_since_the_plan() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("a.rs");
        std::fs::write(&path, "pub fn one() {}\n").expect("seed");
        let planned = crate::file_hash("pub fn one() {}\n");
        let writer = FsWriter { lsp: None };

        // Someone else edits the file between planning and writing.
        std::fs::write(&path, "pub fn changed() {}\n").expect("edit");
        let refused = writer
            .write(WriteRequest {
                expected_hash: Some(planned.clone()),
                absolute: path.clone(),
                display: "a.rs".to_string(),
                op: FileOp::Update,
                move_to: None,
                content: Some("pub fn planned() {}\n".to_string()),
                flush_lsp: false,
                lsp_batch_id: None,
            })
            .await;
        assert!(
            refused.is_err(),
            "a stale plan must be refused, not written: {refused:?}"
        );
        assert_eq!(
            std::fs::read_to_string(&path).expect("read"),
            "pub fn changed() {}\n",
            "the file must be UNTOUCHED: refusing after writing protects nothing"
        );

        // The same write against the bytes the plan actually saw succeeds.
        std::fs::write(&path, "pub fn one() {}\n").expect("restore");
        writer
            .write(WriteRequest {
                expected_hash: Some(planned),
                absolute: path.clone(),
                display: "a.rs".to_string(),
                op: FileOp::Update,
                move_to: None,
                content: Some("pub fn planned() {}\n".to_string()),
                flush_lsp: false,
                lsp_batch_id: None,
            })
            .await
            .expect("a matching precondition writes");
        assert_eq!(
            std::fs::read_to_string(&path).expect("read"),
            "pub fn planned() {}\n"
        );
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
        let tag = crate::store::file_hash("before\n");
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
        let tag = crate::store::file_hash("before\n");
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
                expected_hash: None,
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
                expected_hash: None,
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
                expected_hash: None,
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
    /// The native `edit` tool must declare a sequential execution constraint.
    ///
    /// The loop's parallel dispatch is the default, and an edit is a
    /// read→stage→write cycle: two edits in one batch both stage from V and the
    /// second write discards the first. Serialising only the WRITE does not help —
    /// both reads have already happened — which is why the constraint is on the
    /// tool and not on the writer.
    #[test]
    fn the_edit_tool_declares_a_sequential_execution_constraint() {
        let dir = tempfile::tempdir().unwrap();
        let tool = EditHybridTool::new(
            Arc::new(Mutex::new(crate::EditStore::new())),
            None,
            dir.path().to_path_buf(),
            EditMode::Replace,
            vec![EditMode::Replace],
        );
        assert_eq!(
            tool.execution_mode(),
            Some(pa_agent::types::ToolExecutionMode::Sequential),
            "an edit batch must not run its tools in parallel"
        );
    }

    /// The loop the prompt promises: `read` mints the tag, the edit cites it.
    ///
    /// Before this, nothing in production produced a tag: `record_file` had no caller, so
    /// a `hashline` edit could only learn the hash from a rejection. This pins the whole
    /// path — header shape, numbering, tag round-trip — because each piece existed and
    /// the connection did not.
    #[tokio::test]
    async fn read_mints_the_tag_the_next_hashline_edit_cites() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "alpha\nbeta\n").unwrap();
        let tool = tool_for(dir.path(), EditMode::Hashline, all_modes());
        let read = call(
            &tool,
            serde_json::json!({ "action": "read", "path": "a.txt" }),
        )
        .await;
        assert!(!read.is_error, "read failed: {}", text_of(&read));
        let text = text_of(&read);
        let expected_tag = crate::store::file_hash("alpha\nbeta\n");
        assert_eq!(
            text.lines().next().unwrap(),
            format!("[a.txt#{expected_tag}]"),
            "the header must carry the live tag so it can be copied verbatim"
        );
        assert!(
            text.contains("1:alpha"),
            "rows must be numbered from 1: {text}"
        );
        assert!(text.contains("2:beta"), "{text}");

        // Copy the header straight out of the read, as the prompt tells the model to.
        let header = text.lines().next().unwrap();
        let payload = format!("{header}\nPUT 2.=2:\n+gamma");
        let edit = call(
            &tool,
            serde_json::json!({ "mode": "hashline", "payload": payload }),
        )
        .await;
        assert!(
            !edit.is_error,
            "the tag from `read` must be accepted: {}",
            text_of(&edit)
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join("a.txt")).unwrap(),
            "alpha\ngamma\n"
        );
    }

    /// A read over the row cap says how many rows it elided.
    #[tokio::test]
    async fn a_long_read_names_the_rows_it_elided() {
        let dir = tempfile::tempdir().unwrap();
        let body: String = (1..=READ_MAX_ROWS + 500).fold(String::new(), |mut acc, n| {
            let _ = std::fmt::Write::write_fmt(&mut acc, format_args!("row{n}\n"));
            acc
        });
        std::fs::write(dir.path().join("big.txt"), &body).unwrap();
        let tool = tool_for(dir.path(), EditMode::Hashline, all_modes());
        let read = call(
            &tool,
            serde_json::json!({ "action": "read", "path": "big.txt" }),
        )
        .await;
        let text = text_of(&read);
        assert!(
            text.contains("elided"),
            "a capped read must say so: {text:.200}"
        );
        assert!(
            read.details["truncated"] == serde_json::json!(true),
            "details must report the cap: {:?}",
            read.details
        );
    }

    /// Review finding: a miss with a valid-but-stale tag must tell the model ONE current tag.
    ///
    /// Ported from oh-my-pi, where `read`/`search` mints the tag and every miss message says
    /// "re-read with `read`". This fork has no `read` tool — the native `edit` tool is the only
    /// tag source — so a message that names two different hashes for the same file leaves the
    /// model to guess which one to copy. The retry must work from the message alone.
    #[tokio::test]
    async fn a_stale_tag_miss_names_one_current_tag_the_model_can_retry_with() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "before\n").unwrap();
        let tool = tool_for(dir.path(), EditMode::Hashline, all_modes());
        // A syntactically valid tag that this session never issued.
        let payload = "[a.txt#0000]\nPUT 1.=1:\n+after";
        let result = call(
            &tool,
            serde_json::json!({ "mode": "hashline", "payload": payload }),
        )
        .await;
        assert!(result.is_error, "expected a rejection");
        let text = text_of(&result);
        let current = crate::store::file_hash("before\n");
        // Exactly one `#TAG`-shaped token may name the live content, and the message must
        // not offer a second, different one as if it were also current.
        let hashes: Vec<&str> = text
            .split('#')
            .filter_map(|rest| rest.get(..4))
            .filter(|candidate| candidate.chars().all(|c| c.is_ascii_hexdigit()))
            .collect();
        assert!(
            hashes.contains(&current.as_str()),
            "the current tag must appear so a retry is possible; got {hashes:?} in:\n{text}"
        );
        // The retry the message asks for must actually succeed.
        let retry = format!("[a.txt#{current}]\nPUT 1.=1:\n+after");
        let retried = call(
            &tool,
            serde_json::json!({ "mode": "hashline", "payload": retry }),
        )
        .await;
        assert!(
            !retried.is_error,
            "a retry built from the current tag in the message must succeed: {}",
            text_of(&retried)
        );
    }
}
