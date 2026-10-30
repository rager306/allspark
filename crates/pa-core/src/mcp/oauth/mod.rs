//! The interactive MCP OAuth login flow and its refresh path.
//!
//! One login: discover the server's OAuth metadata, register (or take) a
//! client, run the PKCE flow against a local callback server while racing a
//! manual paste, and hand back endpoint-bound credentials.
//!
//! The flow was one 1 864-line file, 73% of it tests; it is now one module
//! per responsibility:
//!
//! | module | holds |
//! |---|---|
//! | [`ui`] | [`McpOAuthConfig`] and the [`McpLoginUi`] surface a login drives |
//! | [`credentials`] | the persisted-credential builder both paths share, and its clock |
//! | [`login`] | [`mcp_login`]: discovery, registration, callback race, exchange |
//! | [`refresh`] | [`mcp_refresh_token`]: every binding re-checked before one request |
//! | [`tests`] | the tests, grouped by the contract each one pins |
//!
//! The public surface is re-exported here, so `super::oauth::mcp_login` and
//! `crate::mcp::{McpLoginUi, McpOAuthConfig}` keep their paths.

mod credentials;
mod login;
mod refresh;
mod ui;

#[cfg(test)]
mod tests;

pub use login::mcp_login;
pub use refresh::mcp_refresh_token;
pub use ui::{McpLoginUi, McpOAuthConfig};
