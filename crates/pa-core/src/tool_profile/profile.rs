//! Serena-style tool-surface profiles (Phase 7, config isolation):
//!
//! - **S1 contexts** — named profiles over the tool surface
//!   (`excluded_tools` / `included_optional_tools` / `fixed_tools` /
//!   `excluded_apis` / `tool_description_overrides` / `single_project` /
//!   `structured_tool_output`).
//! - **S2 modes** — the nine Serena modes, resolved by name and applied on
//!   top of a context.
//! - **S4 tool markers** — capability/availability filtering
//!   ([`pa_agent::types::ToolMarker`]).
//! - **S8 toolset layering** — the deterministic overlay order
//!   `config -> context -> base modes -> dynamic modes -> single-project ->
//!   project`.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use pa_agent::types::{AgentTool, ToolMarker};
use serde::{Deserialize, Serialize};

/// Tools a single-project context removes (Serena's
/// `SingleProjectExclusions`): project activation and config introspection are
/// meaningless when the project is pinned at startup.
pub const SINGLE_PROJECT_EXCLUDED_TOOLS: &[&str] = &["activate_project", "get_current_config"];

/// Context preset names shipped with the harness.
pub const CONTEXT_CODING: &str = "coding";
/// Read-only query context (no editing tools).
pub const CONTEXT_QUERY: &str = "query";
/// Minimal surface (bash + edit only).
pub const CONTEXT_MINIMAL: &str = "minimal";
/// Debugging surface (keeps diagnostics/graph, drops edits).
pub const CONTEXT_DEBUG: &str = "debug";

fn default_true() -> bool {
    true
}

/// A context/mode/project delta over the tool surface: names to remove and
/// names to force back in (an inclusion always beats an exclusion).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ToolsetDelta {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub excluded_tools: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub included_tools: Vec<String>,
}

impl ToolsetDelta {
    /// An exclusion-only delta.
    #[must_use]
    pub fn excluding(names: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Self {
            excluded_tools: names.into_iter().map(Into::into).collect(),
            included_tools: Vec::new(),
        }
    }
}

/// S1: a named tool-surface profile.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Context {
    /// Profile name (unique within the preset table).
    pub name: String,
    /// Human-facing one-liner.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Jinja-style prompt body injected as the `<context>` section.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    /// Tool names removed from the surface.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub excluded_tools: Vec<String>,
    /// Optional/beta tool names explicitly enabled (they are excluded by
    /// default).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub included_optional_tools: Vec<String>,
    /// Exact tool surface; when non-empty it replaces filtering entirely and
    /// is mutually exclusive with `excluded_tools`/`included_optional_tools`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fixed_tools: Vec<String>,
    /// Facade/API names removed from the REPL surface.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub excluded_apis: Vec<String>,
    /// Facade/API names forced in (e.g. `ext` for `query-projects`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub included_apis: Vec<String>,
    /// Per-tool description replacements (short descriptions for
    /// tool-count-limited clients).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub tool_description_overrides: BTreeMap<String, String>,
    /// The project is pinned at startup; the toolset is minimized and
    /// `activate_project` is removed.
    #[serde(default)]
    pub single_project: bool,
    /// Whether tool results are wrapped in a structured envelope; `false`
    /// for clients with a broken unwrapper (Claude Code).
    #[serde(default = "default_true")]
    pub structured_tool_output: bool,
}

impl Default for Context {
    fn default() -> Self {
        Self {
            name: String::new(),
            description: None,
            prompt: None,
            excluded_tools: Vec::new(),
            included_optional_tools: Vec::new(),
            fixed_tools: Vec::new(),
            excluded_apis: Vec::new(),
            included_apis: Vec::new(),
            tool_description_overrides: BTreeMap::new(),
            single_project: false,
            structured_tool_output: true,
        }
    }
}

impl Context {
    /// A context that excludes the named tools.
    #[must_use]
    pub fn excluding(name: impl Into<String>, tools: &[&str]) -> Self {
        Self {
            name: name.into(),
            excluded_tools: tools.iter().map(|t| (*t).to_string()).collect(),
            ..Self::default()
        }
    }

    /// Validate the profile.
    ///
    /// # Errors
    ///
    /// `fixed_tools` is mutually exclusive with `excluded_tools` /
    /// `included_optional_tools` (Serena `serena_config.py`).
    pub fn validate(&self) -> Result<(), String> {
        if !self.fixed_tools.is_empty()
            && (!self.excluded_tools.is_empty() || !self.included_optional_tools.is_empty())
        {
            return Err(format!(
                "context `{}`: fixed_tools is incompatible with excluded_tools/included_optional_tools",
                self.name
            ));
        }
        Ok(())
    }

    /// S1: filter a tool list to this context's surface.
    ///
    /// With `fixed_tools` the result is exactly those tools; otherwise the
    /// surface is the input minus `excluded_tools`, minus optional/beta tools
    /// unless they appear in `included_optional_tools`.
    #[must_use]
    pub fn apply(&self, tools: &[Arc<dyn AgentTool>]) -> Vec<Arc<dyn AgentTool>> {
        if !self.fixed_tools.is_empty() {
            let fixed: BTreeSet<&str> = self.fixed_tools.iter().map(String::as_str).collect();
            return tools
                .iter()
                .filter(|tool| fixed.contains(tool.name()))
                .cloned()
                .collect();
        }
        let excluded: BTreeSet<&str> = self.excluded_tools.iter().map(String::as_str).collect();
        let included: BTreeSet<&str> = self
            .included_optional_tools
            .iter()
            .map(String::as_str)
            .collect();
        tools
            .iter()
            .filter(|tool| {
                if excluded.contains(tool.name()) {
                    return false;
                }
                let markers = tool.markers();
                let optional =
                    markers.contains(&ToolMarker::Optional) || markers.contains(&ToolMarker::Beta);
                !optional || included.contains(tool.name())
            })
            .cloned()
            .collect()
    }

    /// The effective description for a tool under this context.
    #[must_use]
    pub fn description_override(&self, tool: &str) -> Option<&str> {
        self.tool_description_overrides
            .get(tool)
            .map(String::as_str)
    }

    /// The delta this context contributes to the layering pipeline.
    #[must_use]
    pub fn delta(&self) -> ToolsetDelta {
        let mut excluded = self.excluded_tools.clone();
        if self.single_project {
            excluded.extend(
                SINGLE_PROJECT_EXCLUDED_TOOLS
                    .iter()
                    .map(|s| (*s).to_string()),
            );
        }
        ToolsetDelta {
            excluded_tools: excluded,
            included_tools: self.included_optional_tools.clone(),
        }
    }

    /// The built-in context presets.
    #[must_use]
    pub fn presets() -> Vec<Context> {
        vec![
            Context {
                name: CONTEXT_CODING.into(),
                description: Some("Full coding surface: read, edit, bash, lsp, graph".into()),
                ..Self::default()
            },
            Context {
                name: CONTEXT_QUERY.into(),
                description: Some("Read-only surface: no editing tools".into()),
                excluded_tools: vec!["edit".into(), "bash".into()],
                ..Self::default()
            },
            Context {
                name: CONTEXT_MINIMAL.into(),
                description: Some("Minimal surface: bash + edit only".into()),
                fixed_tools: vec!["bash".into(), "edit".into()],
                ..Self::default()
            },
            Context {
                name: CONTEXT_DEBUG.into(),
                description: Some("Debugging surface: diagnostics + graph, no edits".into()),
                excluded_tools: vec!["edit".into()],
                included_optional_tools: vec!["graph".into()],
                ..Self::default()
            },
        ]
    }

    /// Resolve a preset by name.
    #[must_use]
    pub fn preset(name: &str) -> Option<Context> {
        Self::presets().into_iter().find(|c| c.name == name)
    }
}

/// S2: a Serena mode — a named context-allowlist plus a prompt addendum.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Mode {
    /// Mode name.
    pub name: String,
    /// Contexts this mode is compatible with; empty means "all".
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed_contexts: Vec<String>,
    /// Prompt section appended when the mode is active.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt_addendum: Option<String>,
    /// Tool names the mode removes (layered on top of the context).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub excluded_tools: Vec<String>,
    /// Facade/API names the mode removes.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub excluded_apis: Vec<String>,
}

/// The nine Serena mode names.
pub const MODE_EDITING: &str = "editing";
/// Analysis-only mode.
pub const MODE_PLANNING: &str = "planning";
/// Autonomous single-shot mode.
pub const MODE_ONE_SHOT: &str = "one-shot";
/// Clarify-with-the-user mode.
pub const MODE_INTERACTIVE: &str = "interactive";
/// First-run memory-collecting mode.
pub const MODE_ONBOARDING: &str = "onboarding";
/// Memories disabled.
pub const MODE_NO_MEMORIES: &str = "no-memories";
/// Onboarding disabled (memories kept).
pub const MODE_NO_ONBOARDING: &str = "no-onboarding";
/// Benchmark mode (no memories/onboarding, autonomous).
pub const MODE_BENCHMARK: &str = "benchmark";
/// External-project query mode.
pub const MODE_QUERY_PROJECTS: &str = "query-projects";

impl Mode {
    /// The editing tool names removed in analysis-only modes.
    const EDITING_TOOLS: &'static [&'static str] = &[
        "edit",
        "replace_lines",
        "insert_at_line",
        "delete_lines",
        "replace_symbol_body",
        "insert_before_symbol",
        "insert_after_symbol",
        "safe_delete_symbol",
        "rename_symbol",
    ];

    /// The memory/onboarding tool names.
    const MEMORY_TOOLS: &'static [&'static str] = &[
        "write_memory",
        "read_memory",
        "list_memories",
        "edit_memory",
        "delete_memory",
    ];

    /// Resolve a mode by name (Serena `mode.yml` presets).
    #[must_use]
    pub fn resolve(name: &str) -> Option<Mode> {
        Self::presets().into_iter().find(|m| m.name == name)
    }

    /// The nine built-in modes.
    #[must_use]
    pub fn presets() -> Vec<Mode> {
        vec![
            Mode {
                name: MODE_EDITING.into(),
                prompt_addendum: Some("Prefer symbolic editing tools over line edits.".into()),
                ..Self::default()
            },
            Mode {
                name: MODE_PLANNING.into(),
                prompt_addendum: Some("Analyse only; do not write code.".into()),
                excluded_tools: Self::EDITING_TOOLS
                    .iter()
                    .map(|s| (*s).to_string())
                    .collect(),
                ..Self::default()
            },
            Mode {
                name: MODE_ONE_SHOT.into(),
                prompt_addendum: Some("Work autonomously; do not ask questions.".into()),
                ..Self::default()
            },
            Mode {
                name: MODE_INTERACTIVE.into(),
                prompt_addendum: Some("Confirm ambiguous choices with the user.".into()),
                ..Self::default()
            },
            Mode {
                name: MODE_ONBOARDING.into(),
                prompt_addendum: Some("First run: gather memories as you explore.".into()),
                excluded_tools: Self::EDITING_TOOLS
                    .iter()
                    .map(|s| (*s).to_string())
                    .collect(),
                ..Self::default()
            },
            Mode {
                name: MODE_NO_MEMORIES.into(),
                prompt_addendum: Some("Memory tools are disabled.".into()),
                excluded_tools: Self::MEMORY_TOOLS
                    .iter()
                    .map(|s| (*s).to_string())
                    .collect(),
                excluded_apis: vec!["mem".into()],
                ..Self::default()
            },
            Mode {
                name: MODE_NO_ONBOARDING.into(),
                prompt_addendum: Some("Onboarding is disabled; memories stay enabled.".into()),
                excluded_tools: vec!["onboarding".into()],
                excluded_apis: vec!["mem.onboarding".into()],
                ..Self::default()
            },
            Mode {
                name: MODE_BENCHMARK.into(),
                prompt_addendum: Some(
                    "Benchmark run: autonomous, no memories, no onboarding.".into(),
                ),
                excluded_tools: Self::MEMORY_TOOLS
                    .iter()
                    .chain(std::iter::once(&"onboarding"))
                    .map(|s| (*s).to_string())
                    .collect(),
                excluded_apis: vec!["mem".into()],
                ..Self::default()
            },
            Mode {
                name: MODE_QUERY_PROJECTS.into(),
                prompt_addendum: Some("Query external projects read-only.".into()),
                ..Self::default()
            },
        ]
    }

    /// Whether this mode is compatible with a context (empty allowlist = all).
    #[must_use]
    pub fn allows_context(&self, context: &str) -> bool {
        self.allowed_contexts.is_empty() || self.allowed_contexts.iter().any(|c| c == context)
    }

    /// Apply the mode's exclusions on top of a context-filtered tool list.
    #[must_use]
    pub fn apply(&self, tools: &[Arc<dyn AgentTool>]) -> Vec<Arc<dyn AgentTool>> {
        if self.excluded_tools.is_empty() {
            return tools.to_vec();
        }
        let excluded: BTreeSet<&str> = self.excluded_tools.iter().map(String::as_str).collect();
        tools
            .iter()
            .filter(|tool| !excluded.contains(tool.name()))
            .cloned()
            .collect()
    }

    /// The delta this mode contributes to the layering pipeline.
    #[must_use]
    pub fn delta(&self) -> ToolsetDelta {
        ToolsetDelta::excluding(self.excluded_tools.clone())
    }
}

/// S8: one overlay step in the deterministic toolset pipeline.
#[derive(Debug, Clone)]
pub enum ToolsetLayer {
    /// User config delta (applied first).
    Config(ToolsetDelta),
    /// A context profile.
    Context(Context),
    /// A mode applied to every context.
    BaseMode(Mode),
    /// A mode applied only within a single-project session.
    DynamicMode(Mode),
    /// Single-project exclusions (`activate_project`, `get_current_config`).
    SingleProject,
    /// Project-local delta (applied last).
    Project(ToolsetDelta),
}

impl ToolsetLayer {
    /// Apply an ordered list of layers to a base tool list (the S8 pipeline:
    /// `config -> context -> base modes -> dynamic modes -> single-project ->
    /// project`). An inclusion beats an exclusion.
    #[must_use]
    pub fn apply_layers(
        base_tools: &[Arc<dyn AgentTool>],
        layers: &[ToolsetLayer],
    ) -> Vec<Arc<dyn AgentTool>> {
        apply_layers(base_tools, layers)
    }

    /// The delta this layer contributes.
    #[must_use]
    pub fn delta(&self) -> ToolsetDelta {
        match self {
            ToolsetLayer::Config(delta) | ToolsetLayer::Project(delta) => delta.clone(),
            ToolsetLayer::Context(context) => context.delta(),
            ToolsetLayer::BaseMode(mode) | ToolsetLayer::DynamicMode(mode) => mode.delta(),
            ToolsetLayer::SingleProject => ToolsetDelta::excluding(
                SINGLE_PROJECT_EXCLUDED_TOOLS
                    .iter()
                    .map(|s| (*s).to_string()),
            ),
        }
    }
}

/// S8: apply an ordered list of layers to a base tool list.
///
/// The canonical order is `config -> context -> base modes -> dynamic modes ->
/// single-project -> project`; an explicit inclusion always beats an
/// exclusion, so a later layer can re-enable a tool an earlier layer removed.
#[must_use]
pub fn apply_layers(
    base_tools: &[Arc<dyn AgentTool>],
    layers: &[ToolsetLayer],
) -> Vec<Arc<dyn AgentTool>> {
    let mut excluded: BTreeSet<String> = BTreeSet::new();
    let mut included: BTreeSet<String> = BTreeSet::new();
    for layer in layers {
        let delta = layer.delta();
        excluded.extend(delta.excluded_tools);
        included.extend(delta.included_tools);
    }
    base_tools
        .iter()
        .filter(|tool| !excluded.contains(tool.name()) || included.contains(tool.name()))
        .cloned()
        .collect()
}

/// S4: filter a tool list by markers.
///
/// `required` markers must all be present; `denied` markers must all be
/// absent.
#[must_use]
pub fn filter_by_markers(
    tools: &[Arc<dyn AgentTool>],
    required: &[ToolMarker],
    denied: &[ToolMarker],
) -> Vec<Arc<dyn AgentTool>> {
    tools
        .iter()
        .filter(|tool| {
            let markers = tool.markers();
            required.iter().all(|m| markers.contains(m))
                && denied.iter().all(|m| !markers.contains(m))
        })
        .cloned()
        .collect()
}

/// S4: the set of markers declared by at least one tool
/// (Serena `available_markers`).
#[must_use]
pub fn available_markers(tools: &[Arc<dyn AgentTool>]) -> BTreeSet<ToolMarker> {
    tools
        .iter()
        .flat_map(|tool| tool.markers().iter().copied())
        .collect()
}
