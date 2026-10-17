#![forbid(unsafe_code)]
#![warn(missing_docs)]
//! Native Rust LSP support for the allspark fork: symbol-aware navigation,
//! diagnostics, and renames driven over stdio language servers.
//!
//! FORK-ONLY. The crate synthesizes two upstream designs: oh-my-pi's single
//! `action`-dispatched `lsp` tool and Serena's position-free symbol addressing
//! (a `Class/method` name path). It carries no JSON assets and no MCP/Python
//! bridge — the whole surface is Rust on top of `async-lsp`, `lsp-types`, and
//! `pa-ast` (tree-sitter).
//!
//! Layout:
//! - [`types`] — the tool's argument and result shapes.
//! - [`servers`] — the built-in server table and project-root discovery.
//! - [`client`] — one live language-server client (`spawn`/sync/query).
//! - [`manager`] — per-session client lifecycle, one client per `(server, root)`.
//! - [`ledger`] — diagnostics identity/diffing/grouping.
//! - [`diagnostics_flow`] — the edit diagnostics delta (E6), freshness gate
//!   (L13), and workspace checker runner (L20).
//! - [`writethrough`] — the post-write LSP pipeline (L10).
//! - [`symbols`] — name-path matching over `documentSymbol` trees.
//! - [`symbol_layer`] — name-path symbol tree built from tree-sitter (`pa-ast`).
//! - [`symbols_body`] — compact, EOF-terminated symbol bodies.
//! - [`text`] — byte-offset / LSP-position conversion.
//! - [`render`] — progressive shortening of large results.
//! - [`retriever`] — budgeted, file-grouped symbol-info batches.
//! - [`tool`] — the `lsp` [`pa_agent::types::AgentTool`] implementation.

pub mod client;
pub mod diagnostics_flow;
pub mod ledger;
pub mod manager;
pub mod mux;
pub mod render;
pub mod retriever;
pub mod servers;
pub mod symbol_layer;
pub mod symbols;
pub mod symbols_body;
pub mod text;
pub mod tool;
pub mod types;
pub mod writethrough;

pub use client::{
    apply_workspace_edit, await_diagnostics, DeferredDiagnostics, DiagnosticsOutcome,
    DiagnosticsWaiter, InlineWait, LspClient, WaitStep, DEFERRED_DIAGNOSTICS_WAIT,
    INLINE_DIAGNOSTICS_WAIT,
};
pub use diagnostics_flow::{
    diagnostics_delta, filter_orphan_project_diagnostics, has_marker_ancestor,
    is_orphan_project_diagnostic, run_checkers, select_checkers, snapshot, CheckerSpec,
    DiagnosticsAfterEdit, FreshnessGate, WorkspaceDiagnostic, WorkspaceLanguage, WorkspaceReport,
    DIAGNOSTICS_AFTER_EDIT_DEFAULT, MAX_CONCURRENT_CHECKERS, TS_ROOT_MARKERS,
};
pub use ledger::{
    count_by_severity_and_symbol, diagnostics_to_symbol, group_by_file_severity_symbol, new_since,
    severity_code, severity_name, DiagnosticIdentity, DiagnosticsLedger, DiagnosticsSnapshot,
    OwnedDiagnostic, FILE_LEVEL_BUCKET,
};
pub use manager::{
    Clock, InitFailureBackoff, LspManager, ServerStatus, SystemClock, INIT_FAILURE_BACKOFF,
};
pub use mux::{
    Connection, Handler, LspMux, MuxDaemon, MuxError, MuxIdentity, MuxRequest, MuxResponse,
    MuxResult, MAX_FRAME_BYTES,
};
pub use render::{
    render_kind_counts, render_overview, render_reference_counts, render_reference_locations,
    render_references, render_tree, ReferenceLine, Rendered,
};
pub use retriever::{
    batch_symbol_info, group_by_file, InfoResult, InfoTarget, DEFAULT_SYMBOL_INFO_BUDGET,
};
pub use servers::{project_root, server_for_path, ServerConfig, SERVERS};
pub use symbol_layer::{
    delete_edit, filter_kinds, find, insert_after_edit, insert_before_edit, kind_from_name,
    kind_name, parse_kinds, replace_body_edit, symbol_bytes, symbol_tree, SymbolEdit, SymbolNode,
    SymbolTree,
};
pub use symbols::{flatten_symbols, resolve, FlatSymbol, NamePathMatcher};
pub use symbols_body::{split_lines, BodyError, SymbolBody, SymbolBodyFactory};
pub use text::{byte_offset, byte_range, position_at, range_from_bytes, LineIndex};
pub use tool::{apply_code_action, select_code_action, ActionSelection, LspTool};
pub use writethrough::{
    apply_text_edits, plan as plan_write_through, write_through, WriteThroughOptions,
    WriteThroughReport, WriteThroughStep,
};

/// The LSP wire types (`lsp-types`), re-exported so callers can build request
/// shapes (positions, ranges, URIs) without a direct dependency.
pub use lsp_types;
pub use types::{params_schema, Action, LspParams};
