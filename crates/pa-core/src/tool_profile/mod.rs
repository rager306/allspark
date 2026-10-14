//! Serena-pattern tool-surface profiles (Phase 7 — config profiles /
//! isolation).
//!
//! The module ports the *configuration* half of Serena's tool surface onto
//! prime-agent's [`pa_agent::types::AgentTool`] roster:
//!
//! | Feature | Module |
//! | --- | --- |
//! | S1 contexts | [`profile::Context`] |
//! | S2 modes | [`profile::Mode`] |
//! | S4 tool markers | [`profile::filter_by_markers`], [`pa_agent::types::ToolMarker`] |
//! | S8 toolset layering | [`profile::ToolsetLayer`], [`profile::apply_layers`] |
//! | S5 auto-registry | [`registry::ToolRegistry`] |
//! | S6 progressive disclosure | [`disclosure::Disclosure`] |
//! | S7 anti-hijack prompt | [`prompt_templates::anti_hijack_section`] |
//! | S20 prompt templates | [`prompt_templates::render_template`] |
//! | S17 anti-grep hooks | [`hooks::AntiGrepHook`] |
//! | S18 task executor | [`task_executor::TaskExecutor`] |
//! | S19 session registry | [`session_registry::SessionRegistry`] |
//! | O8 leak recovery | [`leak_recovery`] |
//! | O10 bash minimizer | [`bash_minimizer`] |
//! | S3 facades (registry only) | [`facades`] |
//!
//! **O12 (LSP write-through contracts)** is *not* reimplemented here: the
//! inline/deferred diagnostics split, freshness gate, and ledger dedup already
//! live in `pa-lsp` (Phase 5 — `diagnostics_flow`, `ledger`, `writethrough`).
//! This module treats it as covered and must not duplicate it.
//!
//! All types are `serde`-friendly so a context/mode delta can be loaded from
//! `settings.json`.
#![forbid(unsafe_code)]

pub mod bash_minimizer;
pub mod disclosure;
pub mod facades;
pub mod hooks;
pub mod leak_recovery;
pub mod profile;
pub mod prompt_templates;
pub mod registry;
pub mod resolve;
pub mod session_registry;
pub mod task_executor;

#[cfg(test)]
mod tests;

pub use profile::{
    apply_layers, available_markers, filter_by_markers, Context, Mode, ToolsetDelta, ToolsetLayer,
    CONTEXT_CODING, CONTEXT_DEBUG, CONTEXT_MINIMAL, CONTEXT_QUERY, MODE_BENCHMARK, MODE_EDITING,
    MODE_INTERACTIVE, MODE_NO_MEMORIES, MODE_NO_ONBOARDING, MODE_ONBOARDING, MODE_ONE_SHOT,
    MODE_PLANNING, MODE_QUERY_PROJECTS, SINGLE_PROJECT_EXCLUDED_TOOLS,
};
pub use registry::{RegistryError, ToolFactory, ToolRegistry};
pub use resolve::{resolve_profile, ResolvedProfile};
