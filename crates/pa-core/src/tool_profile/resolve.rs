//! Phase-7 wiring: resolve a [`ToolProfileSettings`] document into the
//! ordered [`ToolsetLayer`] pipeline the session engine applies to its tool
//! roster.
//!
//! The canonical order (S8) is
//! `config -> context -> base modes -> dynamic modes -> single-project ->
//! project`. Here the settings document supplies the context, the mode list,
//! and the project-local delta; the caller supplies the single-project flag
//! (the session's project-pinning decision).

use crate::settings::ToolProfileSettings;

use super::profile::{Context, Mode, ToolsetDelta, ToolsetLayer};

/// The result of resolving a profile: the ordered layers plus the prompt
/// addenda contributed by the active context/modes.
#[derive(Debug, Clone, Default)]
pub struct ResolvedProfile {
    /// The layer pipeline, in application order.
    pub layers: Vec<ToolsetLayer>,
    /// Prompt addenda, context first then modes in order.
    pub prompt_addenda: Vec<String>,
    /// Whether the context pins a single project.
    pub single_project: bool,
    /// Whether tool results should be wrapped structurally (`false` for
    /// clients with a broken unwrapper).
    pub structured_tool_output: bool,
    /// Facade/API names the active context and modes exclude (`mem`,
    /// `mem.onboarding`, ...). The tool roster is the tool-side projection of
    /// the same decision; the Python kernel's API surface is not a tool, so it
    /// travels to the kernel through its env contract (audit §5.3).
    pub excluded_apis: Vec<String>,
}

/// Resolve a [`ToolProfileSettings`] document.
///
/// Unknown context/mode names are skipped (a stale config degrades softly,
/// mirroring Serena's tombstone handling) rather than failing session
/// creation.
#[must_use]
pub fn resolve_profile(
    settings: Option<&ToolProfileSettings>,
    single_project: bool,
) -> ResolvedProfile {
    let Some(settings) = settings else {
        return ResolvedProfile {
            structured_tool_output: true,
            ..ResolvedProfile::default()
        };
    };

    let mut layers = Vec::new();
    let mut prompt_addenda = Vec::new();
    let mut context_single_project = false;
    let mut structured_tool_output = true;
    let mut excluded_apis: Vec<String> = Vec::new();
    let mut push_apis = |apis: &[String]| {
        for api in apis {
            if !excluded_apis.contains(api) {
                excluded_apis.push(api.clone());
            }
        }
    };

    if let Some(name) = settings.context.as_deref() {
        if let Some(context) = Context::preset(name) {
            context_single_project = context.single_project;
            structured_tool_output = context.structured_tool_output;
            if let Some(prompt) = &context.prompt {
                prompt_addenda.push(prompt.clone());
            }
            push_apis(&context.excluded_apis);
            layers.push(ToolsetLayer::Context(context));
        }
    }

    let pinned = single_project || context_single_project;
    for name in &settings.modes {
        if let Some(mode) = Mode::resolve(name) {
            if let Some(prompt) = &mode.prompt_addendum {
                prompt_addenda.push(prompt.clone());
            }
            push_apis(&mode.excluded_apis);
            if pinned {
                layers.push(ToolsetLayer::DynamicMode(mode));
            } else {
                layers.push(ToolsetLayer::BaseMode(mode));
            }
        }
    }

    if pinned {
        layers.push(ToolsetLayer::SingleProject);
    }

    if !settings.excluded_tools.is_empty() || !settings.included_tools.is_empty() {
        layers.push(ToolsetLayer::Project(ToolsetDelta {
            excluded_tools: settings.excluded_tools.clone(),
            included_tools: settings.included_tools.clone(),
            fixed_tools: Vec::new(),
        }));
    }

    ResolvedProfile {
        layers,
        prompt_addenda,
        single_project: pinned,
        structured_tool_output,
        excluded_apis,
    }
}
