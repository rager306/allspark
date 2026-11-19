//! The project store: the canonical append-only event log and the derived
//! SQLite read model. FORK-ONLY.
//!
//! The log is canonical and the SQLite read model is a projection of it, so a
//! damaged or dropped model is rebuilt from the log rather than lost.

pub mod event;
pub mod model;
pub mod project;

pub use event::{Actor, Event, EventLog, SCHEMA_VERSION};
pub use model::ReadModel;

// `READ_MODEL_VERSION`, `ProjectionReport` and the `project` function are
// deliberately not re-exported: nothing outside this crate reads them, and a
// `pub use` with no consumer is surface debt this repo asks to delete rather
// than keep. When a consumer needs one, that is a one-line change with a caller
// to justify it.
