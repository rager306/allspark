//! ast-grep strictness ladder, re-exported for graph consumers.
//!
//! The ladder controls how loosely an ast-grep pattern matches a tree:
//! `Cst` is byte-exact, `Smart` ignores trivia and identifiers' contents,
//! `Template` is the loosest. `pa-graph` exposes the same enum so callers that
//! search the graph and then re-run a structural match do not need a second
//! dependency on `pa-ast`.

pub use pa_ast::ops::AstMatchStrictness;

/// The ladder in increasing looseness, as documented by ast-grep.
pub const LADDER: [AstMatchStrictness; 6] = [
    AstMatchStrictness::Cst,
    AstMatchStrictness::Smart,
    AstMatchStrictness::Ast,
    AstMatchStrictness::Relaxed,
    AstMatchStrictness::Signature,
    AstMatchStrictness::Template,
];

/// Resolve an optional strictness to ast-grep's default (`Smart`).
#[must_use]
pub fn resolve(value: Option<AstMatchStrictness>) -> AstMatchStrictness {
    value.unwrap_or(AstMatchStrictness::Smart)
}

/// Human-readable description of a strictness level.
#[must_use]
pub fn describe(value: AstMatchStrictness) -> &'static str {
    match value {
        AstMatchStrictness::Cst => "exact concrete syntax tree match",
        AstMatchStrictness::Smart => "smart match (ignores trivia)",
        AstMatchStrictness::Ast => "match by AST structure",
        AstMatchStrictness::Relaxed => "relaxed structural match",
        AstMatchStrictness::Signature => "match by signature only",
        AstMatchStrictness::Template => "loosest template match",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ladder_is_ordered_and_complete() {
        assert_eq!(LADDER.len(), 6);
        assert_eq!(LADDER[0], AstMatchStrictness::Cst);
        assert_eq!(LADDER[5], AstMatchStrictness::Template);
    }

    #[test]
    fn default_is_smart() {
        assert_eq!(resolve(None), AstMatchStrictness::Smart);
        assert_eq!(
            resolve(Some(AstMatchStrictness::Ast)),
            AstMatchStrictness::Ast
        );
    }

    #[test]
    fn describe_is_nonempty_for_every_level() {
        for level in LADDER {
            assert_ne!(describe(level), "", "every level has a description");
        }
    }
}
