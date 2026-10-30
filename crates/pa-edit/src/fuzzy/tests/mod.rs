//! The fuzzy module's tests, grouped by the contract each one pins.
//!
//! Each subject is its own module because each pins a different contract:
//! the distance arithmetic, whole-block matching, sequence placement,
//! context-line placement, the two error formatters, and replacement. The
//! split is of the tests only — every test is the one that was there before.

mod block;
mod context;
mod distance;
mod replace;
mod report;
mod sequence;
