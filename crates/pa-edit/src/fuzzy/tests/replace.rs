//! Replacement: indentation adjustment, the all-occurrences path, and the
//! misses that hand a `MatchOutcome` back to the caller.

use super::super::{ReplaceOutcome, ReplaceResult, replace_text};

fn replaced(outcome: ReplaceOutcome) -> ReplaceResult {
    match outcome {
        ReplaceOutcome::Replaced(result) => result,
        ReplaceOutcome::Missed(outcome) => {
            panic!("expected a replacement, missed: {outcome:?}")
        }
    }
}

#[test]
fn replace_text_adjusts_indentation() {
    let result = replaced(
        replace_text(
            "    foo\n    bar",
            "foo\nbar",
            "foo\nbaz\nbar",
            true,
            false,
            None,
        )
        .unwrap(),
    );
    assert_eq!(
        result,
        ReplaceResult {
            content: "    foo\n    baz\n    bar".to_owned(),
            count: 1,
        }
    );

    let deindented = replaced(
        replace_text(
            "    foo\n    bar",
            "        foo\n        bar",
            "        foo\n        baz",
            true,
            false,
            Some(0.9),
        )
        .unwrap(),
    );
    assert_eq!(deindented.content, "    foo\n    baz");
}

#[test]
fn replace_text_all_exact_and_fuzzy() {
    assert_eq!(
        replaced(replace_text("foo foo", "foo", "bar", false, true, None).unwrap()),
        ReplaceResult {
            content: "bar bar".to_owned(),
            count: 2
        }
    );
    let old = "a".repeat(50);
    let first = format!("{}b", "a".repeat(49));
    let second = format!("{}cccccc", "a".repeat(44));
    let new = format!("{old}\nexpanded");
    assert_eq!(
        replaced(
            replace_text(
                &format!("{first}\n{second}"),
                &old,
                &new,
                true,
                true,
                Some(0.8)
            )
            .unwrap()
        ),
        ReplaceResult {
            content: format!("{new}\n{new}"),
            count: 2
        }
    );
}

#[test]
fn replace_text_misses_on_ambiguity_and_normalizes_line_endings() {
    let ReplaceOutcome::Missed(ambiguous) =
        replace_text("foo\nbar\nfoo", "foo", "x", false, false, None).unwrap()
    else {
        panic!("two exact occurrences must not be replaced");
    };
    assert_eq!(ambiguous.occurrences, Some(2));
    assert_eq!(
        replaced(replace_text("a\r\nb", "a\r\nb", "c\r\nd", false, false, None).unwrap()),
        ReplaceResult {
            content: "c\nd".to_owned(),
            count: 1
        }
    );
    let ReplaceOutcome::Missed(missing) =
        replace_text("abc", "missing", "x", false, false, None).unwrap()
    else {
        panic!("an absent string must not be replaced");
    };
    assert!(missing.matched.is_none());
}

#[test]
fn empty_old_text_is_an_apply_error() {
    assert_eq!(
        replace_text("x", "", "y", false, false, None)
            .unwrap_err()
            .to_string(),
        "oldText must not be empty."
    );
}
