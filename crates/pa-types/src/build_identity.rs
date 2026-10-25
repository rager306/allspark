//! Build identity: the content stamp a binary carries and the one predicate
//! that decides whether two processes may talk.
//!
//! Why this exists (measured, ADR-0001 G-1/G-2): a `version` string is read from
//! `package.json` *next to* the binary (a file, forgeable), `buildId` was derived
//! from that same version, and `DAEMON_SCHEMA_ID` is a hand-maintained literal —
//! so two different builds of one version were indistinguishable, and a TUI
//! silently attached to a daemon running yesterday's code.
//!
//! The stamp is compiled in by `build.rs` in the composition root (`pa-cli`),
//! which stamps ONLY the bin crate so the library crates do not recompile on
//! every commit (grok-build's `xai-grok-pager-bin/build.rs` is the reference
//! shape). The stamp reaches `daemon_hello` as `runtime.build_hash` through a
//! composition-root seam, and the comparison happens in exactly one place:
//! [`build_compatible`].

/// The build stamp as it travels on the wire and compares.
///
/// `None` means the side carries no stamp: an unstamped build (no `.git`, a
/// vendored tree) or a pre-upgrade binary. Fail-closed: [`build_compatible`]
/// answers [`BuildCompat::Unknown`] and callers treat it as a mismatch.
pub type BuildHash = Option<String>;

/// Whether a daemon's build identity matches this process's own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuildCompat {
    /// Both sides carry a stamp and they are equal.
    Match,
    /// Both sides carry stamps and they differ: different code.
    Mismatch,
    /// At least one side has no stamp. Fail-closed: treat as a mismatch and
    /// say so — "unverified", never "compatible".
    Unknown,
}

/// The one predicate. Every daemon-acceptance path calls this; none implements
/// its own comparison.
///
/// `daemon_hash` is the hash on the daemon's `daemon_hello`; `own_hash` is this
/// process's compiled-in stamp. `None` on either side is
/// [`BuildCompat::Unknown`], the fail-closed answer.
#[must_use]
pub fn build_compatible(daemon_hash: BuildHash, own_hash: BuildHash) -> BuildCompat {
    match (daemon_hash, own_hash) {
        // An empty stamp is "could not be produced", not a real identity: it
        // must not smuggle a match (and a real stamp is never empty — 12 hex).
        (Some(daemon), Some(own)) if !daemon.is_empty() && !own.is_empty() => {
            if daemon == own {
                BuildCompat::Match
            } else {
                BuildCompat::Mismatch
            }
        }
        _ => BuildCompat::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::{build_compatible, BuildCompat};

    fn hash_a() -> String {
        String::from("559751fdcec0")
    }
    fn hash_b() -> String {
        String::from("01de64db25e5")
    }

    #[test]
    fn equal_hashes_match() {
        assert_eq!(
            build_compatible(Some(hash_a()), Some(hash_a())),
            BuildCompat::Match
        );
    }

    #[test]
    fn different_hashes_do_not_match() {
        assert_eq!(
            build_compatible(Some(hash_a()), Some(hash_b())),
            BuildCompat::Mismatch
        );
    }

    #[test]
    fn a_missing_stamp_is_unknown_not_a_match() {
        // The incident class: two builds, one unstamped, compared on version
        // only. Fail-closed means "no stamp" can never read as "compatible".
        assert_eq!(build_compatible(None, Some(hash_a())), BuildCompat::Unknown);
        assert_eq!(build_compatible(Some(hash_a()), None), BuildCompat::Unknown);
        assert_eq!(build_compatible(None, None), BuildCompat::Unknown);
    }

    #[test]
    fn an_empty_string_is_no_stamp() {
        // "" is what a composition root hands over when the stamp could not be
        // produced; it must not smuggle a match.
        assert_eq!(
            build_compatible(Some(String::new()), Some(hash_a())),
            BuildCompat::Unknown
        );
    }
}
