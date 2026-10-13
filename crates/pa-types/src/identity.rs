//! Fork product identity: the single source of user-facing product strings and
//! on-disk names. Upstream-compatible code points at these constants instead
//! of hardcoding names, so a fork rebrands by editing this one file.
//!
//! Deliberately NOT renamed (wire/legacy compat): the `PRIME_AGENT_*` env
//! variable family (internal plumbing names, not paths), the `prime-inference`
//! provider id, the `ai.primeintellect.*` meta namespaces, and the legacy
//! `~/.prime/config.json` read for prime-inference credentials.

/// The product name shown in user-facing prose.
pub const APP_NAME: &str = "allspark";

/// The compiled binary name (`crates/pa-cli/Cargo.toml` must match).
pub const BINARY_NAME: &str = "allspark";

/// The per-user config/data directory under the home directory
/// (sessions, settings, auth, kernel venv, daemon sockets derive from it).
pub const CONFIG_DIR_NAME: &str = ".allspark";

/// The environment-variable prefix for every public and internal knob.
/// No fallback to the upstream prefix: the fork must never read a
/// `prime-agent` session's environment.
pub const ENV_PREFIX: &str = "ALLSPARK_";

/// The package-dir override (no upstream fallback).
pub const PACKAGE_DIR_ENV: &str = "ALLSPARK_PACKAGE_DIR";

/// The ACP `_meta` namespace key.
pub const META_NAMESPACE: &str = "ai.allspark.agent";

/// The trace-upload auth provider id.
pub const TRACE_PROVIDER_ID: &str = "allspark-traces";

/// The trace-upload provider display name.
pub const TRACE_PROVIDER_NAME: &str = "Allspark Traces";

/// The per-user runtime directory prefix under the temp dir
/// (`/tmp/allspark-<uid>`): daemon, worker, and supervisor sockets.
pub const RUNTIME_DIR_PREFIX: &str = "allspark";

/// The install prefix directory name under `~/.local/share`.
pub const INSTALL_PREFIX_NAME: &str = "allspark";

/// The channel marker file inside the install prefix's share dir.
pub const INSTALL_MARKER: &str = ".allspark-install";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_values_are_the_fork_brand() {
        assert_eq!(APP_NAME, "allspark");
        assert_eq!(BINARY_NAME, APP_NAME);
        assert_eq!(CONFIG_DIR_NAME, ".allspark");
        assert!(CONFIG_DIR_NAME.starts_with('.'));
        assert!(
            !CONFIG_DIR_NAME.contains('/'),
            "keep the dir flat: code joins subpaths itself"
        );
    }
}
