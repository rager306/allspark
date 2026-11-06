//! Mode-neutral engine contract.
//!
//! Every edit mode (`replace`, `patch`, `apply_patch`, `hashline`, `sloppy`)
//! implements [`ModeEngine`]: a pure, in-memory transformation from a parsed
//! argument snapshot plus target file contents into staged file states.
//! Nothing here touches the filesystem for writes; the [`crate::session`]
//! drives previews, staging, and the host writer callback.

use std::path::PathBuf;

use crate::{error::EditError, files::FileSource, store::EditStore, stream_json::ArgSnapshot};

/// The edit variants the tool exposes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EditMode {
    Replace,
    Patch,
    ApplyPatch,
    Hashline,
    Sloppy,
}

impl EditMode {
    /// Wire/settings name (`replace`, `patch`, `apply_patch`, `hashline`,
    /// `sloppy`).
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Replace => "replace",
            Self::Patch => "patch",
            Self::ApplyPatch => "apply_patch",
            Self::Hashline => "hashline",
            Self::Sloppy => "sloppy",
        }
    }

    /// Parse a wire/settings name.
    pub fn parse(name: &str) -> Option<Self> {
        Some(match name {
            "replace" => Self::Replace,
            "patch" => Self::Patch,
            "apply_patch" => Self::ApplyPatch,
            "hashline" => Self::Hashline,
            "sloppy" => Self::Sloppy,
            _ => return None,
        })
    }
}

/// The modes a bare `edit` call may use, and the one it does — resolved once.
///
/// Two owners computed this and disagreed: the TOOL took `editHybrid.defaultMode`
/// (defaulting to `replace`) and fell back to the first ALLOWED mode only when the default
/// was not in the set, while the PROMPT took the first allowed mode whenever no default was
/// configured. With `allowModes: ["hashline", "replace"]` and no default, the tool used
/// `replace` and the prompt told the model `hashline` was the default — so every bare call
/// did something other than what the prompt promised (review #28, the half the first fix
/// left). One function, two callers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditPolicy {
    /// Modes a call may request, in the configured order.
    pub allowed: Vec<EditMode>,
    /// The mode a call omitting `mode` uses. Always a member of `allowed`.
    pub default: EditMode,
}

impl EditPolicy {
    /// Resolve from the raw settings spellings.
    ///
    /// `allow` empty or unparseable means "every mode"; a `default` outside `allow` (or an
    /// unparseable one) falls back to the FIRST allowed mode, so the pair is never
    /// inconsistent and the tool is never dropped.
    #[must_use]
    pub fn resolve(default: Option<&str>, allow: &[String]) -> Self {
        let mut allowed: Vec<EditMode> = allow
            .iter()
            .filter_map(|mode| EditMode::parse(mode))
            .collect();
        if allowed.is_empty() {
            allowed = vec![
                EditMode::Replace,
                EditMode::Patch,
                EditMode::ApplyPatch,
                EditMode::Hashline,
                EditMode::Sloppy,
            ];
        }
        let requested = default.and_then(EditMode::parse);
        let default = match requested {
            Some(mode) if allowed.contains(&mode) => mode,
            // No default configured: `replace` is the documented one, and it is honoured
            // whenever it is requestable. Taking `allowed.first()` here is what made the
            // prompt and the tool disagree.
            None if allowed.contains(&EditMode::Replace) => EditMode::Replace,
            _ => allowed.first().copied().unwrap_or(EditMode::Replace),
        };
        Self { allowed, default }
    }
}

/// What a staged file does to the filesystem.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileOp {
    Create,
    Update,
    Delete,
    /// Parsed and applied cleanly but produced no change; nothing is written.
    Noop,
}

impl FileOp {
    /// Wire label (`create` / `update` / `delete`); `Noop` reports as `update`
    /// like the TypeScript result shape did.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Create => "create",
            Self::Update | Self::Noop => "update",
            Self::Delete => "delete",
        }
    }
}

/// A resolved edit target: the absolute filesystem path plus the display
/// path the model authored (after unique-suffix recovery, if any).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved {
    pub absolute: PathBuf,
    pub display: String,
}

/// How the model-facing result header for a file is produced.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum HeaderKind {
    /// `[<display>]`.
    #[default]
    Path,
    /// `[<display>#TAG]` where TAG hashes the text that actually landed on
    /// disk (hashline).
    HashlineTag,
}

/// One file's fully computed post-edit state, ready to be written.
#[derive(Debug, Clone)]
pub struct StagedFile {
    /// Display path as authored (after suffix / tag recovery).
    pub display: String,
    /// Absolute target path.
    pub absolute: PathBuf,
    pub op: FileOp,
    /// Destination for a rename (`*** Move to:` / hashline `MV`). The write
    /// request becomes a `move`; the result reports `Moved to <display>`.
    pub move_to: Option<Resolved>,
    /// Whether the target existed before the edit.
    pub existed: bool,
    /// Raw pre-edit bytes as text (BOM and line endings intact; notebook
    /// JSON). `None` for creates.
    pub before_raw: Option<String>,
    /// LF-normalized, BOM-stripped pre-edit text (editable notebook text).
    pub before: String,
    /// LF-normalized post-edit text. Equals `before` for `Noop`/`Delete`.
    pub after: String,
    /// Bytes to persist: BOM + original line endings restored; notebook JSON
    /// serialized. `None` for `Delete`/`Noop`.
    pub persisted: Option<String>,
    /// Diff reported in structured details (numbered `generate_diff_string`
    /// format for most modes; plain unified diff for `patch`/`apply_patch`).
    pub diff: String,
    /// Numbered diff the model-facing compact preview is built from when it
    /// differs from `diff` (`patch`/`apply_patch`). `None` uses `diff`.
    pub preview_diff: Option<String>,
    pub first_changed_line: Option<u32>,
    pub header: HeaderKind,
    /// Lines rendered between the header and the preview (hashline block
    /// resolutions).
    pub before_preview: Vec<String>,
    /// Lines rendered after the preview (sloppy recovery notes).
    pub after_preview: Vec<String>,
    /// Rendered under a trailing `Warnings:` block.
    pub warnings: Vec<String>,
    /// Replaces the whole rendered text (hashline no-op diagnostic).
    pub text_override: Option<String>,
    /// Record a fresh snapshot of the written text in the store after the
    /// write lands (hashline) and warn when the persisted text drifted.
    pub record_snapshot: bool,
    /// Clipboard state to publish once this file's write lands (hashline
    /// `CUT`/`PUT` registers).
    pub clipboard_after: Option<crate::store::Clipboard>,
}

impl StagedFile {
    /// Minimal staged update; callers fill in the optional fields.
    pub fn new(display: impl Into<String>, absolute: PathBuf, op: FileOp) -> Self {
        Self {
            display: display.into(),
            absolute,
            op,
            move_to: None,
            existed: true,
            before_raw: None,
            before: String::new(),
            after: String::new(),
            persisted: None,
            diff: String::new(),
            preview_diff: None,
            first_changed_line: None,
            header: HeaderKind::Path,
            before_preview: Vec::new(),
            after_preview: Vec::new(),
            warnings: Vec::new(),
            text_override: None,
            record_snapshot: false,
            clipboard_after: None,
        }
    }
}

/// One file's streamed diff preview.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PreviewFile {
    /// Display path as authored (after recovery when the engine resolved it).
    pub display: String,
    pub diff: Option<String>,
    pub first_changed_line: Option<u32>,
    /// Model-facing error text; mutually exclusive with `diff`.
    pub error: Option<String>,
    pub op: Option<FileOp>,
    pub rename: Option<String>,
}

/// Static projection of a payload (no filesystem access): target paths, the
/// per-file added-lines digest for stream matchers, and delete/move intents.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Inspection {
    /// Every path the payload targets, one entry per section (duplicates kept).
    pub paths: Vec<String>,
    /// `(path, digest)` — the digest is the text the edit introduces into that
    /// file (added lines only, no patch grammar). Same-path sections merge.
    pub entries: Vec<(String, String)>,
    /// Delete / move intents for the ACP permission gate.
    pub file_ops: Vec<FileOpIntent>,
}

/// A destructive file operation a payload declares.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileOpIntent {
    Delete { path: String },
    Move { from: String, to: String },
}

/// A mode engine: pure functions over an argument snapshot.
pub trait ModeEngine: Send + Sync {
    fn mode(&self) -> EditMode;

    /// Per-file diff previews for a (possibly partial) payload. `streaming`
    /// is true while arguments are still arriving; engines trim the trailing
    /// partial section/line and never fail the whole preview on the trailing
    /// section's transient error.
    fn preview(
        &self,
        args: &ArgSnapshot,
        streaming: bool,
        files: &mut dyn FileSource,
        store: &EditStore,
    ) -> Vec<PreviewFile>;

    /// Full parse + locate + apply in memory. All-or-nothing: any failure
    /// stages nothing. Multi-file payloads return one entry per file in
    /// payload order.
    fn stage(
        &self,
        args: &ArgSnapshot,
        files: &mut dyn FileSource,
        store: &EditStore,
    ) -> Result<Vec<StagedFile>, EditError>;

    /// Static projection of the payload (no file reads).
    fn inspect(&self, args: &ArgSnapshot) -> Inspection;
}

#[cfg(test)]
mod edit_policy_tests {
    use super::*;

    /// The pair the two owners disagreed on: `allowModes: [hashline, replace]`, no default.
    ///
    /// The TOOL used `replace` (its own front-loaded default) while the PROMPT took
    /// `allowed.first()` = `hashline`, so a bare call did something other than what the
    /// prompt promised (review #28's surviving half). One resolution now answers both.
    #[test]
    fn an_unset_default_is_replace_when_replace_is_allowed() {
        let policy = EditPolicy::resolve(None, &["hashline".to_string(), "replace".to_string()]);
        assert_eq!(
            policy.default,
            EditMode::Replace,
            "the documented default wins over the configured order: {policy:?}"
        );
        assert_eq!(policy.allowed.len(), 2);
    }

    /// When `replace` is not requestable the first allowed mode is the only honest answer.
    #[test]
    fn an_unset_default_falls_back_to_the_first_allowed_mode() {
        let policy = EditPolicy::resolve(None, &["hashline".to_string()]);
        assert_eq!(policy.default, EditMode::Hashline);
    }

    /// A configured default outside `allow` cannot be enforced, so it does not win.
    #[test]
    fn a_default_outside_the_allowed_set_falls_back() {
        let policy = EditPolicy::resolve(Some("sloppy"), &["hashline".to_string()]);
        assert_eq!(policy.default, EditMode::Hashline);
        assert_eq!(policy.allowed, vec![EditMode::Hashline]);
    }

    /// A configured default inside the set is honoured, in any order.
    #[test]
    fn a_requestable_default_is_honoured() {
        let policy = EditPolicy::resolve(
            Some("hashline"),
            &["replace".to_string(), "hashline".to_string()],
        );
        assert_eq!(policy.default, EditMode::Hashline);
    }

    /// An empty (or wholly unparseable) allow list means every mode, not none.
    #[test]
    fn an_empty_allow_list_means_every_mode() {
        let policy = EditPolicy::resolve(None, &[]);
        assert_eq!(policy.allowed.len(), 5);
        assert_eq!(policy.default, EditMode::Replace);
    }
}
