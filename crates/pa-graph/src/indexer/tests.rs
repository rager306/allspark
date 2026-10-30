//! The indexer's tests, grouped by subject.
//!
//! Each subject is its own module because each pins a different contract:
//! what the walk admits, what a finished run leaves behind, how much each
//! depth writes, why a call site did not resolve, and what a second pass
//! must reuse, redo, or remove. The split is of the tests only — every test
//! is the one that was there before, and the fixtures they share live in
//! [`fixtures`].

mod closeout;
mod depth;
mod discovery;
mod fixtures;
mod incremental;
mod layout;
mod misses;
mod resolver;
