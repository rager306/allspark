//! S20: prompt templates with `{{ tool_names[...] }}` substitution, plus S7
//! anti-hijack prompt sections for CLI contexts.
//!
//! Serena's prompt factory exposes `available_tools`, `available_markers`, and
//! `tool_names` (a canonical -> effective name mapping, so `{{ tool_names[
//! "find_symbol"] }}` resolves to `jet_brains_find_symbol` or
//! `lsp.find_symbol` depending on the backend). Templates must prefer the
//! mapping over a hard-coded name.

use std::collections::{BTreeMap, BTreeSet};

use pa_agent::types::ToolMarker;

/// S20: the template variables a prompt can reference.
#[derive(Debug, Clone, Default)]
pub struct PromptTemplateVars {
    /// Names of the tools on the effective surface.
    pub available_tools: BTreeSet<String>,
    /// Markers declared by at least one tool on the surface.
    pub available_markers: BTreeSet<ToolMarker>,
    /// Canonical tool name -> effective tool name.
    pub tool_names: BTreeMap<String, String>,
}

impl PromptTemplateVars {
    /// Build the variables from an effective surface: `canonical` names map to
    /// the effective names actually present (a canonical name missing from
    /// the surface maps to itself, so the template still renders).
    #[must_use]
    pub fn from_surface<'a>(
        effective: impl IntoIterator<Item = &'a str>,
        canonical: &[&str],
    ) -> Self {
        let effective: BTreeSet<String> = effective.into_iter().map(str::to_string).collect();
        let mut tool_names = BTreeMap::new();
        for name in canonical {
            let mapped = effective
                .get(*name)
                .cloned()
                .unwrap_or_else(|| (*name).to_string());
            tool_names.insert((*name).to_string(), mapped);
        }
        for name in &effective {
            tool_names.entry(name.clone()).or_insert_with(|| name.clone());
        }
        Self {
            available_tools: effective,
            available_markers: BTreeSet::new(),
            tool_names,
        }
    }

    /// Attach the markers declared by the surface.
    #[must_use]
    pub fn with_markers(mut self, markers: impl IntoIterator<Item = ToolMarker>) -> Self {
        self.available_markers = markers.into_iter().collect();
        self
    }

    /// Register an explicit canonical -> effective rename (the backend
    /// renamed a tool, e.g. `find_symbol` -> `jet_brains_find_symbol`).
    #[must_use]
    pub fn with_tool_name(
        mut self,
        canonical: impl Into<String>,
        effective: impl Into<String>,
    ) -> Self {
        let effective = effective.into();
        self.available_tools.insert(effective.clone());
        self.tool_names.insert(canonical.into(), effective);
        self
    }

    /// Whether a tool is available.
    #[must_use]
    pub fn has_tool(&self, name: &str) -> bool {
        self.available_tools.contains(name)
    }

    /// Whether a marker is present on the surface.
    #[must_use]
    pub fn has_marker(&self, marker: ToolMarker) -> bool {
        self.available_markers.contains(&marker)
    }

    /// Resolve a canonical name to its effective name.
    #[must_use]
    pub fn tool_name<'a>(&'a self, canonical: &'a str) -> &'a str {
        self.tool_names
            .get(canonical)
            .map_or(canonical, String::as_str)
    }
}

/// Render a `{{ ... }}` template against [`PromptTemplateVars`].
///
/// Supported expressions:
/// - `{{ tool_names["find_symbol"] }}` — canonical -> effective name;
/// - `{{ tool_names['find_symbol'] }}` — same, single quotes;
/// - `{{ available_tools }}` — comma-separated tool list;
/// - `{{ available_markers }}` — comma-separated marker class names.
///
/// Unknown expressions render unchanged, so a partially-known template never
/// produces a silent hole.
#[must_use]
pub fn render_template(template: &str, vars: &PromptTemplateVars) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(start) = rest.find("{{") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let Some(end) = after.find("}}") else {
            out.push_str(&rest[start..]);
            return out;
        };
        let expr = after[..end].trim();
        if let Some(value) = evaluate(expr, vars) {
            out.push_str(&value);
        } else {
            out.push_str("{{");
            out.push_str(&after[..end]);
            out.push_str("}}");
        }
        rest = &after[end + 2..];
    }
    out.push_str(rest);
    out
}

fn evaluate(expr: &str, vars: &PromptTemplateVars) -> Option<String> {
    if expr == "available_tools" {
        return Some(vars.available_tools.iter().cloned().collect::<Vec<_>>().join(", "));
    }
    if expr == "available_markers" {
        return Some(
            vars.available_markers
                .iter()
                .map(|m| m.class_name())
                .collect::<Vec<_>>()
                .join(", "),
        );
    }
    let inner = expr.strip_prefix("tool_names[")?.strip_suffix(']')?.trim();
    let key = inner
        .strip_prefix('"')
        .and_then(|s| s.strip_suffix('"'))
        .or_else(|| inner.strip_prefix('\'').and_then(|s| s.strip_suffix('\'')))?;
    Some(vars.tool_name(key).to_string())
}

/// S7: the anti-hijack prompt section for CLI contexts.
///
/// CLI clients (`Claude Code`, `CodeBuddy`, `Codex`, ...) ship a `Read`/`Grep`
/// habit that hijacks the agent away from symbolic tools. Serena answers with
/// a hard "Read FORBIDDEN" rule, a task -> tool mapping table, and an explicit
/// "disallowed reasoning" list that names the rationalizations to refuse.
#[must_use]
pub fn anti_hijack_section(vars: &PromptTemplateVars) -> String {
    let find_symbol = vars.tool_name("find_symbol");
    let get_symbols_overview = vars.tool_name("get_symbols_overview");
    let replace_symbol_body = vars.tool_name("replace_symbol_body");
    let rename_symbol = vars.tool_name("rename_symbol");
    format!(
        "<context name=\"cli-anti-hijack\">\n\
         For file discovery and code navigation, whole-file Read and Grep are \
         FORBIDDEN. Use the symbolic tools:\n\
         | Task | Tool |\n\
         | --- | --- |\n\
         | File/symbol overview | `{get_symbols_overview}` |\n\
         | Find a symbol | `{find_symbol}` |\n\
         | Replace a symbol body | `{replace_symbol_body}` |\n\
         | Rename a symbol | `{rename_symbol}` |\n\
         Disallowed reasoning — do not act on any of these:\n\
         - \"I already know the path, one Read is faster.\"\n\
         - \"One Read is cheaper than three symbolic calls.\"\n\
         - \"Grep is enough to locate this.\"\n\
         Whole-file reads are permitted only when the symbolic tools cannot \
         answer the question, and then only for the file you are about to \
         edit.\n\
         </context>"
    )
}
