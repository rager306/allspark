//! Layered system-prompt assembly: static layer files first (cacheable),
//! every session-specific value last. See the parent module docs.

use crate::skills::{format_skills_for_prompt, Skill};

use super::layers;

/// The bundled skill the refinement trigger guidance keys on.
pub const REFINE_SKILL_NAME: &str = "refine";

/// Whether a prompt segment belongs to the cache-stable prefix or the
/// session-specific tail.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SegmentKind {
    /// Cacheable static prefix content (layer files, or a user replacement).
    Static,
    /// Session-specific tail content; must never leak into [`SegmentKind::Static`].
    Dynamic,
}

/// One named slice of the assembled prompt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptSegment {
    /// Stable segment name (`core`, `usage`, `packages`, `environment`, ...).
    pub name: &'static str,
    pub kind: SegmentKind,
    /// Provenance: the layer file, or what generated the segment.
    pub source: &'static str,
    pub text: String,
}

impl PromptSegment {
    fn static_segment(name: &'static str, source: &'static str, text: String) -> Self {
        Self {
            name,
            kind: SegmentKind::Static,
            source,
            text,
        }
    }

    fn dynamic_segment(name: &'static str, source: &'static str, text: String) -> Self {
        Self {
            name,
            kind: SegmentKind::Dynamic,
            source,
            text,
        }
    }
}

/// The assembled prompt plus its per-layer breakdown. `cached_prefix_len` is
/// the byte length of the static prefix: `assembled[..cached_prefix_len]` is
/// byte-identical across every session that shares the same custom-prompt
/// and model-selection inputs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SystemPromptBreakdown {
    pub segments: Vec<PromptSegment>,
    pub assembled: String,
    pub cached_prefix_len: usize,
}

/// Inputs for one session's prompt. Static inputs (`custom_prompt`,
/// `model_prompt_extras`) select the cached prefix; everything else feeds
/// the dynamic tail.
#[derive(Debug, Default)]
pub struct BuildSystemPromptOptions<'a> {
    /// Replaces the layered static prefix with user text. The dynamic tail
    /// still applies.
    pub custom_prompt: Option<String>,
    pub model_prompt_extras: Option<&'a str>,
    /// Whether the resolved model accepts image input, when known.
    pub vision_capable: Option<bool>,
    /// Active tools. Tool schemas carry tool descriptions outside the prompt.
    pub selected_tools: Option<Vec<&'a str>>,
    pub append_system_prompt: Option<String>,
    pub cwd: String,
    /// Conversation log path.
    pub messages_path: Option<String>,
    /// Pre-loaded context files (path, content).
    pub context_files: Vec<(String, String)>,
    pub skills: Vec<Skill>,
    /// Whether to include the subagent surface in this session.
    pub allow_recursion: Option<bool>,
    /// Fixed recursive-agent depth for this session.
    pub rlm_depth: Option<u32>,
    /// Human-readable parent name or id for child communication doctrine.
    pub rlm_parent_agent: Option<&'a str>,
    /// Whether the session runs without a daemon (no scheduler, no
    /// bash-completion wake-ups). The wake-up facts the daemonless line
    /// states must match the embedding's wiring.
    pub daemonless: bool,
    /// Whether `rlm.spawn` can create children in this session (the
    /// daemonless print host or a daemon worker); a daemonless session
    /// without one cannot spawn at all.
    pub can_spawn: bool,
    /// Enabled user-configured generic MCP servers.
    pub generic_mcp_servers: Vec<String>,
}

/// Build the system prompt (assembled text only).
#[must_use]
pub fn build_system_prompt(options: &BuildSystemPromptOptions) -> String {
    system_prompt_breakdown(options).assembled
}

/// Build the system prompt with its per-layer breakdown.
#[must_use]
pub fn system_prompt_breakdown(options: &BuildSystemPromptOptions) -> SystemPromptBreakdown {
    let mut segments: Vec<PromptSegment> = Vec::new();

    // The static prefix: the user's replacement prompt, or the layered files.
    match options.custom_prompt.as_deref() {
        Some(custom) if !custom.is_empty() => {
            segments.push(PromptSegment::static_segment(
                "custom",
                "--system-prompt",
                custom.to_string(),
            ));
        }
        _ => {
            segments.push(PromptSegment::static_segment(
                "core",
                layers::layer_source("core").unwrap_or_default(),
                layers::CORE_LAYER.trim().to_string(),
            ));
            segments.push(PromptSegment::static_segment(
                "usage",
                layers::layer_source("usage").unwrap_or_default(),
                layers::USAGE_LAYER.trim().to_string(),
            ));
            segments.push(PromptSegment::static_segment(
                "opinionated",
                layers::layer_source("opinionated").unwrap_or_default(),
                layers::OPINIONATED_LAYER.trim().to_string(),
            ));
            if let Some(extras) = options.model_prompt_extras {
                segments.push(PromptSegment::static_segment(
                    "model-prompts",
                    layers::layer_source("model-prompts").unwrap_or_default(),
                    extras.to_string(),
                ));
            }
        }
    }
    let cached_prefix_len = segments
        .iter()
        .map(|segment| segment.text.len())
        .sum::<usize>()
        + 2 * segments.len().saturating_sub(1);

    // The dynamic tail, in fixed order: packages -> project context ->
    // skills inventory -> MCP servers -> environment -> session role ->
    // additional guidance -> appended prompt.
    let tools: Vec<&str> = options
        .selected_tools
        .clone()
        .unwrap_or_else(|| vec![pa_types::ai::PYTHON_TOOL_NAME]);
    let has_ipython = tools.contains(&pa_types::ai::PYTHON_TOOL_NAME);
    let has_file_access = has_ipython || tools.contains(&"bash");

    segments.push(PromptSegment::dynamic_segment(
        "packages",
        "kernel bootstrap defaults",
        packages_section(),
    ));

    let context = context_files_section(&options.context_files);
    if !context.is_empty() {
        segments.push(PromptSegment::dynamic_segment(
            "project-context",
            "AGENTS.md discovery",
            context,
        ));
    }

    let visible_skills: Vec<&Skill> = options
        .skills
        .iter()
        .filter(|skill| !skill.disable_model_invocation)
        .collect();
    if has_file_access && !visible_skills.is_empty() {
        let inventory = format_skills_for_prompt(&options.skills).trim().to_string();
        segments.push(PromptSegment::dynamic_segment(
            "skills-inventory",
            "skill discovery",
            inventory,
        ));
    }

    if has_ipython {
        let mcp = format_generic_mcp_guidance(&options.generic_mcp_servers);
        if !mcp.is_empty() {
            segments.push(PromptSegment::dynamic_segment(
                "mcp-servers",
                "generic MCP settings",
                mcp,
            ));
        }
    }

    segments.push(PromptSegment::dynamic_segment(
        "environment",
        "session configuration",
        environment_section(options),
    ));

    // The fork's native tools (lsp, graph) are top-level tools, not REPL
    // programmatic tools; a short section tells the model they exist only
    // when they are on this session's roster.
    let native = native_tools_section(&tools);
    if !native.is_empty() {
        segments.push(PromptSegment::dynamic_segment(
            "native-tools",
            "native fork tools",
            native,
        ));
    }

    let role = session_role_section(options, has_ipython);
    if !role.is_empty() {
        segments.push(PromptSegment::dynamic_segment(
            "session-role",
            "RLM recursion state",
            role,
        ));
    }

    if let Some(extra) = options.append_system_prompt.as_deref() {
        if !extra.is_empty() {
            segments.push(PromptSegment::dynamic_segment(
                "appended-prompt",
                "--append-system-prompt",
                extra.to_string(),
            ));
        }
    }

    let assembled = segments
        .iter()
        .map(|segment| segment.text.as_str())
        .collect::<Vec<_>>()
        .join("\n\n");
    SystemPromptBreakdown {
        segments,
        assembled,
        cached_prefix_len,
    }
}

fn packages_section() -> String {
    use crate::kernel::bootstrap::default_rlm_extra_import_labels;
    let mut lines = vec![format!(
        "Pre-installed Python packages: {}.",
        default_rlm_extra_import_labels().join(", ")
    )];
    lines.push(
        "Install additional packages with `uv pip install <pkg>` (this is a uv-managed venv with no pip module)."
            .to_string(),
    );
    lines.join("\n")
}

fn context_files_section(context_files: &[(String, String)]) -> String {
    if context_files.is_empty() {
        return String::new();
    }
    let mut lines = vec![
        "# Project Context".to_string(),
        String::new(),
        "Project-specific instructions and guidelines:".to_string(),
        String::new(),
    ];
    for (file_path, content) in context_files {
        lines.push(format!("## {file_path}\n\n{content}"));
    }
    lines.join("\n")
}

fn environment_section(options: &BuildSystemPromptOptions) -> String {
    let cwd = options.cwd.replace('\\', "/");
    let messages_path = options
        .messages_path
        .clone()
        .unwrap_or_else(|| "not persisted".to_string())
        .replace('\\', "/");
    let mut lines = vec![
        format!("Current date: {}", today()),
        format!("Working directory: {cwd}"),
        format!("Conversation log: {messages_path}"),
    ];
    match options.vision_capable {
        Some(true) => lines.push(
            "Image input: this model can see images; `attach_image` loads them into context."
                .to_string(),
        ),
        Some(false) => lines.push(
            "Image input: this model cannot see images; `attach_image` errors for it.".to_string(),
        ),
        None => {}
    }
    lines.join("\n")
}

/// The native fork tools (`lsp`, `graph`, `edit`, `jev`) present on this
/// session's roster. Empty when none is active, so the segment never renders
/// for sessions that keep only the REPL surface.
fn native_tools_section(tools: &[&str]) -> String {
    let has_lsp = tools.contains(&"lsp");
    let has_graph = tools.contains(&"graph");
    let has_edit = tools.contains(&"edit");
    let has_jev = tools.contains(&"jev");
    if !has_lsp && !has_graph && !has_edit && !has_jev {
        return String::new();
    }
    // Hybrid of two reference prompts, adapted to this fork's tools: Serena's
    // system/editing instructions (symbol-first exploration, symbol-level vs
    // file-level editing, trust a refactor once it returns) and oh-my-pi's
    // "Specialized Tools" policy (MUST use the specialized tool over the shell
    // equivalent, plus the reference-before-exported-edit rule). Both source
    // prompts gate their blocks on which tools are on the roster; so does this
    // one, because it is a dynamic-tail segment and may name tools freely.
    let mut lines = vec![
        "# Native tools".to_string(),
        String::new(),
        "These native tools are callable directly (top-level tool calls), not through the Python REPL.".to_string(),
        String::new(),
        "**MUST use the specialized tool over its shell or REPL equivalent.** They are structured, \
         semantic, and cheaper than reaching the same answer by hand:"
            .to_string(),
    ];
    if has_edit {
        lines.push(
            "- `edit`: every file modification. `replace` (the default) takes `path` + `edits` \
             (`[{ oldText, newText, replaceAll? }]`) and requires a prior read of the file; \
             `patch`/`apply_patch`/`hashline`/`sloppy` take a raw `payload` string. NEVER issue an \
             individual edit with `sed`/`perl`/`python` through `bash` — this engine matches fuzzily \
             on whitespace, reports a unique-match error you can refine, previews the diff, and \
             detects no-op loops that a silent `sed` would hide."
                .to_string(),
        );
        lines.push(pa_edit::description(pa_edit::EditMode::Hashline).to_string());
    }
    if has_lsp {
        lines.push(
            "- `lsp`: code intelligence from real language servers plus a tree-sitter symbol layer. \
             Actions: `status`, `capabilities`, `diagnostics` (file, glob, or workspace), \
             `definition`/`declaration`/`type_definition`/`implementation`, `references`, `hover`, \
             `symbols` (file overview or workspace search), `find_symbol`, \
             `find_referencing_symbols`, `find_implementations`, `find_declaration`, `rename`, \
             `rename_file`, `code_actions`, `reload`, and symbol-aware edits \
             (`replace_symbol_body`, `insert_before_symbol`/`insert_after_symbol`, \
             `safe_delete_symbol`). Address a symbol by a `Class/method` name path (`symbol`) or by \
             a 1-based `line`. MUST use it for definitions, references, implementations, hover, and \
             renames; NEVER text-search for code intelligence — a text search misses shadowing, \
             re-exports, and cross-file callsites, and a `sed` rename silently drops them. Reach for \
             `code_actions` on imports, quick fixes, and server-known refactors before editing by hand."
                .to_string(),
        );
    }
    if has_graph {
        lines.push(
            "- `graph`: the on-disk code graph for the working directory. Actions: `index`/`reindex` \
             (build or incrementally repair the symbol/call/import graph), `search` (hybrid symbol \
             search — describe behavior in words, not a regex), `trace` (callers/callees from a \
             symbol; `direction` in/out/both, `depth`, `edge_types`), `architecture` (`view`: \
             overview/structure/layers/clusters/cycles/hotspots/entry_points/file_tree), `coverage` \
             (index honesty report), `changes` (git diff → affected symbols), `status`. MUST use it \
             for architectural questions and call-chain questions instead of reading file after \
             file: the graph already holds the relationships."
                .to_string(),
        );
    }
    if has_jev {
        lines.push(
            "- `jev`: typed calibrated decisions — `classify` into a menu, `route` with an escape, \
             `check` a yes/no probability, `ask` an explicit choice/score/noul question. One call \
             replaces a whole model round-trip for a gray-zone judgment; NOT for arithmetic or \
             lookups."
                .to_string(),
        );
    }
    lines.push(String::new());
    lines.push("## Working with these tools".to_string());
    lines.push(String::new());
    let mut workflow: Vec<&str> = Vec::new();
    workflow.push(
        "- Acquire context step by step; NEVER open a guessed file or read a whole file to find one \
         symbol. Get an overview first (`lsp` `symbols`), narrow to the symbol, and read only the \
         body you need. Once you have read a file, do not re-analyse it with the symbol tools — you \
         already have it.",
    );
    if has_lsp {
        workflow.push(
            "- Before changing an exported or widely used symbol, run `lsp` `references` first, then \
             keep the change backward-compatible or update every callsite in the same pass.",
        );
        workflow.push(
            "- A successful `rename`, `rename_file`, or `safe_delete_symbol` is already complete and \
             consistent across declarations, references, overrides, and imports. Trust it: do not \
             re-read the changed files or re-run the build just to confirm the refactor propagated.",
        );
    }
    if has_edit {
        workflow.push(
            "- Choose the edit granularity: replace a whole symbol with `lsp` \
             `replace_symbol_body` when the entire body changes, and use `edit` `replace` when only a \
             few lines inside a larger symbol change. For several small edits in one file, prefer a \
             batch of targeted `replace` calls over re-emitting the whole file.",
        );
    }
    workflow.push(
        "- Independent calls are safe to batch in one turn; only serialize when one call genuinely \
         needs another's result.",
    );
    workflow.push(
        "- `python_repl` remains the right tool for orchestration, data transformation, and \
         computation — not for what one of these tools does better.",
    );
    for line in workflow {
        lines.push(line.to_string());
    }
    lines.join("\n")
}

fn session_role_section(options: &BuildSystemPromptOptions, has_ipython: bool) -> String {
    let depth = options.rlm_depth.unwrap_or(0);
    let mut lines = vec![format!(
        "Recursive agent depth: {depth}{}",
        if depth == 0 { " (root)" } else { " (not root)" }
    )];
    if options.daemonless {
        lines.push(if options.can_spawn {
            "This session has no daemon: heartbeats never fire, and background `bash()` completions do not wake you, so await them before ending your turn. Subagent replies and exit notices still wake you, and the run waits for your subagents before it exits."
                .to_string()
        } else {
            "This session has no daemon: `rlm.spawn` is unavailable, heartbeats never fire, and background `bash()` completions do not wake you, so await them before ending your turn."
                .to_string()
        });
    }
    if !has_ipython {
        lines.push(
            "This session has no Python REPL (`python_repl` tool): the programmatic tools described above are unavailable here."
                .to_string(),
        );
    }
    if options.allow_recursion == Some(false) {
        lines.push("Subagent spawning is disabled in this session.".to_string());
    }
    if depth > 0 {
        lines.push(format!(
            "You are a child agent spawned by {}. Task prompts are labeled `[task from parent]`.",
            options.rlm_parent_agent.unwrap_or("your parent agent")
        ));
        if has_ipython {
            lines.push(
                "When a task calls for an answer, reply explicitly with `await agent_message.send(message, receiver_role=\"parent\")`. Not every message or task needs a reply; continue cleanup after sending and go idle normally.".to_string(),
            );
            lines.push(
                "For long-running work, report brief progress with `await rlm.progress_note('...')` (at most 512 characters, throttled to about one note per 10 seconds); the parent sees notes without needing a reply.".to_string(),
            );
        }
    }
    lines.join("\n")
}

fn today() -> String {
    // UTC date in YYYY-MM-DD form; the prompt is date context only.
    let days = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs() / 86_400);
    // Civil-from-days algorithm (Howard Hinnant).
    let z = days as i64 + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}")
}

fn format_generic_mcp_guidance(servers: &[String]) -> String {
    let mut enabled: Vec<&String> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for server in servers {
        if seen.insert(server) {
            enabled.push(server);
        }
    }
    enabled.sort();
    if enabled.is_empty() {
        return String::new();
    }
    let mut lines = vec![
        "# Generic MCP Connections".to_string(),
        String::new(),
        "Generic MCP connections are accessed through the pre-imported Python `mcp` object in the Python REPL, not as top-level native tool namespaces or installed Python skills.".to_string(),
        format!(
            "Enabled generic MCP servers: {}.",
            enabled
                .iter()
                .map(|server| format!("`{server}`"))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    ];
    for server in &enabled {
        lines.push(format!(
            "For `{server}`, first discover its tools with `await mcp.list_tools(\"{server}\")`, then call one with `await mcp.call_tool(\"{server}\", \"<tool>\", arguments)`."
        ));
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skills::{create_synthetic_source_info, SkillKind, SourceScope};
    use std::path::PathBuf;

    fn skill(name: &str, import: Option<&str>) -> Skill {
        Skill {
            name: name.to_string(),
            description: format!("Skill {name}"),
            file_path: PathBuf::from("/skills/SKILL.md"),
            base_dir: PathBuf::from("/skills"),
            source_info: create_synthetic_source_info("/skills", "user", SourceScope::User, None),
            disable_model_invocation: false,
            kind: if import.is_some() {
                SkillKind::Python
            } else {
                SkillKind::Markdown
            },
            python: import.map(|import_name| crate::skills::SkillPythonMetadata {
                import_name: import_name.to_string(),
                package_path: PathBuf::from("/skills"),
                pyproject_path: PathBuf::from("/skills/pyproject.toml"),
            }),
        }
    }

    fn base_options() -> BuildSystemPromptOptions<'static> {
        BuildSystemPromptOptions {
            cwd: "/w".to_string(),
            messages_path: Some("/log.jsonl".to_string()),
            skills: vec![
                skill("web-search", Some("websearch")),
                skill("refine", Some("refine")),
                skill("agent-message", Some("agent_message")),
            ],
            ..Default::default()
        }
    }

    #[test]
    fn default_prompt_is_layered_with_static_prefix_first() {
        let breakdown = system_prompt_breakdown(&base_options());
        let prompt = &breakdown.assembled;
        assert!(prompt.starts_with("# prime-agent harness"));
        assert_eq!(breakdown.segments[0].kind, SegmentKind::Static);
        assert_eq!(breakdown.segments[0].name, "core");
        let names: Vec<&str> = breakdown
            .segments
            .iter()
            .map(|segment| segment.name)
            .collect();
        assert_eq!(
            names[..4],
            ["core", "usage", "opinionated", "packages"],
            "core/usage/opinionated layers, then the dynamic tail"
        );
        assert_eq!(
            &prompt[..breakdown.cached_prefix_len],
            format!(
                "{}\n\n{}\n\n{}",
                layers::CORE_LAYER.trim(),
                layers::USAGE_LAYER.trim(),
                layers::OPINIONATED_LAYER.trim()
            )
        );
        let tail = &prompt[breakdown.cached_prefix_len..];
        assert!(tail.contains("Working directory: /w"));
        assert!(tail.contains("Conversation log: /log.jsonl"));
        assert!(tail.contains("<available_skills>"));
        assert!(tail.contains("Recursive agent depth: 0 (root)"));
        assert!(tail.contains("Pre-installed Python packages: requests, httpx,"));
    }

    #[test]
    fn dynamic_tail_isolates_the_cached_prefix() {
        let mut other = base_options();
        other.cwd = "/somewhere/else".to_string();
        other.messages_path = None;
        other.rlm_depth = Some(2);
        other.rlm_parent_agent = Some("the lead");
        other.generic_mcp_servers = vec!["slack".to_string()];
        other.context_files = vec![("AGENTS.md".to_string(), "Rule one.".to_string())];
        let left = system_prompt_breakdown(&base_options());
        let right = system_prompt_breakdown(&other);
        assert_eq!(
            left.assembled[..left.cached_prefix_len],
            right.assembled[..right.cached_prefix_len]
        );
        assert_eq!(left.cached_prefix_len, right.cached_prefix_len);
        assert!(right.assembled[right.cached_prefix_len..]
            .contains("Enabled generic MCP servers: `slack`"));
        assert!(right.assembled[right.cached_prefix_len..]
            .contains("You are a child agent spawned by the lead."));
    }

    #[test]
    fn custom_prompt_replaces_layers_keeps_tail() {
        let mut options = base_options();
        options.custom_prompt = Some("Be terse.".to_string());
        options.model_prompt_extras = Some("Tier-2 guidance.");
        options.context_files = vec![("AGENTS.md".to_string(), "Rule one.".to_string())];
        let breakdown = system_prompt_breakdown(&options);
        let prompt = &breakdown.assembled;
        assert!(prompt.starts_with("Be terse."));
        assert_eq!(breakdown.segments[0].name, "custom");
        assert!(prompt.contains("## AGENTS.md\n\nRule one."));
        assert!(prompt.contains("Working directory: /w"));
        assert!(prompt.contains("Current date: "));
        assert!(!prompt.contains("# prime-agent harness"));
        assert!(!prompt.contains("Tier-2 guidance."));
    }

    #[test]
    fn mcp_servers_are_a_tail_segment() {
        let mut options = base_options();
        options.generic_mcp_servers = vec!["t".to_string(), "t".to_string()];
        let breakdown = system_prompt_breakdown(&options);
        let names: Vec<&str> = breakdown
            .segments
            .iter()
            .map(|segment| segment.name)
            .collect();
        assert!(names.contains(&"mcp-servers"));
        assert!(breakdown.assembled.contains("# Generic MCP Connections"));
        assert!(breakdown
            .assembled
            .contains("Enabled generic MCP servers: `t`."));
    }

    #[test]
    fn model_prompt_extras_form_the_last_static_segment() {
        let mut options = base_options();
        options.model_prompt_extras = Some("Tier-2 guidance.");
        let breakdown = system_prompt_breakdown(&options);
        let names: Vec<&str> = breakdown
            .segments
            .iter()
            .map(|segment| segment.name)
            .collect();
        assert_eq!(
            names[..5],
            ["core", "usage", "opinionated", "model-prompts", "packages"],
            "extras are the last static segment, directly before the dynamic tail"
        );
    }

    /// The native-tools section is the fork's hybrid of Serena's symbol-first
    /// instructions and oh-my-pi's "specialized tool over shell equivalent"
    /// policy. Each block must name the tool it belongs to and carry its
    /// directive; the section must never advertise a tool the roster lacks.
    #[test]
    fn native_tools_section_documents_each_active_tool_and_no_other() {
        let all = native_tools_section(&["bash", "edit", "graph", "jev", "lsp"]);
        for expected in [
            "- `edit`:",
            "- `lsp`:",
            "- `graph`:",
            "- `jev`:",
            "MUST use the specialized tool over its shell or REPL equivalent",
            "NEVER issue an individual edit with `sed`",
            "NEVER text-search for code intelligence",
            "Trust it: do not re-read the changed files",
        ] {
            assert!(all.contains(expected), "missing {expected:?} in:\n{all}");
        }

        // A roster without `lsp` must not advertise lsp actions.
        let no_lsp = native_tools_section(&["edit", "graph"]);
        assert!(no_lsp.contains("- `edit`:"));
        assert!(no_lsp.contains("- `graph`:"));
        assert!(!no_lsp.contains("- `lsp`:"), "leaked lsp into:\n{no_lsp}");
        assert!(!no_lsp.contains("- `jev`:"));
        // The lsp-only workflow lines must follow their tool.
        assert!(!no_lsp.contains("`references` first"));
        assert!(!no_lsp.contains("safe_delete_symbol` is already complete"));

        // A roster with none of them renders no section at all.
        assert!(native_tools_section(&["bash", "python_repl"]).is_empty());
    }

    /// Every action named in the native-tools section must exist on the tool
    /// that owns it, so the prompt cannot drift from the real surface.
    #[test]
    fn native_tools_section_names_real_actions() {
        let section = native_tools_section(&["edit", "graph", "jev", "lsp"]);
        // pa-lsp's action enum (crates/pa-lsp/src/types.rs).
        for action in [
            "status",
            "capabilities",
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
            "rename_file",
            "code_actions",
            "reload",
            "replace_symbol_body",
            "insert_before_symbol",
            "insert_after_symbol",
            "safe_delete_symbol",
        ] {
            assert!(
                section.contains(action),
                "lsp action {action:?} undocumented"
            );
        }
        // The graph tool's dispatch table (crates/pa-core/src/tools/graph.rs).
        for action in [
            "index",
            "reindex",
            "search",
            "trace",
            "architecture",
            "coverage",
            "changes",
            "status",
        ] {
            assert!(
                section.contains(action),
                "graph action {action:?} undocumented"
            );
        }
        // The jev tool's dispatch table (crates/pa-core/src/jev/tool.rs).
        for action in ["classify", "route", "check", "ask"] {
            assert!(
                section.contains(action),
                "jev action {action:?} undocumented"
            );
        }
        // The pa-edit engine's modes (crates/pa-edit/src/engine.rs).
        for mode in ["replace", "patch", "apply_patch", "hashline", "sloppy"] {
            assert!(section.contains(mode), "edit mode {mode:?} undocumented");
        }
    }
}
