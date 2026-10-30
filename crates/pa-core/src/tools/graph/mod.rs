//! The native `graph` tool: a thin [`pa_agent::types::AgentTool`] over `pa-graph`.
//! FORK-ONLY.
//!
//! Split by responsibility (it was one 6 322-line file):
//!
//! | module | holds |
//! |---|---|
//! | [`reports`] | the response vocabulary — `Impact*`, `EvidencePack`, `Pack*` |
//! | [`build`] | the two composed builders: reverse impact, evidence pack |
//! | [`args`] | argument parsers and the scope/notes helpers they feed |
//! | [`tool`] | the tool: construction, store access, and the 14 actions |
//!
//! The public surface is re-exported here, so `crate::tools::graph::GraphTool` and
//! `project_key_in` keep working unchanged.

pub mod args;
pub mod build;
pub mod reports;
pub mod tool;

#[cfg(test)]
mod tests;

// The surface other crates use. Measured, not guessed: `rg 'tools::graph::X'` over
// `crates/` finds exactly these two call sites (`engine.rs` for both).
pub use build::project_key_in;
pub use tool::GraphTool;

// Also re-exported at the module root so the tests' `super::X` paths keep working
// after the split (they were written when everything lived in one file).
