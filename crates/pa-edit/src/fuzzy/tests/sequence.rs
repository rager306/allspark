//! The exact-to-character ladder, and the fuzzy/character fallback that places a
//! near-miss sequence.

use super::super::{SEQUENCE_FUZZY_THRESHOLD, SequenceMatchStrategy, seek_sequence};

#[test]
fn sequence_matching_ladder() {
    assert_eq!(
        seek_sequence(&["foo", "bar", "baz"], &["bar", "baz"], 0, false, true).index,
        Some(1)
    );
    assert_eq!(
        seek_sequence(&["foo   ", "bar\t\t"], &["foo", "bar"], 0, false, true).strategy,
        Some(SequenceMatchStrategy::TrimTrailing)
    );
    assert_eq!(
        seek_sequence(&["    foo   ", "   bar\t"], &["foo", "bar"], 0, false, true).strategy,
        Some(SequenceMatchStrategy::Trim)
    );
    assert_eq!(
        seek_sequence(&["a", "b", "c", "d", "e"], &["d", "e"], 0, true, true).index,
        Some(3)
    );
    assert_eq!(
        seek_sequence(
            &["import asyncio  # local import – avoids top‑level dep"],
            &["import asyncio  # local import - avoids top-level dep"],
            0,
            false,
            true
        )
        .strategy,
        Some(SequenceMatchStrategy::Unicode)
    );
    assert_eq!(
        seek_sequence(&["foo", "bar"], &[], 5, false, true).index,
        Some(5)
    );
    assert_eq!(
        seek_sequence(&["one"], &["too", "many"], 0, false, true).index,
        None
    );
    let minor = seek_sequence(
        &["function greet() {", "  console.log(\"Hello!\");", "}"],
        &["function greet() {", "  console.log(\"Hello!\")  ", "}"],
        0,
        false,
        true,
    );
    assert_eq!(minor.index, Some(0));
    assert!(minor.confidence >= SEQUENCE_FUZZY_THRESHOLD);
}

#[test]
fn sequence_fuzzy_and_character_fallback() {
    let lines = [
        "function calculateTotal(items) {",
        "  let sum = 0;",
        "  for (const item of items) {",
        "    sum += item.price * item.quantity;",
        "  }",
        "  return sum;",
        "}",
    ];
    let result = seek_sequence(
        &lines,
        &[
            "  for (const item of items)  {",
            "    sum += item.price*item.quantity;",
        ],
        0,
        false,
        true,
    );
    assert_eq!(result.index, Some(2));
    assert!(result.confidence > 0.9);
}
