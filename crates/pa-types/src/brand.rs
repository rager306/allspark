//! The product's user-facing names, in ONE place.
//!
//! The fork ships two names on purpose (see AGENTS.md, "Fork identity and
//! isolation"): the binary is `prime-agent` for upstream and TS-parity reasons,
//! while the product the user runs is `allspark`. Every help line, usage error
//! and hint that tells a user what to type must name the LAUNCHER, because that
//! is what is on their `PATH` — telling them to run `prime-agent` sends them to
//! whichever binary happens to be there, which on a co-installed machine is the
//! upstream build that cannot see their sessions.
//!
//! WHY A CONSTANT RATHER THAN A LITERAL PER SITE. Before this module ~96
//! user-visible strings spelled the command by hand. A rename then means finding
//! them by grep, and the ones that are easy to miss are exactly the ones a user
//! reads at the worst moment: the resume hint after quitting, an incompatible
//! daemon error, a usage message after a typo. One constant makes the answer a
//! one-line change and makes a stale spelling impossible to introduce silently.

/// The command a user types: the launcher installed on their `PATH`.
///
/// This is `allspark`, NOT the payload name. The payload stays `prime-agent`
/// (crates/pa-cli `[[bin]]`) so upstream merges and TS-parity diffs stay clean;
/// the launcher is what the install writes to `bin/`.
pub const INVOCATION: &str = "allspark";

/// The product name as prose, for sentences ("Prime Agent timed out").
pub const PRODUCT: &str = "Prime Agent";

/// The lowercase product name, for mid-sentence and log-adjacent text.
pub const PRODUCT_LOWER: &str = "allspark";

/// The daemon socket name the fork pins, matching the launcher.
pub const DAEMON_SOCKET_NAME: &str = "allspark-daemon.sock";

#[cfg(test)]
mod tests {
    use super::*;

    /// The launcher and the payload deliberately differ; this pins the pair so a
    /// "fix" that unifies them fails here rather than in the installer, where the
    /// launcher's socket pin is what keeps a co-installed upstream daemon apart.
    #[test]
    fn the_launcher_is_not_the_payload_name() {
        assert_eq!(INVOCATION, "allspark");
        assert_ne!(INVOCATION, "prime-agent");
        assert_eq!(PRODUCT, "Prime Agent");
    }
}
