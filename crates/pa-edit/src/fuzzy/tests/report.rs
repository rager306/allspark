//! The two formatters reproduce the TypeScript message byte-for-byte, including
//! the occurrence-limit suffix.

use super::super::{FuzzyMatch, MatchOutcome, format_match_error, format_occurrence_error};

#[test]
fn match_error_matches_typescript_formatter() {
    let closest = FuzzyMatch {
        actual_text: "alpha\ngamma".to_owned(),
        start_index: 10,
        start_line: 4,
        confidence: 0.874,
    };
    assert_eq!(
        format_match_error("src/a.ts", "alpha\nbeta", Some(&closest), true, 0.95, None),
        "Could not find a close enough match in src/a.ts.\n\nClosest match (87% similar) at line \
			 4:\n  - beta\n  + gamma\nClosest match was below the 95% similarity threshold."
    );
    assert_eq!(
        format_match_error("src/a.ts", "x", None, false, 0.95, None),
        "Could not find the exact text in src/a.ts. The old text must match exactly including \
			 all whitespace and newlines."
    );
    assert_eq!(
        format_match_error(
            "src/a.ts",
            "alpha\nbeta",
            Some(&closest),
            true,
            0.95,
            Some(3)
        ),
        "Could not find a close enough match in src/a.ts.\n\nClosest match (87% similar) at line \
			 4:\n  - beta\n  + gamma\nFound 3 high-confidence matches. Provide more context to make \
			 it unique."
    );
    assert_eq!(
        format_match_error("src/a.ts", "alpha\nbeta", Some(&closest), false, 0.95, None),
        "Could not find the exact text in src/a.ts.\n\nClosest match (87% similar) at line 4:\n  \
			 - beta\n  + gamma\nFuzzy matching is disabled. Enable 'Edit fuzzy match' in settings to \
			 accept high-confidence matches."
    );
    assert_eq!(
        format_match_error("src/a.ts", "x", None, true, 0.95, None),
        "Could not find a close enough match in src/a.ts."
    );
}

#[test]
fn occurrence_error_includes_preview_limit_suffix() {
    let outcome = MatchOutcome {
        occurrences: Some(7),
        occurrence_previews: Some(vec!["preview".to_owned()]),
        ..MatchOutcome::default()
    };
    assert_eq!(
        format_occurrence_error("a.ts", &outcome),
        "Found 7 occurrences in a.ts (showing first 5 of 7):\n\npreview\n\nAdd more context \
			 lines to disambiguate."
    );
}
