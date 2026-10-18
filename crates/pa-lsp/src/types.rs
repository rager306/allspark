//! The `lsp` tool's argument and result shapes.
//!
//! FORK-ONLY (allspark). The argument set synthesizes two upstream designs:
//! oh-my-pi's single `action`-dispatched tool (one native tool, not ten) and
//! Serena's position-free symbol addressing (`symbol` as a `Class/method`
//! name path). Both are optional in different combinations:
//!
//! - position form: `file` + `line` (+ optional `symbol` to disambiguate a
//!   line with several occurrences)
//! - name-path form: `file` + `symbol` (the `documentSymbol` tree, or — for the
//!   symbol-layer actions — the tree-sitter tree, resolves the position)

use serde::{Deserialize, Serialize};

/// The `lsp` action selector. Mirrors oh-my-pi's action set (L2) minus the raw
/// `request` escape hatch and the `rename_file` file-operation, and adds
/// Serena's symbol-layer actions (S9-S14).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    /// Report which language servers are known and which have live clients.
    Status,
    /// Report a live server's advertised capabilities (L2).
    Capabilities,
    /// Ask a server to reload its workspace (L2).
    Reload,
    /// Rename files on disk and let servers react (L2 `rename_file`).
    RenameFile,
    /// Diagnostics for one file (pull `textDocument/diagnostic`, publish
    /// fallback); `file:"*"` runs workspace checkers instead.
    Diagnostics,
    /// Go-to-definition at a position or symbol.
    Definition,
    /// Go-to-declaration at a position or symbol (S12).
    Declaration,
    /// Go-to-type-definition at a position or symbol.
    TypeDefinition,
    /// Go-to-implementation at a position or symbol.
    Implementation,
    /// All references to the symbol at a position or name path.
    References,
    /// Hover text at a position or symbol.
    Hover,
    /// The document symbol tree for a file (Serena's `get_symbols_overview`, S9).
    Symbols,
    /// Find symbols by name path across the syntax tree (Serena's `find_symbol`, S10).
    FindSymbol,
    /// References to a symbol, from the syntax tree (S11).
    FindReferencingSymbols,
    /// Implementations of a symbol (S12).
    FindImplementations,
    /// The declaration/definition of a symbol (S12 `find_declaration`).
    FindDeclaration,
    /// Rename the symbol at a position or name path.
    Rename,
    /// Replace a symbol's whole body by name path (Serena's `replace_symbol_body`, S13).
    ReplaceSymbolBody,
    /// Insert content before a symbol (S13).
    InsertBeforeSymbol,
    /// Insert content after a symbol (S13).
    InsertAfterSymbol,
    /// Delete a symbol only when nothing references it (Serena's `safe_delete_symbol`, S14).
    SafeDeleteSymbol,
    /// Code actions available at a range (list by default; `apply:true` +
    /// `query` selects one client-side).
    CodeActions,
}

impl Action {
    /// Actions that never mutate the workspace or server state.
    #[must_use]
    pub fn is_readonly(self) -> bool {
        !matches!(
            self,
            Action::Rename
                | Action::CodeActions
                | Action::ReplaceSymbolBody
                | Action::InsertBeforeSymbol
                | Action::InsertAfterSymbol
                | Action::SafeDeleteSymbol
                | Action::Reload
                | Action::RenameFile
        )
    }

    /// Every action's wire spelling, for error text and schema enums.
    pub const ALL: &'static [&'static str] = &[
        "status",
        "capabilities",
        "reload",
        "rename_file",
        "diagnostics",
        "definition",
        "declaration",
        "type_definition",
        "implementation",
        "references",
        "hover",
        "symbols",
        "find_symbol",
        "find_referencing_symbols",
        "find_implementations",
        "find_declaration",
        "rename",
        "replace_symbol_body",
        "insert_before_symbol",
        "insert_after_symbol",
        "safe_delete_symbol",
        "code_actions",
    ];

    /// Parse the wire spelling; unknown actions are an error the tool surfaces.
    ///
    /// # Errors
    /// Fails with the list of accepted actions when `raw` is not one of them.
    pub fn parse(raw: &str) -> anyhow::Result<Self> {
        Ok(match raw {
            "status" => Action::Status,
            "capabilities" => Action::Capabilities,
            "reload" => Action::Reload,
            "rename_file" => Action::RenameFile,
            "diagnostics" => Action::Diagnostics,
            "definition" => Action::Definition,
            "declaration" => Action::Declaration,
            "type_definition" => Action::TypeDefinition,
            "implementation" => Action::Implementation,
            "references" => Action::References,
            "hover" => Action::Hover,
            "symbols" => Action::Symbols,
            "find_symbol" => Action::FindSymbol,
            "find_referencing_symbols" => Action::FindReferencingSymbols,
            "find_implementations" => Action::FindImplementations,
            "find_declaration" => Action::FindDeclaration,
            "rename" => Action::Rename,
            "replace_symbol_body" => Action::ReplaceSymbolBody,
            "insert_before_symbol" => Action::InsertBeforeSymbol,
            "insert_after_symbol" => Action::InsertAfterSymbol,
            "safe_delete_symbol" => Action::SafeDeleteSymbol,
            "code_actions" => Action::CodeActions,
            other => anyhow::bail!(
                "unknown lsp action `{other}`; expected one of: {}",
                Action::ALL.join(", ")
            ),
        })
    }
}

/// The tool's parsed arguments.
#[derive(Debug, Clone, Deserialize)]
pub struct LspParams {
    /// The action to run.
    pub action: String,
    /// File path (relative to the session cwd, absolute, or `~`-prefixed).
    #[serde(default)]
    pub file: Option<String>,
    /// 1-based line number for position-based actions.
    #[serde(default)]
    pub line: Option<u32>,
    /// Symbol name, or a `Class/method` name path (Serena form).
    #[serde(default)]
    pub symbol: Option<String>,
    /// Workspace symbol query (`action="symbols"` without a file).
    #[serde(default)]
    pub query: Option<String>,
    /// New name for `action="rename"`.
    #[serde(default)]
    pub new_name: Option<String>,
    /// Replacement body for `replace_symbol_body` / content for `insert_*`.
    #[serde(default)]
    pub body: Option<String>,
    /// Apply an edit-producing action instead of returning it (default false).
    #[serde(default)]
    pub apply: Option<bool>,
    /// Per-request timeout in seconds (default 20, clamped to 5..=300).
    #[serde(default)]
    pub timeout: Option<u64>,
    /// Symbol-tree depth for `symbols`/`find_symbol` (`None` = unlimited).
    #[serde(default)]
    pub depth: Option<u32>,
    /// Comma/space-separated symbol kinds filter, e.g. `"method,function"`.
    #[serde(default)]
    pub kinds: Option<String>,
    /// Substring (rather than exact) leaf matching for `find_symbol`.
    #[serde(default)]
    pub substring: Option<bool>,
    /// Cap on the number of `find_symbol` matches returned.
    #[serde(default)]
    pub max_matches: Option<usize>,
    /// Include the symbol body in `find_symbol` output.
    #[serde(default)]
    pub include_body: Option<bool>,
    /// Include hover/signature info in `find_symbol` output (L16 budgeted).
    #[serde(default)]
    pub include_info: Option<bool>,
    /// Include the declaration itself in `references`/`find_referencing_symbols`.
    #[serde(default)]
    pub include_declaration: Option<bool>,
    /// Include import statements in `find_referencing_symbols` (default false).
    #[serde(default)]
    pub include_imports: Option<bool>,
    /// Destination path for `rename_file`.
    #[serde(default)]
    pub dest: Option<String>,
    /// Delete even when references cannot be verified (`safe_delete_symbol`).
    ///
    /// By default an unavailable reference lookup refuses the delete: an empty
    /// or failed answer is not proof that nothing references the symbol.
    #[serde(default)]
    pub force: Option<bool>,
}

/// The JSON schema advertised to the model.
#[must_use]
pub fn params_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "action": {
                "type": "string",
                "enum": Action::ALL,
                "description": "The LSP operation to run."
            },
            "file": {
                "type": "string",
                "description": "File to query, relative to the working directory (or absolute, or ~-prefixed). Required for file-scoped actions. Use \"*\" with action=diagnostics for a workspace-wide run."
            },
            "line": {
                "type": "integer",
                "description": "1-based line number, when addressing a position directly."
            },
            "symbol": {
                "type": "string",
                "description": "Symbol name or a `Class/method` name path. With no `line`, the symbol tree resolves the position."
            },
            "query": {
                "type": "string",
                "description": "Workspace-wide symbol query for action=\"symbols\" with no file, or the code-action selector for action=\"code_actions\" with apply=true (a 1-based index or a title/kind substring)."
            },
            "new_name": {
                "type": "string",
                "description": "The replacement name for action=\"rename\"."
            },
            "body": {
                "type": "string",
                "description": "The replacement text for replace_symbol_body / insert_before_symbol / insert_after_symbol."
            },
            "apply": {
                "type": "boolean",
                "description": "Apply an edit-producing action (rename, symbol edits) instead of only returning the computed edit. Defaults to false."
            },
            "timeout": {
                "type": "integer",
                "description": "Per-request timeout in seconds (default 20, range 5-300)."
            },
            "depth": {
                "type": "integer",
                "description": "Symbol-tree depth for symbols/find_symbol (0 = top level only; omitted = unlimited)."
            },
            "kinds": {
                "type": "string",
                "description": "Comma-separated symbol kinds filter, e.g. \"method,function,class\"."
            },
            "substring": {
                "type": "boolean",
                "description": "Match symbol leaf names by substring instead of exactly (find_symbol)."
            },
            "max_matches": {
                "type": "integer",
                "description": "Cap on find_symbol matches returned."
            },
            "include_body": {
                "type": "boolean",
                "description": "Include each found symbol's body text (find_symbol)."
            },
            "include_info": {
                "type": "boolean",
                "description": "Include hover/signature info for found symbols (find_symbol; time-budgeted)."
            },
            "include_declaration": {
                "type": "boolean",
                "description": "Include the declaration itself in references results."
            },
            "include_imports": {
                "type": "boolean",
                "description": "Include import statements among find_referencing_symbols results (default false)."
            },
            "dest": {
                "type": "string",
                "description": "Destination path for action=\"rename_file\"."
            },
            "force": {
                "type": "boolean",
                "description": "action=\"safe_delete_symbol\": delete even when the reference lookup fails. Off by default, because an empty or unavailable answer is not proof that nothing references the symbol."
            }
        },
        "required": ["action"]
    })
}

/// Default per-request timeout in seconds.
pub const DEFAULT_TIMEOUT_SECS: u64 = 20;
/// Lower clamp for the timeout.
pub const MIN_TIMEOUT_SECS: u64 = 5;
/// Upper clamp for the timeout.
pub const MAX_TIMEOUT_SECS: u64 = 300;

impl LspParams {
    /// The effective timeout, clamped to the documented range.
    #[must_use]
    pub fn timeout_secs(&self) -> u64 {
        self.timeout
            .unwrap_or(DEFAULT_TIMEOUT_SECS)
            .clamp(MIN_TIMEOUT_SECS, MAX_TIMEOUT_SECS)
    }

    /// The effective `include_declaration` flag (defaults to true).
    #[must_use]
    pub fn include_declaration(&self) -> bool {
        self.include_declaration.unwrap_or(true)
    }

    /// The effective `include_imports` flag (defaults to false, Serena parity).
    #[must_use]
    pub fn include_imports(&self) -> bool {
        self.include_imports.unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn action_round_trips_the_wire_spelling() {
        for raw in Action::ALL {
            let action = Action::parse(raw).unwrap();
            let is_write = matches!(
                action,
                Action::Rename
                    | Action::CodeActions
                    | Action::ReplaceSymbolBody
                    | Action::InsertBeforeSymbol
                    | Action::InsertAfterSymbol
                    | Action::SafeDeleteSymbol
                    | Action::Reload
                    | Action::RenameFile
            );
            assert_eq!(action.is_readonly(), !is_write);
        }
        assert!(Action::parse("nope").is_err());
        assert_eq!(Action::ALL.len(), 22);
    }

    #[test]
    fn schema_enum_matches_the_action_list() {
        let schema = params_schema();
        let enum_values = schema["properties"]["action"]["enum"].as_array().unwrap();
        assert_eq!(enum_values.len(), Action::ALL.len());
        for raw in Action::ALL {
            assert!(enum_values.iter().any(|v| v == raw), "missing {raw}");
        }
    }

    #[test]
    fn timeout_defaults_and_clamps() {
        let mut p = LspParams {
            action: "status".into(),
            file: None,
            line: None,
            symbol: None,
            query: None,
            new_name: None,
            body: None,
            apply: None,
            timeout: None,
            depth: None,
            kinds: None,
            substring: None,
            max_matches: None,
            include_body: None,
            include_info: None,
            include_declaration: None,
            include_imports: None,
            dest: None,
            force: None,
        };
        assert_eq!(p.timeout_secs(), DEFAULT_TIMEOUT_SECS);
        p.timeout = Some(1);
        assert_eq!(p.timeout_secs(), MIN_TIMEOUT_SECS);
        p.timeout = Some(9_999);
        assert_eq!(p.timeout_secs(), MAX_TIMEOUT_SECS);
        assert!(p.include_declaration());
        assert!(!p.include_imports());
    }
}
