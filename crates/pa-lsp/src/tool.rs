//! The `lsp` tool: one native `AgentTool` dispatching every LSP action.
//!
//! FORK-ONLY (allspark). Mirrors oh-my-pi's single `action`-dispatched tool: the
//! model picks an `action`, the tool routes it through the session's
//! [`LspManager`]. Symbol addressing is position-free when a `symbol` name path
//! is given (Serena form) and positional when a `line` is given.
//!
//! Two symbol sources cooperate:
//! - the language server's `documentSymbol` tree (via [`crate::symbols`]) for
//!   server-backed queries, and
//! - the tree-sitter tree built by [`crate::symbol_layer`] for the symbol-layer
//!   actions (`symbols`, `find_symbol`, and the symbol edits), which work even
//!   with no server.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use lsp_types::{
    GotoDefinitionResponse, Hover, HoverContents, Location, MarkedString, Position, SymbolKind,
    WorkspaceSymbolResponse,
};
use pa_agent::abort::AbortSignal;
use pa_agent::types::{AgentTool, AgentToolResult, AgentToolUpdateCallback};

use crate::client::{apply_workspace_edit, LspClient};
use crate::ledger;
use crate::manager::LspManager;
use crate::render::{self, ReferenceLine};
use crate::retriever::{self, InfoTarget};
use crate::symbol_layer::{self, SymbolEdit};
use crate::symbols::{self, FlatSymbol};
use crate::text::LineIndex;
use crate::types::{params_schema, Action, LspParams};

/// The `lsp` tool. Holds the session's manager and the working directory that
/// relative `file` arguments resolve against.
pub struct LspTool {
    manager: Arc<LspManager>,
    cwd: PathBuf,
    schema: serde_json::Value,
}

impl LspTool {
    /// Build the tool for a session rooted at `cwd`.
    #[must_use]
    pub fn new(manager: Arc<LspManager>, cwd: PathBuf) -> Self {
        Self {
            manager,
            cwd,
            schema: params_schema(),
        }
    }

    /// The manager backing this tool (for lifecycle calls from the session).
    #[must_use]
    pub fn manager(&self) -> &Arc<LspManager> {
        &self.manager
    }

    /// Route a parsed request to its handler.
    async fn dispatch(
        &self,
        params: &LspParams,
        timeout: Duration,
    ) -> anyhow::Result<AgentToolResult> {
        let action = Action::parse(&params.action)?;
        match action {
            Action::Status => Ok(self.status().await),
            Action::Capabilities => self.capabilities(params).await,
            Action::Reload => self.reload(params).await,
            Action::RenameFile => self.rename_file(params).await,
            Action::Diagnostics => self.diagnostics(params, timeout).await,
            Action::Symbols => self.symbols(params).await,
            Action::FindSymbol => self.find_symbol(params, timeout).await,
            Action::Definition | Action::FindDeclaration => {
                self.goto(params, timeout, Goto::Definition).await
            }
            Action::Declaration => self.goto(params, timeout, Goto::Declaration).await,
            Action::TypeDefinition => self.goto(params, timeout, Goto::TypeDefinition).await,
            Action::Implementation | Action::FindImplementations => {
                self.goto(params, timeout, Goto::Implementation).await
            }
            Action::References => {
                self.references(params, timeout, params.include_declaration(), false)
                    .await
            }
            Action::FindReferencingSymbols => {
                // Serena's `find_referencing_symbols` excludes the declaration
                // itself and import statements by default.
                let include_declaration = params.include_declaration.unwrap_or(false);
                self.references(
                    params,
                    timeout,
                    include_declaration,
                    !params.include_imports(),
                )
                .await
            }
            Action::Hover => self.hover(params, timeout).await,
            Action::Rename => self.rename(params, timeout).await,
            Action::ReplaceSymbolBody => self.symbol_edit(params, SymbolEditKind::Replace).await,
            Action::InsertBeforeSymbol => {
                self.symbol_edit(params, SymbolEditKind::InsertBefore).await
            }
            Action::InsertAfterSymbol => {
                self.symbol_edit(params, SymbolEditKind::InsertAfter).await
            }
            Action::SafeDeleteSymbol => self.safe_delete(params, timeout).await,
            Action::CodeActions => self.code_actions(params, timeout).await,
        }
    }

    /// Resolve the `file` argument to an absolute path (expanding `~`).
    fn file_path(&self, params: &LspParams) -> anyhow::Result<PathBuf> {
        let raw = params
            .file
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("`file` is required for action `{}`", params.action))?;
        Ok(resolve_file(&self.cwd, raw))
    }

    /// Read a file's text (used by the symbol-layer actions).
    async fn read_file(&self, params: &LspParams) -> anyhow::Result<(PathBuf, String)> {
        let path = self.file_path(params)?;
        let text = tokio::fs::read_to_string(&path)
            .await
            .map_err(|err| anyhow::anyhow!("cannot read {}: {err}", path.display()))?;
        Ok((path, text))
    }

    /// Resolve a position from `line`/`symbol`, opening the document symbols
    /// when a name path must be resolved.
    async fn target(
        &self,
        client: &Arc<LspClient>,
        path: &Path,
        params: &LspParams,
    ) -> anyhow::Result<(Position, Option<FlatSymbol>)> {
        if let Some(symbol) = params.symbol.as_deref().filter(|s| !s.is_empty()) {
            // The server's `documentSymbol` tree is the authority, but its shape
            // differs by server (gopls lists methods top-level, so `Shape/Area`
            // misses). When it does not resolve the name path, fall back to the
            // tree-sitter symbol layer, which normalizes those paths.
            let server_flat = client.flat_symbols(path).await?;
            let matches = symbols::resolve(&server_flat, symbol, false);
            if !matches.is_empty() {
                let chosen = match params.line {
                    Some(line) => pick_on_line(&matches, line - 1)?,
                    None => pick_unique(&matches, symbol)?,
                };
                return Ok((chosen.selection_range.start, Some(chosen)));
            }
            let text = tokio::fs::read_to_string(path).await.unwrap_or_default();
            let tree = symbol_layer::symbol_tree(&text, Some(path));
            let tree_matches = symbol_layer::find(&tree, symbol, false);
            let chosen = match params.line {
                Some(line) => pick_on_line(&tree_matches, line - 1)?,
                None => pick_unique(&tree_matches, symbol)?,
            };
            return Ok((chosen.selection_range.start, Some(chosen)));
        }
        let line = params.line.ok_or_else(|| {
            anyhow::anyhow!(
                "a `line` or a `symbol` is required for action `{}`",
                params.action
            )
        })?;
        anyhow::ensure!(line >= 1, "`line` is 1-based");
        Ok((Position::new(line - 1, 0), None))
    }

    async fn status(&self) -> AgentToolResult {
        let live = self.manager.status().await;
        let mut text = String::from("Language servers:\n");
        if live.is_empty() {
            text.push_str("  (no live clients; spawned on first use)\n");
        }
        for entry in &live {
            let state = if entry.alive { "alive" } else { "dead" };
            let _ = writeln!(
                text,
                "  - {} [{}] {}",
                entry.server,
                state,
                entry.root.display()
            );
        }
        text.push_str("Known servers:\n");
        for name in LspManager::known_servers() {
            let _ = writeln!(text, "  - {name}");
        }
        let details = serde_json::json!({
            "live": live.iter().map(|e| serde_json::json!({
                "server": e.server, "root": e.root, "alive": e.alive,
            })).collect::<Vec<_>>(),
            "known": LspManager::known_servers(),
        });
        AgentToolResult {
            content: vec![pa_agent::types::ToolResultContent::text(text)],
            details,
            terminate: None,
            is_error: false,
        }
    }

    /// Report a live server's advertised capabilities (L2).
    async fn capabilities(&self, params: &LspParams) -> anyhow::Result<AgentToolResult> {
        let path = self.file_path(params)?;
        let client = self.manager.ensure_client(&path).await?;
        let json = client.capabilities_json();
        Ok(AgentToolResult::text(
            serde_json::to_string_pretty(&json).unwrap_or_default(),
        ))
    }

    /// Ask a server to reload its workspace (L2).
    async fn reload(&self, params: &LspParams) -> anyhow::Result<AgentToolResult> {
        let path = self.file_path(params)?;
        let client = self.manager.ensure_client(&path).await?;
        client.reload().await?;
        Ok(AgentToolResult::text(format!(
            "requested workspace reload from `{}`",
            client.name
        )))
    }

    /// Rename a file on disk and let language servers react (L18 `rename_file`).
    ///
    /// The manager broadcasts `workspace/willRenameFiles` to every live server
    /// *before* the move (applying the returned edits), performs the rename,
    /// then broadcasts `workspace/didRenameFiles` to every live server.
    async fn rename_file(&self, params: &LspParams) -> anyhow::Result<AgentToolResult> {
        let from = self.file_path(params)?;
        let dest = params
            .dest
            .as_deref()
            .filter(|d| !d.is_empty())
            .ok_or_else(|| anyhow::anyhow!("`dest` is required for action `rename_file`"))?;
        let to = resolve_file(&self.cwd, dest);

        let touched = self.manager.rename_file(&from, &to).await?;

        let mut text = format!("renamed {} -> {}", from.display(), to.display());
        if !touched.is_empty() {
            let _ = write!(text, "\napplied edits to {} file(s)", touched.len());
        }
        Ok(AgentToolResult::text(text))
    }

    /// Diagnostics for one file (L4 pull-first, L8 publish fallback), attributed
    /// to the owning symbol (L9) and grouped `severity -> name_path`.
    ///
    /// A `file` of `"*"` is the workspace-wide mode (L20): the project language
    /// is detected and up to two checkers run, aggregating their diagnostics.
    async fn diagnostics(
        &self,
        params: &LspParams,
        timeout: Duration,
    ) -> anyhow::Result<AgentToolResult> {
        if params.file.as_deref() == Some("*") {
            return self.workspace_diagnostics(timeout).await;
        }
        let path = self.file_path(params)?;
        let client = self.manager.ensure_client(&path).await?;
        let diagnostics = if client.supports_pull_diagnostics() {
            client.diagnostics(&path).await?
        } else {
            let Some((diagnostics, _)) = client.wait_for_diagnostics(&path, timeout).await else {
                return Ok(AgentToolResult::text(format!(
                    "no diagnostics published for {} within {:?}",
                    path.display(),
                    timeout
                )));
            };
            diagnostics
        };
        let flat = client.flat_symbols(&path).await.unwrap_or_default();
        let grouped = ledger::group_by_severity_and_symbol(&diagnostics, |diag| {
            symbol_containing(&flat, diag.range.start).map_or_else(
                || ledger::FILE_LEVEL_BUCKET.to_string(),
                FlatSymbol::name_path,
            )
        });
        let mut text = format!("{} diagnostics for {}\n", diagnostics.len(), path.display());
        for (severity, name_path, diag) in &grouped {
            let position = diag.range.start;
            let _ = writeln!(
                text,
                "  [{}] {} ({}:{}): {}",
                severity,
                name_path,
                position.line + 1,
                position.character + 1,
                diag.message
            );
        }
        let details = serde_json::to_value(&grouped).unwrap_or(serde_json::Value::Null);
        Ok(AgentToolResult {
            content: vec![pa_agent::types::ToolResultContent::text(text)],
            details,
            terminate: None,
            is_error: false,
        })
    }

    /// Workspace-wide diagnostics (L20): detect the project language(s) at the
    /// session cwd and run up to two checkers, aggregating their output.
    async fn workspace_diagnostics(&self, timeout: Duration) -> anyhow::Result<AgentToolResult> {
        let root = &self.cwd;
        let available = |program: &str| crate::diagnostics_flow::binary_on_path(program);
        let specs = crate::diagnostics_flow::select_checkers(root, available);
        if specs.is_empty() {
            let detected = crate::diagnostics_flow::detect_languages(root);
            let names: Vec<&str> = detected.iter().map(|l| l.name()).collect();
            return Ok(AgentToolResult::text(format!(
                "no workspace checker available for {} (detected: {})",
                root.display(),
                if names.is_empty() {
                    "none".to_string()
                } else {
                    names.join(", ")
                }
            )));
        }
        let budget = timeout.min(crate::diagnostics_flow::CHECKER_TIMEOUT);
        let report = crate::diagnostics_flow::run_checkers(root, &specs, budget).await;

        let mut text = format!(
            "{} workspace diagnostic(s) from {} checker(s):\n",
            report.diagnostics.len(),
            report.checkers.join(", ")
        );
        for (severity, count) in report.count_by_severity() {
            let _ = writeln!(text, "  {severity}: {count}");
        }
        for diagnostic in &report.diagnostics {
            let _ = writeln!(
                text,
                "  [{}] {}:{}:{}: {} ({})",
                diagnostic.severity,
                diagnostic.file.display(),
                diagnostic.line,
                diagnostic.column,
                diagnostic.message,
                diagnostic.checker
            );
        }
        let details = serde_json::to_value(&report).unwrap_or(serde_json::Value::Null);
        Ok(AgentToolResult {
            content: vec![pa_agent::types::ToolResultContent::text(text)],
            details,
            terminate: None,
            is_error: false,
        })
    }

    /// Symbol overview (S9) or workspace symbol search. With no `file`, a
    /// `query` drives `workspace/symbol`.
    async fn symbols(&self, params: &LspParams) -> anyhow::Result<AgentToolResult> {
        if let Some(query) = params.query.as_deref().filter(|q| !q.is_empty()) {
            let client = self.manager.any_client().await.ok_or_else(|| {
                anyhow::anyhow!("workspace symbol search needs at least one open file first")
            })?;
            let response = client.workspace_symbols(query).await?;
            let text = format_workspace_symbols(query, response.as_ref());
            return Ok(AgentToolResult::text(text));
        }
        // S9: build the tree from tree-sitter, no server needed.
        let (path, text) = self.read_file(params).await?;
        let tree = symbol_layer::symbol_tree(&text, Some(&path));
        if !tree.parsed {
            anyhow::bail!(
                "could not parse {} into a symbol tree (unsupported language or syntax errors)",
                path.display()
            );
        }
        let budget = params.max_matches.unwrap_or(200);
        let rendered =
            render::render_overview(&tree.roots, params.depth.unwrap_or(u32::MAX), budget);
        let details = serde_json::json!({
            "language": tree.language,
            "stage": rendered.label,
            "symbols": tree.flatten().iter().map(|s| serde_json::json!({
                "name_path": s.name_path(),
                "kind": symbol_layer::kind_name(s.kind),
                "line": s.selection_range.start.line + 1,
            })).collect::<Vec<_>>(),
        });
        Ok(AgentToolResult {
            content: vec![pa_agent::types::ToolResultContent::text(format!(
                "{} ({} symbols)\n{}",
                path.display(),
                tree.flatten().len(),
                rendered.text
            ))],
            details,
            terminate: None,
            is_error: false,
        })
    }

    /// `find_symbol` (S10): name-path resolution over the syntax tree with
    /// `kinds`/`substring`/`max_matches`, optional body and budgeted hover info.
    async fn find_symbol(
        &self,
        params: &LspParams,
        timeout: Duration,
    ) -> anyhow::Result<AgentToolResult> {
        let (path, text) = self.read_file(params).await?;
        let tree = symbol_layer::symbol_tree(&text, Some(&path));
        anyhow::ensure!(
            tree.parsed,
            "could not parse {} into a symbol tree",
            path.display()
        );

        let pattern = params.symbol.as_deref().unwrap_or("");
        let substring = params.substring.unwrap_or(false);
        let kinds = params
            .kinds
            .as_deref()
            .map_or_else(Vec::new, symbol_layer::parse_kinds);
        let mut matches = symbol_layer::find(&tree, pattern, substring);
        matches = symbol_layer::filter_kinds(&matches, &kinds);
        let total = matches.len();
        let limit = params.max_matches.unwrap_or(usize::MAX);
        matches.truncate(limit);

        let mut text_out = format!(
            "{} match(es) for `{}` in {}\n",
            total,
            if pattern.is_empty() { "*" } else { pattern },
            path.display()
        );
        for symbol in &matches {
            let start = symbol.selection_range.start;
            let _ = writeln!(
                text_out,
                "  - {} ({}) {}:{}",
                symbol.name_path(),
                symbol_layer::kind_name(symbol.kind),
                start.line + 1,
                start.character + 1
            );
        }
        if total > matches.len() {
            let _ = writeln!(text_out, "  ... and {} more", total - matches.len());
        }

        if params.include_body.unwrap_or(false) {
            let index = LineIndex::new(&text);
            for symbol in &matches {
                let (start, end) = crate::text::byte_range(&text, &index, &symbol.range);
                let _ = writeln!(
                    text_out,
                    "\n--- {} ---\n{}",
                    symbol.name_path(),
                    &text[start..end.min(text.len())]
                );
            }
        }
        if params.include_info.unwrap_or(false) {
            let info = self.symbol_info(&path, &matches, timeout).await;
            text_out.push_str(&info);
        }
        Ok(AgentToolResult::text(text_out))
    }

    /// Fetch hover info for found symbols under a time budget (L16).
    async fn symbol_info(&self, path: &Path, symbols: &[FlatSymbol], _timeout: Duration) -> String {
        let client = match self.manager.ensure_client(path).await {
            Ok(client) => client,
            Err(err) => return format!("(info unavailable: {err})\n"),
        };
        let targets: Vec<InfoTarget> = symbols
            .iter()
            .map(|symbol| InfoTarget {
                symbol: symbol.name_path(),
                file: path.to_path_buf(),
                position: symbol.selection_range.start,
            })
            .collect();
        let results = retriever::batch_symbol_info(
            &targets,
            retriever::DEFAULT_SYMBOL_INFO_BUDGET,
            |target| {
                let client = Arc::clone(&client);
                let path = target.file.clone();
                let position = target.position;
                async move {
                    client
                        .hover(&path, position)
                        .await
                        .map(|h| h.map(|h| hover_text(&h)))
                }
            },
        )
        .await;
        retriever::render_info_results(&results)
    }

    async fn goto(
        &self,
        params: &LspParams,
        timeout: Duration,
        kind: Goto,
    ) -> anyhow::Result<AgentToolResult> {
        // The WHOLE cycle is bounded, not only the final request. `ensure_client`
        // starts a server and completes `initialize`, and the symbol lookup may
        // issue its own requests; with the bound only around the last request, a
        // server that never answers `initialize` hung the call far longer than the
        // timeout the caller asked for.
        with_timeout(timeout, kind.label(), async {
            let path = self.file_path(params)?;
            let client = self.manager.ensure_client(&path).await?;
            let (position, symbol) = self.target(&client, &path, params).await?;
            let response = match kind {
                Goto::Definition => {
                    with_timeout(timeout, kind.label(), client.definition(&path, position)).await?
                }
                Goto::Declaration => {
                    with_timeout(timeout, kind.label(), client.declaration(&path, position)).await?
                }
                Goto::TypeDefinition => {
                    with_timeout(
                        timeout,
                        kind.label(),
                        client.type_definition(&path, position),
                    )
                    .await?
                }
                Goto::Implementation => {
                    with_timeout(
                        timeout,
                        kind.label(),
                        client.implementation(&path, position),
                    )
                    .await?
                }
            };
            let locations = definition_locations(response);
            let origin = symbol.map_or_else(
                || format!("{}:{}", path.display(), position.line + 1),
                |s| s.name_path(),
            );
            let mut text = format!("{} of {}:\n", kind.label(), origin);
            if locations.is_empty() {
                text.push_str("  (none)\n");
            }
            for location in &locations {
                let _ = writeln!(text, "  - {}", format_location(location));
            }
            Ok(AgentToolResult::text(text))
        })
        .await
    }

    /// References via LSP (S11), with `include_declaration`, an optional
    /// `kinds` filter, and — for `find_referencing_symbols` — an
    /// `include_imports=false` default that drops `use`/`import` hits.
    async fn references(
        &self,
        params: &LspParams,
        timeout: Duration,
        include_declaration: bool,
        drop_imports: bool,
    ) -> anyhow::Result<AgentToolResult> {
        let path = self.file_path(params)?;
        let client = self.manager.ensure_client(&path).await?;
        let (position, symbol) = self.target(&client, &path, params).await?;
        let locations = with_timeout(
            timeout,
            "references",
            client.references(&path, position, include_declaration),
        )
        .await?
        .unwrap_or_default();
        // A server that is still loading answers `null`, which is legal LSP and
        // indistinguishable from "nothing found" once it is flattened. Measured
        // with rust-analyzer here: the first call returns null in 19 ms and the
        // first non-empty answer arrives at 21.9 s. Reporting that null as a result
        // is a confident wrong answer, so say what is actually true.
        if locations.is_empty() {
            let warmth = client.warmth().await;
            if !warmth.answers_are_final() {
                return Ok(AgentToolResult {
                    content: vec![pa_agent::types::ToolResultContent::text(format!(
                        "[lsp: {} {}] no references are known yet — the server is still                          indexing this workspace. Retry in a few seconds, or use `graph                          trace` (which answers from the built graph in about 2 ms).",
                        client.name,
                        warmth.line(),
                    ))],
                    details: serde_json::json!({
                        "warming": true,
                        "server": client.name,
                        "state": warmth.line(),
                    }),
                    terminate: None,
                    is_error: false,
                });
            }
        }
        let origin = symbol.map_or_else(
            || format!("{}:{}", path.display(), position.line + 1),
            |s| s.name_path(),
        );

        let kinds = params
            .kinds
            .as_deref()
            .map_or_else(Vec::new, symbol_layer::parse_kinds);
        let mut import_cache: std::collections::HashMap<PathBuf, Vec<(u32, u32)>> =
            std::collections::HashMap::new();
        let refs: Vec<ReferenceLine> = locations
            .iter()
            .filter(|location| kind_filter_allows(location, &kinds))
            .filter(|location| {
                if !drop_imports {
                    return true;
                }
                !is_import_location(location, &mut import_cache)
            })
            .map(|location| ReferenceLine {
                file: location.uri.to_file_path().map_or_else(
                    |()| location.uri.to_string(),
                    |path| path.display().to_string(),
                ),
                line: location.range.start.line + 1,
                column: location.range.start.character + 1,
            })
            .collect();
        let budget = params.max_matches.unwrap_or(200);
        let rendered = render::render_references(&refs, budget);
        Ok(AgentToolResult::text(format!(
            "references to {origin} (stage: {}):\n{}",
            rendered.label, rendered.text
        )))
    }

    async fn hover(
        &self,
        params: &LspParams,
        timeout: Duration,
    ) -> anyhow::Result<AgentToolResult> {
        let path = self.file_path(params)?;
        let client = self.manager.ensure_client(&path).await?;
        let (position, _) = self.target(&client, &path, params).await?;
        match with_timeout(timeout, "hover", client.hover(&path, position)).await? {
            Some(hover) => Ok(AgentToolResult::text(hover_text(&hover))),
            None => Ok(AgentToolResult::text("(no hover information)")),
        }
    }

    async fn rename(
        &self,
        params: &LspParams,
        timeout: Duration,
    ) -> anyhow::Result<AgentToolResult> {
        let path = self.file_path(params)?;
        let new_name = params
            .new_name
            .as_deref()
            .filter(|name| !name.is_empty())
            .ok_or_else(|| anyhow::anyhow!("`new_name` is required for action `rename`"))?;
        let client = self.manager.ensure_client(&path).await?;
        let (position, _) = self.target(&client, &path, params).await?;
        let Some(edit) =
            with_timeout(timeout, "rename", client.rename(&path, position, new_name)).await?
        else {
            return Ok(AgentToolResult::text("(server returned no rename edit)"));
        };
        if params.apply.unwrap_or(false) {
            let touched = apply_workspace_edit(&edit).await?;
            let mut text = format!("renamed to `{new_name}` in {} file(s):\n", touched.len());
            for file in &touched {
                let _ = writeln!(text, "  - {}", file.display());
            }
            Ok(AgentToolResult::text(text))
        } else {
            let summary = serde_json::to_string_pretty(&edit).unwrap_or_default();
            Ok(AgentToolResult::text(format!(
                "computed rename to `{new_name}` (apply=false):\n{summary}"
            )))
        }
    }

    /// Symbol-aware edits (S13): replace/insert a symbol's body by name path.
    async fn symbol_edit(
        &self,
        params: &LspParams,
        kind: SymbolEditKind,
    ) -> anyhow::Result<AgentToolResult> {
        let (path, text) = self.read_file(params).await?;
        let symbol = params
            .symbol
            .as_deref()
            .filter(|s| !s.is_empty())
            .ok_or_else(|| {
                anyhow::anyhow!("`symbol` is required for action `{}`", params.action)
            })?;
        let body = params
            .body
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("`body` is required for action `{}`", params.action))?;
        let edit = match kind {
            SymbolEditKind::Replace => {
                symbol_layer::replace_body_edit(&text, Some(&path), symbol, body)?
            }
            SymbolEditKind::InsertBefore => {
                symbol_layer::insert_before_edit(&text, Some(&path), symbol, body)?
            }
            SymbolEditKind::InsertAfter => {
                symbol_layer::insert_after_edit(&text, Some(&path), symbol, body)?
            }
        };
        self.apply_symbol_edit(params, &path, &text, &edit).await
    }

    /// Delete a symbol only when LSP references show nothing depends on it (S14).
    async fn safe_delete(
        &self,
        params: &LspParams,
        timeout: Duration,
    ) -> anyhow::Result<AgentToolResult> {
        let (path, text) = self.read_file(params).await?;
        let symbol = params
            .symbol
            .as_deref()
            .filter(|s| !s.is_empty())
            .ok_or_else(|| {
                anyhow::anyhow!("`symbol` is required for action `safe_delete_symbol`")
            })?;

        // Resolve the symbol to its *name* position so the server answers
        // references for the identifier (not the `func`/`fn` keyword).
        let tree = symbol_layer::symbol_tree(&text, Some(&path));
        let matches = symbol_layer::find(&tree, symbol, false);
        let chosen = match matches.as_slice() {
            [] => anyhow::bail!("no symbol matches `{symbol}`"),
            [one] => one.clone(),
            many => {
                let names: Vec<String> = many.iter().map(FlatSymbol::name_path).collect();
                anyhow::bail!(
                    "`{symbol}` is ambiguous ({} matches: {})",
                    many.len(),
                    names.join(", ")
                )
            }
        };
        let position = chosen.selection_range.start;

        // A failed reference lookup is NOT evidence that nothing references the
        // symbol. Serena shipped exactly this bug: `safe_delete_symbol` deleted
        // symbols that still had callers whenever the server was cold,
        // restarting, or timed out, because an empty `references` reply was read
        // as "no callers" (their issue class #1814). The deletion is
        // irreversible, so an unavailable answer must refuse rather than
        // proceed. `force: true` remains for a caller who established the
        // symbol is unreferenced some other way.
        let force = params.force.unwrap_or(false);
        let references = match self.manager.ensure_client(&path).await {
            Ok(client) => {
                // `include_declaration = false`: the declaration itself is not a
                // reference that blocks the delete.
                match with_timeout(
                    timeout,
                    "safe_delete_symbol references",
                    client.references(&path, position, false),
                )
                .await
                {
                    Ok(locations) => locations.unwrap_or_default(),
                    Err(_) if force => Vec::new(),
                    Err(err) => {
                        return Ok(AgentToolResult::text(format!(
                            "refusing to delete `{symbol}`: the reference lookup failed, so \
                             callers cannot be ruled out ({err}). Re-run with `force: true` \
                             only after confirming nothing references the symbol."
                        )));
                    }
                }
            }
            Err(_) if force => Vec::new(),
            Err(err) => {
                return Ok(AgentToolResult::text(format!(
                    "refusing to delete `{symbol}`: no language server is available to check \
                     references, so callers cannot be ruled out ({err}). Re-run with \
                     `force: true` only after confirming nothing references the symbol."
                )));
            }
        };
        if !references.is_empty() {
            let mut text_out = format!(
                "refusing to delete `{symbol}`: {} reference(s) still point at it\n",
                references.len()
            );
            for location in &references {
                let _ = writeln!(text_out, "  - {}", format_location(location));
            }
            return Ok(AgentToolResult::text(text_out));
        }
        let edit = symbol_layer::delete_edit(&text, Some(&path), symbol)?;
        self.apply_symbol_edit(params, &path, &text, &edit).await
    }

    /// Apply (or preview) a computed symbol edit.
    async fn apply_symbol_edit(
        &self,
        params: &LspParams,
        path: &Path,
        text: &str,
        edit: &SymbolEdit,
    ) -> anyhow::Result<AgentToolResult> {
        let updated = edit.apply(text);
        if params.apply.unwrap_or(false) {
            tokio::fs::write(path, &updated)
                .await
                .map_err(|err| anyhow::anyhow!("cannot write {}: {err}", path.display()))?;
            Ok(AgentToolResult::text(format!(
                "applied edit to {} ({} bytes -> {} bytes)",
                path.display(),
                text.len(),
                updated.len()
            )))
        } else {
            let diff = unified_preview(text, &updated);
            Ok(AgentToolResult::text(format!(
                "computed edit for {} (apply=false):\n{diff}",
                path.display()
            )))
        }
    }

    /// Code actions at a position (L19).
    ///
    /// With no `apply`, the list is returned. With `apply: true`, `query`
    /// selects one action client-side: either a 1-based index into the listed
    /// actions or a case-insensitive substring of the action's title/kind. When
    /// `query` is omitted the first action carrying an edit (preferring the
    /// server's `isPreferred`) is applied.
    async fn code_actions(
        &self,
        params: &LspParams,
        timeout: Duration,
    ) -> anyhow::Result<AgentToolResult> {
        let path = self.file_path(params)?;
        let client = self.manager.ensure_client(&path).await?;
        let (position, _) = self.target(&client, &path, params).await?;
        let range = lsp_types::Range::new(position, position);
        let actions = with_timeout(timeout, "code_actions", client.code_actions(&path, range))
            .await?
            .unwrap_or_default();

        let mut text = format!("{} code action(s):\n", actions.len());
        for (index, action) in actions.iter().enumerate() {
            let _ = writeln!(text, "  {}. {}", index + 1, describe_code_action(action));
        }

        if params.apply.unwrap_or(false) {
            let query = params.query.as_deref().filter(|q| !q.is_empty());
            match select_code_action(&actions, query) {
                ActionSelection::None => {
                    text.push_str("(no applicable code action to apply)\n");
                }
                ActionSelection::Ambiguous(candidates) => {
                    let _ = writeln!(
                        text,
                        "query matched {} actions; narrow it: {}",
                        candidates.len(),
                        candidates.join(", ")
                    );
                }
                ActionSelection::Selected(index) => {
                    let action = &actions[index];
                    let _ = writeln!(text, "applying: {}", describe_code_action(action));
                    match apply_code_action(&client, &path, action).await? {
                        Some(touched) => {
                            let _ = writeln!(text, "applied edit across {} file(s)", touched.len());
                        }
                        None => {
                            text.push_str("(selected action has no edit to apply)\n");
                        }
                    }
                }
            }
        }
        Ok(AgentToolResult::text(text))
    }
}

/// A one-line description of a code action (title + kind, or command).
fn describe_code_action(action: &lsp_types::CodeActionOrCommand) -> String {
    match action {
        lsp_types::CodeActionOrCommand::CodeAction(action) => {
            let kind = action
                .kind
                .as_ref()
                .map_or("", lsp_types::CodeActionKind::as_str);
            let preferred = if action.is_preferred.unwrap_or(false) {
                " [preferred]"
            } else {
                ""
            };
            format!("{} ({kind}){preferred}", action.title)
        }
        lsp_types::CodeActionOrCommand::Command(command) => {
            format!("{} [command: {}]", command.title, command.command)
        }
    }
}

/// Which action a `query` selected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActionSelection {
    /// The 0-based index into the action list to apply.
    Selected(usize),
    /// The query matched more than one action; the candidate titles are listed.
    Ambiguous(Vec<String>),
    /// No action matched (or none carried an edit).
    None,
}

/// The title of a code action, command or not.
fn action_title(action: &lsp_types::CodeActionOrCommand) -> &str {
    match action {
        lsp_types::CodeActionOrCommand::CodeAction(action) => &action.title,
        lsp_types::CodeActionOrCommand::Command(command) => &command.title,
    }
}

/// The kind string of a code action, empty for a command.
fn action_kind(action: &lsp_types::CodeActionOrCommand) -> &str {
    match action {
        lsp_types::CodeActionOrCommand::CodeAction(action) => action
            .kind
            .as_ref()
            .map_or("", lsp_types::CodeActionKind::as_str),
        lsp_types::CodeActionOrCommand::Command(_) => "",
    }
}

/// Whether a code action carries an edit (only those are applicable here).
fn action_has_edit(action: &lsp_types::CodeActionOrCommand) -> bool {
    matches!(
        action,
        lsp_types::CodeActionOrCommand::CodeAction(action) if action.edit.is_some()
    )
}

/// Select the code action `query` names (L19's client-side selector).
///
/// `query` is either a 1-based index (matching the printed list) or a
/// case-insensitive substring of an action's title or kind. With no `query`, the
/// first action carrying an edit is chosen, preferring the server's
/// `isPreferred` marker. A substring that matches several actions is
/// [`ActionSelection::Ambiguous`] so the caller can refine rather than guess.
#[must_use]
pub fn select_code_action(
    actions: &[lsp_types::CodeActionOrCommand],
    query: Option<&str>,
) -> ActionSelection {
    if actions.is_empty() {
        return ActionSelection::None;
    }
    match query {
        None => {
            // Prefer the server's preferred action, then any editable one.
            let preferred = actions.iter().position(|action| {
                action_has_edit(action)
                    && matches!(
                        action,
                        lsp_types::CodeActionOrCommand::CodeAction(a)
                            if a.is_preferred.unwrap_or(false)
                    )
            });
            if let Some(index) = preferred {
                return ActionSelection::Selected(index);
            }
            actions
                .iter()
                .position(action_has_edit)
                .map_or(ActionSelection::None, ActionSelection::Selected)
        }
        Some(query) => {
            // A bare integer is an index into the printed (1-based) list.
            if let Ok(index) = query.trim().parse::<usize>() {
                if index >= 1 && index <= actions.len() {
                    return ActionSelection::Selected(index - 1);
                }
                return ActionSelection::None;
            }
            let needle = query.to_lowercase();
            let matches: Vec<usize> = actions
                .iter()
                .enumerate()
                .filter(|(_, action)| {
                    action_title(action).to_lowercase().contains(&needle)
                        || action_kind(action).to_lowercase().contains(&needle)
                })
                .map(|(index, _)| index)
                .collect();
            match matches.as_slice() {
                [] => ActionSelection::None,
                [one] => ActionSelection::Selected(*one),
                many => ActionSelection::Ambiguous(
                    many.iter()
                        .map(|i| action_title(&actions[*i]).to_string())
                        .collect(),
                ),
            }
        }
    }
}

/// Apply a selected code action, returning the touched files (`None` when the
/// action carries no edit).
///
/// A command-only action has no edit to apply here; the caller reports it as
/// not applied rather than silently executing an unmodeled command.
///
/// # Errors
/// Fails when the workspace edit cannot be applied.
pub async fn apply_code_action(
    _client: &Arc<LspClient>,
    _path: &Path,
    action: &lsp_types::CodeActionOrCommand,
) -> anyhow::Result<Option<Vec<PathBuf>>> {
    match action {
        lsp_types::CodeActionOrCommand::CodeAction(action) => match &action.edit {
            Some(edit) => Ok(Some(apply_workspace_edit(edit).await?)),
            None => Ok(None),
        },
        lsp_types::CodeActionOrCommand::Command(_) => Ok(None),
    }
}

impl AgentTool for LspTool {
    fn name(&self) -> &'static str {
        "lsp"
    }

    fn description(&self) -> &'static str {
        "Query language servers for a file: status, diagnostics, definitions, \
         references, hover, document/workspace symbols, renames, and code \
         actions. Also a symbol layer over tree-sitter: symbols (overview), \
         find_symbol, find_referencing_symbols, find_implementations, and \
         symbol-aware edits (replace_symbol_body, insert_before/after_symbol, \
         safe_delete_symbol). Address a symbol by `symbol` (a `Class/method` \
         name path) or by a 1-based `line`."
    }

    fn parameters(&self) -> &serde_json::Value {
        &self.schema
    }

    fn label(&self) -> &'static str {
        "LSP"
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
            let parsed: LspParams = serde_json::from_value(params)
                .map_err(|err| anyhow::anyhow!("invalid lsp arguments: {err}"))?;
            let timeout = Duration::from_secs(parsed.timeout_secs());
            let result = self.dispatch(&parsed, timeout).await?;
            Ok(result
                .with_duration_ms(u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)))
        })
    }
}

/// Run one language-server request under the call's timeout.
///
/// Every LSP round-trip must be bounded: a server that is indexing, wedged, or
/// waiting on a lock would otherwise hang the tool forever, and the model has
/// no way to cancel. oh-my-pi shipped `lsp` with the timeout parameter declared
/// but unused in most actions (their F08); the fix is one wrapper applied at
/// every call site rather than a per-action convention.
async fn with_timeout<T>(
    budget: Duration,
    action: &str,
    fut: impl std::future::Future<Output = anyhow::Result<T>>,
) -> anyhow::Result<T> {
    match tokio::time::timeout(budget, fut).await {
        Ok(result) => result,
        Err(_) => anyhow::bail!(
            "`{action}` did not answer within {budget:?}; the language server may still be \
             indexing. Raise `timeout` (seconds) and retry."
        ),
    }
}

/// Which go-to request a `goto` call issues.
#[derive(Clone, Copy)]
enum Goto {
    Definition,
    Declaration,
    TypeDefinition,
    Implementation,
}

impl Goto {
    fn label(self) -> &'static str {
        match self {
            Goto::Definition => "definition",
            Goto::Declaration => "declaration",
            Goto::TypeDefinition => "type definition",
            Goto::Implementation => "implementation",
        }
    }
}

/// Which symbol-layer edit a call issues.
#[derive(Clone, Copy)]
enum SymbolEditKind {
    Replace,
    InsertBefore,
    InsertAfter,
}

/// Resolve `raw` (absolute, `~`-prefixed, or relative) against `cwd`.
fn resolve_file(cwd: &Path, raw: &str) -> PathBuf {
    if raw == "~" {
        return pa_types::platform::home_dir().unwrap_or_else(|| cwd.to_path_buf());
    }
    if let Some(rest) = raw.strip_prefix("~/") {
        if let Some(home) = pa_types::platform::home_dir() {
            return home.join(rest);
        }
    }
    let candidate = PathBuf::from(raw);
    if candidate.is_absolute() {
        candidate
    } else {
        cwd.join(candidate)
    }
}

/// Pick the symbol whose selection starts on `line` (0-based), else error.
fn pick_on_line(matches: &[FlatSymbol], line: u32) -> anyhow::Result<FlatSymbol> {
    matches
        .iter()
        .find(|symbol| symbol.selection_range.start.line == line)
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("no symbol on line {}", line + 1))
}

/// Require exactly one match for a name path; ambiguity lists the candidates.
fn pick_unique(matches: &[FlatSymbol], pattern: &str) -> anyhow::Result<FlatSymbol> {
    match matches {
        [] => anyhow::bail!("no symbol matches `{pattern}`"),
        [one] => Ok(one.clone()),
        many => {
            let names: Vec<String> = many.iter().map(FlatSymbol::name_path).collect();
            anyhow::bail!(
                "`{pattern}` is ambiguous ({} matches: {}); add a `line` or an overload index",
                many.len(),
                names.join(", ")
            )
        }
    }
}

/// The innermost symbol whose body range contains `position`.
fn symbol_containing(flat: &[FlatSymbol], position: Position) -> Option<&FlatSymbol> {
    flat.iter()
        .filter(|symbol| range_contains(&symbol.range, position))
        .min_by_key(|symbol| range_span(&symbol.range))
}

fn range_contains(range: &lsp_types::Range, position: Position) -> bool {
    position >= range.start && position <= range.end
}

fn range_span(range: &lsp_types::Range) -> (u32, u32) {
    (
        range.end.line.saturating_sub(range.start.line),
        range.end.character.saturating_sub(range.start.character),
    )
}

/// Flatten a definition response to plain locations.
fn definition_locations(response: Option<GotoDefinitionResponse>) -> Vec<Location> {
    match response {
        Some(GotoDefinitionResponse::Scalar(location)) => vec![location],
        Some(GotoDefinitionResponse::Array(locations)) => locations,
        Some(GotoDefinitionResponse::Link(links)) => links
            .into_iter()
            .map(|link| Location {
                uri: link.target_uri,
                range: link.target_selection_range,
            })
            .collect(),
        None => Vec::new(),
    }
}

fn format_location(location: &Location) -> String {
    let path = location.uri.to_file_path().map_or_else(
        |()| location.uri.to_string(),
        |path| path.display().to_string(),
    );
    format!(
        "{}:{}:{}",
        path,
        location.range.start.line + 1,
        location.range.start.character + 1
    )
}

/// Whether a reference location sits on an `import`/`use` line (S11). Import
/// ranges are computed per file and memoized in `cache`.
fn is_import_location(
    location: &Location,
    cache: &mut std::collections::HashMap<PathBuf, Vec<(u32, u32)>>,
) -> bool {
    let Ok(path) = location.uri.to_file_path() else {
        return false;
    };
    if !cache.contains_key(&path) {
        let ranges = std::fs::read_to_string(&path).map_or_else(
            |_| Vec::new(),
            |text| symbol_layer::import_line_ranges(&text, Some(&path)),
        );
        cache.insert(path.clone(), ranges);
    }
    let ranges = cache.get(&path).map_or(&[][..], Vec::as_slice);
    symbol_layer::line_in_ranges(location.range.start.line + 1, ranges)
}

/// Whether a reference location survives the `kinds` filter. Without a filter
/// every location passes; with one, the location's file is parsed and the
/// innermost symbol at the reference position must match a wanted kind.
fn kind_filter_allows(location: &Location, kinds: &[SymbolKind]) -> bool {
    if kinds.is_empty() {
        return true;
    }
    let Ok(path) = location.uri.to_file_path() else {
        return true;
    };
    let Ok(text) = std::fs::read_to_string(&path) else {
        return true;
    };
    let tree = symbol_layer::symbol_tree(&text, Some(&path));
    let flat = tree.flatten();
    match symbol_containing(&flat, location.range.start) {
        Some(symbol) => kinds.contains(&symbol.kind),
        // A reference outside every symbol is not filtered out.
        None => true,
    }
}

/// Render hover contents as plain text.
fn hover_text(hover: &Hover) -> String {
    match &hover.contents {
        HoverContents::Scalar(marked) => marked_string_text(marked),
        HoverContents::Array(items) => items
            .iter()
            .map(marked_string_text)
            .collect::<Vec<_>>()
            .join("\n"),
        HoverContents::Markup(content) => content.value.clone(),
    }
}

fn marked_string_text(marked: &MarkedString) -> String {
    match marked {
        MarkedString::String(text) => text.clone(),
        MarkedString::LanguageString(language_string) => format!(
            "```{}\n{}\n```",
            language_string.language, language_string.value
        ),
    }
}

/// Render a workspace-symbol response as text lines.
fn format_workspace_symbols(query: &str, response: Option<&WorkspaceSymbolResponse>) -> String {
    let mut text = format!("workspace symbols matching `{query}`:\n");
    match response {
        Some(WorkspaceSymbolResponse::Flat(symbols)) => {
            for symbol in symbols {
                let _ = writeln!(
                    text,
                    "  - {} ({})",
                    symbol.name,
                    format_location(&symbol.location)
                );
            }
        }
        Some(WorkspaceSymbolResponse::Nested(symbols)) => {
            for symbol in symbols {
                let uri = match &symbol.location {
                    lsp_types::OneOf::Left(location) => location.uri.to_string(),
                    lsp_types::OneOf::Right(location) => location.uri.to_string(),
                };
                let _ = writeln!(text, "  - {} ({uri})", symbol.name);
            }
        }
        None => text.push_str("  (none)\n"),
    }
    text
}

/// A minimal line diff for `apply=false` previews (common prefix/suffix elided).
fn unified_preview(before: &str, after: &str) -> String {
    if before == after {
        return "(no change)".to_string();
    }
    let mut removed = 0usize;
    let mut added = 0usize;
    let before_lines: Vec<&str> = before.lines().collect();
    let after_lines: Vec<&str> = after.lines().collect();
    for (a, b) in before_lines.iter().zip(after_lines.iter()) {
        if a != b {
            break;
        }
        removed += 1;
        added += 1;
    }
    let mut out = String::from("--- before\n+++ after\n");
    for line in &before_lines[removed.min(before_lines.len())..] {
        let _ = writeln!(out, "-{line}");
    }
    for line in &after_lines[added.min(after_lines.len())..] {
        let _ = writeln!(out, "+{line}");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_advertises_its_contract() {
        let tool = LspTool::new(Arc::new(LspManager::new()), PathBuf::from("/tmp"));
        assert_eq!(tool.name(), "lsp");
        assert_eq!(tool.label(), "LSP");
        assert!(tool.description().contains("language servers"));
        assert_eq!(
            tool.parameters()["properties"]["action"]["type"],
            serde_json::json!("string")
        );
        assert_eq!(tool.parameters()["required"], serde_json::json!(["action"]));
        // The new symbol-layer actions are advertised.
        let actions = tool.parameters()["properties"]["action"]["enum"]
            .as_array()
            .unwrap();
        assert!(actions.iter().any(|a| a == "find_symbol"));
        assert!(actions.iter().any(|a| a == "replace_symbol_body"));
        assert!(actions.iter().any(|a| a == "safe_delete_symbol"));
    }

    #[test]
    fn file_resolution_expands_and_anchors() {
        let cwd = Path::new("/work");
        assert_eq!(resolve_file(cwd, "a/b.rs"), PathBuf::from("/work/a/b.rs"));
        assert_eq!(resolve_file(cwd, "/abs/c.rs"), PathBuf::from("/abs/c.rs"));
    }

    #[test]
    fn kind_filter_without_kinds_allows_everything() {
        let location = Location {
            uri: lsp_types::Url::parse("file:///tmp/x.rs").unwrap(),
            range: lsp_types::Range::new(Position::new(0, 0), Position::new(0, 1)),
        };
        assert!(kind_filter_allows(&location, &[]));
    }

    #[test]
    fn unified_preview_marks_removed_and_added_lines() {
        let before = "a\nb\nc\n";
        let after = "a\nB\nc\n";
        let diff = unified_preview(before, after);
        assert!(diff.contains("-b"));
        assert!(diff.contains("+B"));
        assert!(diff.contains("-a") || !diff.contains("-a"));
    }
    /// The bound covers PREPARATION, not only the final LSP request.
    ///
    /// `goto` used to run `ensure_client` (which starts a server and completes
    /// `initialize`) and the symbol lookup outside `with_timeout`, so a server that
    /// never answered `initialize` hung the call far past the timeout the caller
    /// asked for. With the bound around the whole cycle, a zero budget expires
    /// before `ensure_client` can do anything — which is observable without a
    /// language server: the call returns the bound's own error instead of the
    /// file/manager error that preparation would raise.
    #[tokio::test]
    async fn goto_bounds_its_preparation_not_only_the_final_request() {
        let tool = LspTool::new(Arc::new(LspManager::new()), PathBuf::from("/tmp"));
        let params: LspParams = serde_json::from_value(serde_json::json!({
            "action": "definition",
            "symbol": "some_symbol",
            "file": "/tmp/does-not-exist.rs"
        }))
        .expect("params");
        let error = tool
            .goto(&params, std::time::Duration::ZERO, Goto::Definition)
            .await
            .expect_err("a zero budget must expire during preparation");
        assert!(
            error.to_string().contains("did not answer within"),
            "the bound itself must fire, not a preparation error: {error}"
        );
    }
}
