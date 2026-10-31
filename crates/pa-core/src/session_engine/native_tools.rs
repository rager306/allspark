//! Registration of the fork's gated native tools (`lsp`, `graph`, `edit`, `jev`,
//! `schedule`).
//!
//! Extracted from `engine.rs`, where 200 lines of tool wiring sat inside the session
//! constructor: upstream edits that file ~29 times per 100 commits, and every merge
//! resolved the same block by hand. The contract here is one struct in, one struct out
//! — the engine keeps only the two handles it must own (`lsp_manager`, `graph_watch`).
//!
//! FORK-ONLY.

use std::sync::Arc;

use pa_agent::types::AgentTool;

use crate::settings::types::GraphAuto;

/// Everything the native-tool layer reads from settings, resolved by the caller.
///
/// A struct rather than a dozen positional arguments (AGENTS.md: no opaque positional
/// parameters), and one field per setting so a new gate is a visible addition here.
pub(crate) struct NativeToolSettings {
    pub lsp_enabled: bool,
    pub lsp_idle_timeout: Option<std::time::Duration>,
    /// `lsp.servers`: per-server overrides applied to the built-in table.
    pub lsp_servers: Option<serde_json::Map<String, serde_json::Value>>,
    pub graph_enabled: bool,
    pub graph_db_path: Option<std::path::PathBuf>,
    pub graph_auto: GraphAuto,
    pub jev_enabled: crate::settings::JevEnabledSetting,
    pub jev_graph_rerank: crate::settings::JevGraphRerankSetting,
    /// The raw spelling, so a status can show "on" and "auto" as the same gate.
    pub jev_graph_rerank_raw: Option<String>,
    pub edit_hybrid_enabled: bool,
    pub edit_hybrid_default_mode: Option<String>,
    pub edit_hybrid_allow_modes: Vec<String>,
    /// The `schedule` tool's store + identity + wake. `None` means the tool is
    /// absent, for either reason: `[schedule] enabled` is off (its default),
    /// or the embedding wired no cron scheduler (print/RPC) and could not
    /// honour a schedule it accepted. Resolved by the caller, like `jev_judge`
    /// — one field, one decision, no gate the caller cannot see.
    pub schedule_target: Option<crate::tools::schedule::ScheduleTarget>,
}

/// What the engine must keep after registration.
pub(crate) struct NativeTools {
    /// The manager that owns live language-server clients for the session.
    pub lsp_manager: Option<Arc<pa_lsp::LspManager>>,
    /// The live graph watcher (`graph.auto = "watch"`), stopped on teardown.
    pub graph_watch: Option<super::graph_watch::WatchHandle>,
    /// Names actually pushed, in push order (filtered against the final roster by
    /// the caller, so this mirrors the effective surface rather than the intent).
    pub native_tool_names: Vec<String>,
}

/// Push the enabled native tools onto `tools` and report the handles to keep.
///
/// `jev_judge` is resolved by the caller because the same chain serves the tool, the
/// auto-thinking classifier and the graph re-rank — one resolution per session.
pub(crate) fn register_native_tools(
    tools: &mut Vec<Arc<dyn AgentTool>>,
    cwd: &std::path::Path,
    agent_dir: &std::path::Path,
    settings: &NativeToolSettings,
    jev_judge: Option<&Arc<crate::jev::ChainJudge>>,
) -> NativeTools {
    let mut native_tool_names: Vec<String> = Vec::new();
    let mut lsp_manager = None;
    let mut graph_watch = None;
    if settings.lsp_enabled {
        // `lsp.idleTimeoutMs`: a bound means an idle client is evicted instead
        // of living until the session ends. Unset keeps the session-long
        // lifetime every build before this had.
        // `lsp.servers`: per-server overrides (a different `command`, extra `args`,
        // more extensions, other root markers). Resolved once here, so the table is
        // fixed for the session and two files in one project cannot disagree about
        // which binary serves them.
        let mut manager = match settings.lsp_idle_timeout {
            Some(idle) => {
                pa_lsp::LspManager::with_idle_timeout(idle, Arc::new(pa_lsp::SystemClock))
            }
            None => pa_lsp::LspManager::new(),
        };
        if settings.lsp_servers.is_some() {
            manager = manager.with_server_overrides(settings.lsp_servers.as_ref());
        }
        let manager = Arc::new(manager);
        tools.push(Arc::new(pa_lsp::LspTool::new(
            Arc::clone(&manager),
            cwd.to_path_buf(),
        )));
        lsp_manager = Some(manager);
        native_tool_names.push("lsp".to_string());
    }
    if settings.graph_enabled {
        let db_path = settings
            .graph_db_path
            .clone()
            .unwrap_or_else(|| agent_dir.join("graph.db"));
        // FORK-ONLY (graph=auto): the analogue of codebase-memory-mcp's daemon
        // — a background build so the graph is warm before the first query.
        // The tool's own `ensure_indexed` remains as the on-demand fallback;
        // this task makes the fallback unnecessary in the common case. It is
        // fire-and-forget: a slow index must never delay session startup, and
        // the on-disk store makes the result visible to every later query.
        if settings.graph_auto != crate::settings::types::GraphAuto::Off {
            let index_root = cwd.to_path_buf();
            let index_db = db_path.clone();
            let project = std::path::Path::new(&index_root)
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("project")
                .to_string();
            tokio::spawn(async move {
                // The store is opened inside the blocking task: the build is
                // CPU-bound (rayon over files) and the SQLite connection must
                // live where it is used.
                let result = tokio::task::spawn_blocking(move || {
                    let mut store = match pa_graph::GraphStore::open(&index_db) {
                        Ok(store) => store,
                        Err(err) => return Err(format!("graph store: {err}")),
                    };
                    // The session's own build is LEAN. It runs before anyone
                    // asked for a graph, so startup must not pay for the ignored
                    // set (measured here: ~2.3 s of walk against ~300 ms lean)
                    // nor let every symbol appear again in `target/` and in
                    // scratch copies of the tree. A caller who wants the wide
                    // view runs the `index` action, which defaults to it.
                    let opts = pa_graph::IndexOptions::new(&project).with_respect_gitignore(true);
                    store
                        .index_project(&index_root, &opts)
                        .map(|report| {
                            format!(
                                "graph auto-index: {} nodes, {} edges, {} files parsed",
                                report.store_nodes, report.edges, report.parsed
                            )
                        })
                        .map_err(|err| format!("graph index: {err}"))
                })
                .await;
                match result {
                    Ok(Ok(summary)) => tracing::debug!("{summary}"),
                    Ok(Err(err)) => tracing::warn!("{err}"),
                    Err(err) => tracing::warn!("graph auto-index task failed: {err}"),
                }
            });
        }
        // FORK-ONLY (graph.auto = "watch"): the value means what it says. The
        // setting was accepted and silently behaved like `auto` — an edit only
        // reached the graph when the next query happened to find the store
        // stale. The watcher is started here, after the start-up build is
        // queued, so the first batch it sees is the tree as it stands.
        if settings.graph_auto == crate::settings::types::GraphAuto::Watch {
            // The watcher must write to the SAME project key the tool reads.
            // It used to take the bare basename, so on a genuine basename
            // collision the tool queried `service-<digest>` while the watcher
            // repaired plain `service` — two graphs, one stale (the
            // lightweight-graph review §3.2). One identity rule: the same
            // store-backed resolution the tool uses, basename as the fallback.
            let watch_key = pa_graph::GraphStore::open(&db_path)
                .ok()
                .and_then(|store| crate::tools::graph::project_key_in(&store, cwd).ok())
                .unwrap_or_else(|| super::graph_watch::project_name(cwd));
            if let Some(handle) = super::graph_watch::start(cwd, &db_path, &watch_key) {
                graph_watch = Some(handle);
            }
        }
        // FORK-ONLY (jev Phase 2): the `search` re-rank, installed only when
        // `[jev] graphRerank = auto` resolves behind a native judge.
        let reranker = crate::jev::graph_rerank::should_run_graph_rerank(
            settings.jev_enabled,
            settings.jev_graph_rerank,
            jev_judge.map(std::convert::AsRef::as_ref),
        )
        .then(|| {
            jev_judge.map(|chain| {
                Arc::new(crate::jev::GraphReranker::new(
                    Arc::clone(chain) as Arc<dyn crate::jev::Judge>
                ))
            })
        })
        .flatten();
        // The raw spelling rides to `status`: "on" and "auto" resolve to the
        // same gate, and the tools report's trap was exactly that equivalence
        // being invisible together with the read-once-at-construction rule.
        let rerank_armed = reranker.is_some();
        tools.push(Arc::new(
            crate::tools::graph::GraphTool::new(db_path, cwd.to_path_buf())
                .with_reranker(reranker)
                .with_rerank_gate(settings.jev_graph_rerank_raw.clone(), rerank_armed),
        ));
        native_tool_names.push("graph".to_string());
    }
    if settings.edit_hybrid_enabled {
        // The store is in-memory and `Clone`; the tool holds one shared
        // instance so seen-line / no-op guards live for the whole session.
        let store = Arc::new(std::sync::Mutex::new(pa_edit::EditStore::new()));
        let default_mode = settings
            .edit_hybrid_default_mode
            .as_deref()
            .and_then(pa_edit::EditMode::parse)
            .unwrap_or(pa_edit::EditMode::Replace);
        let allow_modes: Vec<pa_edit::EditMode> = if settings.edit_hybrid_allow_modes.is_empty() {
            vec![
                pa_edit::EditMode::Replace,
                pa_edit::EditMode::Patch,
                pa_edit::EditMode::ApplyPatch,
                pa_edit::EditMode::Hashline,
                pa_edit::EditMode::Sloppy,
            ]
        } else {
            settings
                .edit_hybrid_allow_modes
                .iter()
                .filter_map(|mode| pa_edit::EditMode::parse(mode))
                .collect()
        };
        // The default mode must be requestable; otherwise fall back to the
        // first allowed mode so a misconfigured pair never drops the tool.
        let default_mode = if allow_modes.contains(&default_mode) {
            default_mode
        } else {
            allow_modes
                .first()
                .copied()
                .unwrap_or(pa_edit::EditMode::Replace)
        };
        tools.push(Arc::new(pa_edit::tool::EditHybridTool::new(
            store,
            lsp_manager.clone(),
            cwd.to_path_buf(),
            default_mode,
            allow_modes,
        )));
        native_tool_names.push("edit".to_string());
    }
    // FORK-ONLY (fork: jev): the typed-decision tool. `on` installs whenever
    // a chain resolved; `auto` only when the chain starts native - no
    // prompted stand-in, ever.
    if crate::jev::should_install_jev_tool(
        settings.jev_enabled,
        jev_judge.map(std::convert::AsRef::as_ref),
    ) {
        if let Some(chain) = jev_judge {
            tools.push(Arc::new(crate::jev::JevTool::new(
                Arc::clone(chain) as Arc<dyn crate::jev::Judge>
            )));
            native_tool_names.push("jev".to_string());
        }
    }
    // FORK-ONLY (fork: schedule): the agent's durable scheduling surface. The
    // caller supplies a target only when the tool can be honoured (gate on AND
    // a scheduler wired); a tool without a store would accept a schedule it
    // cannot fire, which is worse than its absence -- the model would report
    // durable work that never happens.
    if let Some(target) = settings.schedule_target.clone() {
        tools.push(Arc::new(crate::tools::schedule::ScheduleTool::new(target)));
        native_tool_names.push("schedule".to_string());
    }
    NativeTools {
        lsp_manager,
        graph_watch,
        native_tool_names,
    }
}
