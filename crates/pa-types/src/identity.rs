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

/// The fork package-dir override checked before the legacy
/// [`LEGACY_PACKAGE_DIR_ENV`] (`PI_PACKAGE_DIR`, kept for TS parity).
pub const PACKAGE_DIR_ENV: &str = "ALLSPARK_PACKAGE_DIR";

/// The legacy package-dir environment variable from upstream.
pub const LEGACY_PACKAGE_DIR_ENV: &str = "PI_PACKAGE_DIR";

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
