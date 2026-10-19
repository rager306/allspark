//! The edit engine: five modes over one session, staging, and preview machinery.
//!
//! FORK-ONLY. Ported from oh-my-pi's `pi-edit`; do not push upstream.
//!
//! [`EditMode`] selects the wire format (`replace`, `patch`, `apply_patch`,
//! `hashline`, `sloppy`) and [`description`] returns the prompt text for a mode,
//! so the tool description and the parser cannot drift apart. The engine
//! guarantees a unique match or an error, preserves line endings and BOM,
//! recovers a stale `hashline` snapshot by rebasing it, and detects a no-op
//! edit loop. `pa-core::tools::edit_hybrid` is the tool surface.

// FORK-ONLY: ported into the allspark fork; do not push upstream.
// Ported from oh-my-pi's `pi-edit` (the edit engine: exact-match, AST/hashline,
// apply_patch, sloppy and replace modes plus the streaming session). The
// workspace pedantic gate rides in via `[lints] workspace = true`; these
// crate-wide exceptions are the classes the ~8.3k-LOC port legitimately trips
// (mirroring pa-ast/pa-diff and upstream oh-my-pi's own lint table):
//   - cast_*: the ported line/column/offset arithmetic is usize/u32/i32/isize
//     by construction (JS `number` semantics and tree-sitter's u32 ABI); every
//     cast is intentional and bounded by file/line counts.
//   - must_use_candidate / return_self_not_must_use: not every ported helper
//     needs `#[must_use]`, and the builder-style setters mirror the JS API.
//   - missing_errors_doc / missing_panics_doc: the ported API documents the
//     error strings it returns (models are trained on them) where it matters,
//     not on every `Result`/panic.
//   - too_many_lines / too_many_arguments / items_after_statements: several
//     parsers and the hashline patcher are verbatim long functions, and the
//     upstream layout keeps helper consts near their use.
//   - match_same_arms / similar_names / struct_excessive_bools: mode dispatch
//     tables and option structs are intentionally explicit for readability.
//   - needless_pass_by_value: by-value args mirror the JS signatures.
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss,
    clippy::items_after_statements,
    clippy::match_same_arms,
    clippy::missing_errors_doc,
    clippy::missing_panics_doc,
    clippy::must_use_candidate,
    clippy::needless_pass_by_value,
    clippy::return_self_not_must_use,
    clippy::similar_names,
    clippy::struct_excessive_bools,
    clippy::too_many_arguments,
    clippy::too_many_lines
)]

//! Edit engine: every edit mode's parsing, matching, in-memory application,
//! and diff generation, plus a streaming [`session::Session`] that turns raw
//! tool-call argument deltas into progressive previews and a final atomic
//! apply through a host-owned writer.
//!
//! # Architecture
//! ```text
//! raw arg deltas ──> stream_json::ArgStream ──> ArgSnapshot
//!                                                  │
//!                     ┌────────────────────────────┴───────────────┐
//!                     ▼                                            ▼
//!            ModeEngine::preview (streaming)              ModeEngine::stage
//!                     │                                            │
//!                     ▼                                            ▼
//!            PreviewBatch (to TUI)              StagedFile[] ──> EditWriter (host)
//! ```
//! Engines are pure over [`files::FileSource`] (a read-through cache honoring
//! [`path_policy::PathPolicy`]) and [`store::EditStore`] (snapshots,
//! clipboard, no-op guard). Error strings are byte-identical to the
//! TypeScript implementation they replace; models are trained on them.

pub mod diff_string;
pub mod engine;
pub mod error;
pub mod files;
pub mod fuzzy;
pub mod modes;
pub mod notebook;
pub mod path_policy;
pub mod session;
pub mod store;
pub mod stream_json;
pub mod text;

pub use engine::{
    EditMode, FileOp, FileOpIntent, HeaderKind, Inspection, ModeEngine, PreviewFile, Resolved,
    StagedFile,
};
pub use error::{EditError, EditResult};
pub use path_policy::{PathPolicy, UrlResolution};
pub use session::{
    ApplyOutcome, ApplyRequest, EditWriter, FileOutcome, PreviewBatch, Session, WriteRequest,
    WriteResponse,
};
pub use store::EditStore;
pub use stream_json::{ArgSnapshot, ArgStream, EditEntry};

/// Prompt text for a mode (the tool description).
pub const fn description(mode: EditMode) -> &'static str {
    match mode {
        EditMode::Replace => include_str!("../prompts/replace.md"),
        EditMode::Patch => include_str!("../prompts/patch.md"),
        EditMode::ApplyPatch => include_str!("../prompts/apply_patch.md"),
        EditMode::Hashline => include_str!("../prompts/hashline.md"),
        EditMode::Sloppy => include_str!("../prompts/sloppy.md"),
    }
}

/// Lark grammar for modes that expose a custom (non-JSON) wire format.
pub const fn grammar(mode: EditMode) -> Option<&'static str> {
    match mode {
        EditMode::ApplyPatch => Some(include_str!("../grammars/apply_patch.lark")),
        EditMode::Hashline => Some(include_str!("../grammars/hashline.lark")),
        EditMode::Sloppy => Some(include_str!("../grammars/sloppy.lark")),
        EditMode::Replace | EditMode::Patch => None,
    }
}
