//! The interactive surface a login drives, and the per-server setup it receives.

use std::future::Future;
use std::pin::Pin;

use anyhow::Result;

/// One MCP server's OAuth setup: builtin catalog entries and `--oauth`
/// user servers both reduce to this.
#[derive(Debug, Clone, PartialEq)]
pub struct McpOAuthConfig {
    /// MCP server name; the credential lands under `mcp:<server>`.
    pub server: String,
    /// Human-readable label for progress and error messages.
    pub label: String,
    /// The MCP resource URL discovery starts from.
    pub url: String,
    /// Pre-registered client id (servers without dynamic registration).
    pub client_id: Option<String>,
    /// Fixed loopback port registered for a public client; otherwise use the DCR port range.
    pub callback_port: Option<u16>,
    /// Secret of a pre-registered confidential client, sent as `client_secret_post`.
    pub client_secret: Option<String>,
    /// Requested scopes; defaults to the server's advertised scopes.
    pub scopes: Option<String>,
}

/// The interactive surface a login drives (the TS `OAuthLoginCallbacks`).
pub trait McpLoginUi: Send + Sync {
    /// Narration for slow steps (discovery, registration, exchange).
    fn on_progress(&self, message: &str);
    /// The authorization URL to open in a browser, plus instructions.
    fn on_auth(&self, url: &str, instructions: &str);
    /// Ask the user for one line of input (the callback fallback when no
    /// manual-paste surface exists). An error means the login was cancelled.
    fn on_prompt(
        &self,
        message: &str,
        placeholder: &str,
    ) -> Pin<Box<dyn Future<Output = Result<String>> + Send>>;
    /// The manual-paste channel racing the browser callback: `None` when
    /// the surface offers none. Resolving `None` cancels the paste.
    fn on_manual_code_input(&self) -> Option<Pin<Box<dyn Future<Output = Option<String>> + Send>>>;
}
