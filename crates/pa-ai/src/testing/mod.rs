//! FORK-ONLY test infrastructure (not in upstream): a wire-level mock
//! inference server that speaks real HTTP/1.1 + SSE over an ephemeral
//! loopback port, so provider code paths can be exercised end-to-end
//! (request build, transport, SSE decode, failure classification) without a
//! network or a spawned process.
//!
//! Compiled only for this crate's own tests or when the `testing` feature is
//! enabled by a dependent crate's dev-dependencies (`pa-daemon`'s failover
//! drill); never part of a production build.

pub mod mock_server;

pub use mock_server::{MockInferenceServer, MockServer, RecordedRequest};
