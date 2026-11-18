//! The project store: the canonical append-only event log and the derived
//! SQLite read model. FORK-ONLY.
//!
//! The log is canonical and the SQLite read model is a projection of it, so a
//! damaged or dropped model is rebuilt from the log rather than lost.

pub mod event;
pub mod model;
pub mod project;

pub use event::{Actor, Event, EventLog, SCHEMA_VERSION};
pub use model::{READ_MODEL_VERSION, ReadModel};
pub use project::{ProjectionReport, project};
