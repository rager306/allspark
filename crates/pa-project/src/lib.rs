//! Metadata-driven project store: the `meta/` definitions and the canonical
//! append-only event log. FORK-ONLY.
//!
//! This crate holds no session state and no SQLite: the read model is a later
//! layer over this log, so the definitions and the log stay testable on their
//! own.

pub mod event;
pub mod meta;

pub use event::{Actor, Event, EventLog, SCHEMA_VERSION};
pub use meta::{
    CompletionPolicy, EntitySpec, FieldSpec, LinkCheckPolicy, Meta, PoliciesSpec, ProjectionSpec,
    RelationSpec,
};
