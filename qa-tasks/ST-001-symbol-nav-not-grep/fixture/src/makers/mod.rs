//! The constructors, split by position rather than by the type they build: `first::make`
//! and `second::make` have the same signature (`-> impl Run`), so neither the call site nor
//! the signature says which struct a caller gets. Only the body does, and it is one file
//! away from the call.

pub mod first;
pub mod second;
