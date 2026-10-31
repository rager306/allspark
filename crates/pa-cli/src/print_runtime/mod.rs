//! The headless print runtime: single-shot prompt -> answer over the pa-core
//! session engine with a real pa-ai provider.
//!
//! This facade owns [`PrintRuntime`] and the selector-error renderer the
//! interactive mode shares; the mode entrypoints, the engine assembly, the
//! session selection, and the prompt driver live in the modules beside it. The
//! engine types `print_rlm` builds inline children from are re-exported here, so
//! a caller keeps naming the same path.

use crate::mode::{AppMode, MissingSubsystem, RunOptions};
use pa_core::session::discovery::SessionSelectorError;

mod driver;
mod engine;
mod faux;
mod modes;
#[cfg(test)]
mod print_model_tests;
mod selection;
#[cfg(test)]
mod tests;

use modes::{run_acp_mode, run_print_mode, run_rpc_mode};

pub use engine::ProviderTargetSlot;
pub(crate) use engine::{
    build_engine, ChildEngineOverrides, ChildLane, HeadlessEmbedding, HeadlessEmbeddingFields,
    HeadlessEngine,
};
pub(crate) use faux::{faux_response_steps, FauxChildQueue};

pub struct PrintRuntime;

impl crate::mode::Runtime for PrintRuntime {
    fn run(&self, options: &RunOptions) -> Result<i32, MissingSubsystem> {
        if options.list_models.is_some() {
            return match crate::list_models::run(options) {
                Ok(code) => Ok(code),
                Err(message) => {
                    eprintln!("Error: {message}");
                    Ok(1)
                }
            };
        }
        match options.app_mode {
            // Runtime failures print themselves and exit non-zero; the typed
            // MissingSubsystem channel stays reserved for unwired subsystems.
            AppMode::Print | AppMode::Json => match run_print_mode(options) {
                Ok(code) => Ok(code),
                Err(message) => {
                    eprintln!("Error: {message}");
                    Ok(1)
                }
            },
            AppMode::Interactive => match crate::interactive_mode::run_interactive_mode(options) {
                Ok(code) => Ok(code),
                Err(error) => {
                    eprintln!("Error: {error:#}");
                    Ok(1)
                }
            },
            AppMode::Daemon => {
                match crate::daemon_mode::run_daemon_mode(options.daemon_socket.as_deref()) {
                    Ok(code) => Ok(code),
                    Err(error) => {
                        eprintln!("Error: {error:#}");
                        Ok(1)
                    }
                }
            }
            // ACP mode: a thin JSON-RPC stdio transport over a daemon
            // session.
            AppMode::Acp => match run_acp_mode(options) {
                Ok(code) => Ok(code),
                Err(error) => {
                    eprintln!("Error: {error:#}");
                    Ok(1)
                }
            },
            AppMode::Rpc => match run_rpc_mode(options) {
                Ok(code) => Ok(code),
                Err(error) => {
                    eprintln!("Error: {error:#}");
                    Ok(1)
                }
            },
        }
    }
}

pub(crate) fn render_selector_error(error: &SessionSelectorError) -> String {
    format!(
        "{}.{}\nOpen {} and press left-arrow to browse sessions.",
        error.message(),
        error.suggestion().unwrap_or_default(),
        pa_types::brand::INVOCATION
    )
}
