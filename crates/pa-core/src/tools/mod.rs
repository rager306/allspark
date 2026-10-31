//! All modules are `pub(crate)` internals of the tools subsystem; the
//! crate facade (crate root) re-exports only the public tool surface.

pub(crate) mod golden_replay;
// The native code-graph tool (fork-only; gated by `graph.enabled`).
pub(crate) mod graph;
pub(crate) mod graph_in_flight;
// Process-wide tool metrics: one registry, filled at the telemetry boundary.
pub(crate) mod ipython;
pub(crate) mod metrics;
#[allow(dead_code)] // util module; every fn exercised by unit + golden tests
pub(crate) mod path_utils;
pub(crate) mod render_utils;
pub(crate) mod repeat_codec;
// The agent-facing `schedule` tool (fork-only; gated by `schedule.enabled`).
pub(crate) mod schedule;
pub(crate) mod tool_definition;
