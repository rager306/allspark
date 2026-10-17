//! L10 `writethrough`: the post-write LSP pipeline an edit runs so the server
//! stays in lock-step with disk and the edit result can carry fresh
//! diagnostics.
//!
//! FORK-ONLY (allspark). Ports oh-my-pi's `runLspWritethrough`
//! (`lsp/writethrough.ts:358`): after a file lands on disk the server is
//! re-synced (`syncFileContent`), optionally formatted in place, written back,
//! and notified (`notifyFileSaved`). TypeScript servers additionally get a
//! `tsserverRequest reloadProjects`, without which their cached
//! missing-module resolution "sticks" after a create/rename.
//!
//! The ordered step list is computed by the pure [`plan`] function so the
//! pipeline shape is unit-testable with no server; [`write_through`] is the
//! async driver that walks it against a live [`LspClient`].

use std::path::Path;

use lsp_types::TextEdit;

use crate::client::LspClient;
use crate::text::LineIndex;

/// What a write-through pass is allowed to do.
///
/// The four flags are the independent knobs oh-my-pi's writethrough exposes
/// (format / notify / sync / created); folding them into enums would obscure
/// the 1:1 mapping with the upstream option object.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WriteThroughOptions {
    /// Ask the server to format the document and persist the result.
    pub format_on_save: bool,
    /// Send `textDocument/didSave` after the write.
    pub notify_saved: bool,
    /// Re-sync the document (`didChange`) before saving.
    pub sync_content: bool,
    /// Whether the file was newly created (drives the tsserver reload).
    pub created: bool,
}

impl Default for WriteThroughOptions {
    fn default() -> Self {
        Self {
            format_on_save: false,
            notify_saved: true,
            sync_content: true,
            created: false,
        }
    }
}

impl WriteThroughOptions {
    /// A pass that only re-syncs and notifies (no formatting).
    #[must_use]
    pub fn sync_only() -> Self {
        Self::default()
    }

    /// A pass that also formats on save.
    #[must_use]
    pub fn with_format_on_save() -> Self {
        Self {
            format_on_save: true,
            ..Self::default()
        }
    }
}

/// One step of the write-through pipeline, in execution order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteThroughStep {
    /// `syncFileContent`: re-read disk and `didChange` if it moved.
    SyncContent,
    /// Ask the server to format; persist the result when it differs.
    Format,
    /// The caller's write itself (the file is already on disk when the pass
    /// starts, so this step is the formatted write-back).
    WriteDisk,
    /// `notifyFileSaved`: `textDocument/didSave`.
    NotifySaved,
    /// tsserver-only `reloadProjects`.
    ReloadProjects,
}

/// Compute the ordered pipeline for the given capabilities + options.
///
/// Pure, so the conditional shape (format only with a formatter; reload only
/// for tsserver) is testable without a server.
#[must_use]
pub fn plan(
    options: &WriteThroughOptions,
    has_formatter: bool,
    is_typescript: bool,
) -> Vec<WriteThroughStep> {
    let mut steps = Vec::new();
    if options.sync_content {
        steps.push(WriteThroughStep::SyncContent);
    }
    if options.format_on_save && has_formatter {
        steps.push(WriteThroughStep::Format);
        steps.push(WriteThroughStep::WriteDisk);
    }
    if options.notify_saved {
        steps.push(WriteThroughStep::NotifySaved);
    }
    // tsserver caches module resolution; a create/rename must flush it.
    if is_typescript && options.created {
        steps.push(WriteThroughStep::ReloadProjects);
    }
    steps
}

/// What a write-through pass actually did, for the edit result's `details`.
///
/// Each bool is an independent outcome bit of the pipeline; an enum would not
/// express "formatted but not reloaded" without a combinatorial type.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct WriteThroughReport {
    /// The steps that ran, in order.
    pub steps: Vec<WriteThroughStep>,
    /// Whether formatting changed the on-disk text.
    pub formatted: bool,
    /// Whether the formatted write-back succeeded.
    pub wrote_disk: bool,
    /// Whether the server was notified of the save.
    pub notified_saved: bool,
    /// Whether a tsserver `reloadProjects` was issued.
    pub reloaded_projects: bool,
}

impl WriteThroughReport {
    /// The step names, for a compact status line.
    #[must_use]
    pub fn labels(&self) -> Vec<&'static str> {
        self.steps.iter().map(|step| step.label()).collect()
    }
}

impl WriteThroughStep {
    /// A short human label.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            WriteThroughStep::SyncContent => "sync",
            WriteThroughStep::Format => "format",
            WriteThroughStep::WriteDisk => "write",
            WriteThroughStep::NotifySaved => "didSave",
            WriteThroughStep::ReloadProjects => "reloadProjects",
        }
    }
}

/// Apply a list of LSP text edits to `text`, returning the new text.
///
/// Edits are applied from the end so earlier ranges keep their offsets, and an
/// out-of-bounds range is clamped (servers occasionally overshoot on a stale
/// document). This is the in-memory half of format-on-save.
///
/// # Errors
/// Fails when an edit's start sorts after its end (a malformed server edit).
pub fn apply_text_edits(text: &str, edits: &[TextEdit]) -> anyhow::Result<String> {
    let index = LineIndex::new(text);
    let mut sorted: Vec<&TextEdit> = edits.iter().collect();
    sorted.sort_by(|a, b| {
        let ka = (a.range.start.line, a.range.start.character);
        let kb = (b.range.start.line, b.range.start.character);
        kb.cmp(&ka)
    });
    let mut out = text.to_string();
    for edit in sorted {
        let start = crate::text::byte_offset(&out, &index, edit.range.start);
        let end = crate::text::byte_offset(&out, &index, edit.range.end);
        anyhow::ensure!(
            start <= end && end <= out.len(),
            "format edit range is out of bounds"
        );
        out.replace_range(start..end, &edit.new_text);
    }
    Ok(out)
}

/// Run the L10 write-through pipeline for `path` against `client`.
///
/// Assumes `path` is already on disk (the edit wrote it). Re-syncs the server,
/// optionally formats and writes back, notifies the save, and flushes the
/// tsserver project cache on a create.
///
/// # Errors
/// Fails when a required server interaction (sync/format/save) errors. A
/// formatting error degrades to "no format" rather than failing the whole
/// write, matching oh-my-pi's tolerance of a flaky formatter.
pub async fn write_through(
    client: &LspClient,
    path: &Path,
    options: &WriteThroughOptions,
) -> anyhow::Result<WriteThroughReport> {
    let steps = plan(
        options,
        client.supports_formatting(),
        client.is_typescript(),
    );
    let mut report = WriteThroughReport {
        steps: steps.clone(),
        ..WriteThroughReport::default()
    };

    // `Format` computes the formatted text; `WriteDisk` persists it. The two
    // are separate steps so the pipeline mirrors oh-my-pi's
    // `format → sync formatted → write` ordering and the report can tell a
    // format-only pass from one that actually wrote.
    let mut pending_write: Option<String> = None;

    for step in steps {
        match step {
            WriteThroughStep::SyncContent => {
                client.ensure_open(path).await?;
            }
            WriteThroughStep::Format => {
                let text = tokio::fs::read_to_string(path).await.unwrap_or_default();
                match client.formatting(path).await {
                    Ok(Some(edits)) if !edits.is_empty() => {
                        let formatted = apply_text_edits(&text, &edits)?;
                        if formatted != text {
                            report.formatted = true;
                            pending_write = Some(formatted);
                        }
                    }
                    // No formatter edits, or a formatter error: leave the file.
                    Ok(_) | Err(_) => {}
                }
            }
            WriteThroughStep::WriteDisk => {
                if let Some(formatted) = pending_write.take() {
                    tokio::fs::write(path, &formatted).await?;
                    report.wrote_disk = true;
                    // Re-sync so the server sees the formatted bytes.
                    client.ensure_open(path).await?;
                }
            }
            WriteThroughStep::NotifySaved => {
                client.did_save(path, None)?;
                report.notified_saved = true;
            }
            WriteThroughStep::ReloadProjects => {
                let _ = client.reload_typescript_projects().await;
                report.reloaded_projects = true;
            }
        }
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use lsp_types::{Position, Range};

    fn edit(start: (u32, u32), end: (u32, u32), new_text: &str) -> TextEdit {
        TextEdit {
            range: Range::new(Position::new(start.0, start.1), Position::new(end.0, end.1)),
            new_text: new_text.to_string(),
        }
    }

    #[test]
    fn plan_syncs_and_notifies_by_default() {
        let steps = plan(&WriteThroughOptions::default(), false, false);
        assert_eq!(
            steps,
            vec![WriteThroughStep::SyncContent, WriteThroughStep::NotifySaved]
        );
    }

    #[test]
    fn plan_formats_only_when_a_formatter_exists() {
        let opts = WriteThroughOptions::with_format_on_save();
        let with = plan(&opts, true, false);
        assert_eq!(
            with,
            vec![
                WriteThroughStep::SyncContent,
                WriteThroughStep::Format,
                WriteThroughStep::WriteDisk,
                WriteThroughStep::NotifySaved,
            ]
        );
        // No formatter advertised: the format steps are dropped.
        let without = plan(&opts, false, false);
        assert!(!without.contains(&WriteThroughStep::Format));
    }

    #[test]
    fn plan_reloads_projects_only_for_a_created_typescript_file() {
        let created = WriteThroughOptions {
            created: true,
            ..WriteThroughOptions::default()
        };
        assert!(plan(&created, false, true).contains(&WriteThroughStep::ReloadProjects));
        // Not TypeScript: no reload.
        assert!(!plan(&created, false, false).contains(&WriteThroughStep::ReloadProjects));
        // TypeScript but an update (not a create): no reload.
        assert!(!plan(&WriteThroughOptions::default(), false, true)
            .contains(&WriteThroughStep::ReloadProjects));
    }

    #[test]
    fn plan_can_skip_sync_and_save() {
        let opts = WriteThroughOptions {
            sync_content: false,
            notify_saved: false,
            ..WriteThroughOptions::default()
        };
        assert_eq!(plan(&opts, true, false).len(), 0);
    }

    #[test]
    fn apply_text_edits_rewrites_from_the_end() {
        let text = "let x=1;\nlet y=2;\n";
        // Two edits on different lines; applied end-first so offsets hold.
        let edits = vec![edit((1, 4), (1, 5), "bar"), edit((0, 4), (0, 5), "foo")];
        let out = apply_text_edits(text, &edits).unwrap();
        assert_eq!(out, "let foo=1;\nlet bar=2;\n");
    }

    #[test]
    fn apply_text_edits_is_identity_for_no_edits() {
        assert_eq!(apply_text_edits("same", &[]).unwrap(), "same");
    }

    #[test]
    fn report_labels_are_stable() {
        let report = WriteThroughReport {
            steps: vec![WriteThroughStep::SyncContent, WriteThroughStep::NotifySaved],
            ..WriteThroughReport::default()
        };
        assert_eq!(report.labels(), vec!["sync", "didSave"]);
    }
}
