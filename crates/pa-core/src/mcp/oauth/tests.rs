//! The OAuth flow's tests, grouped by the contract each one pins.
//!
//! The tests are the ones that were in `oauth.rs`; only their files moved.
//! [`fixtures`] holds the scripted transport, the login UI, and the server
//! shapes every subject shares.

mod audience;
mod discovery;
mod fixtures;
mod race;
mod refresh;
mod registration;
