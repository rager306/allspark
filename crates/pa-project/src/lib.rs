//! Metadata-driven project store: the `meta/` definitions, the canonical
//! append-only event log, and the derived read model. FORK-ONLY.
//!
//! The log is canonical and the SQLite read model is a projection of it, so a
//! damaged or dropped model is rebuilt from the log rather than lost.

pub mod event;
pub mod links;
pub mod meta;
pub mod model;
pub mod project;

pub use event::{Actor, Event, EventLog, SCHEMA_VERSION};
pub use links::{LinkReport, RefResolver, RefStatus, check_links};
pub use meta::{
    CompletionPolicy, EntitySpec, FieldSpec, LinkCheckPolicy, Meta, PoliciesSpec, ProjectionSpec,
    RelationSpec,
};
pub use model::{PendingRef, READ_MODEL_VERSION, ReadModel};
pub use project::{ProjectionReport, project};
