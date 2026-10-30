//! The context-line ladder: exact, prefix, unicode, substring, then fuzzy.

use super::super::{ContextMatchStrategy, find_context_line};

#[test]
fn context_matching_ladder() {
    assert_eq!(
        find_context_line(&["function foo() {"], "function foo() {", 0, true, false).strategy,
        Some(ContextMatchStrategy::Exact)
    );
    assert_eq!(
        find_context_line(&["  function foo()  {"], "function foo() {", 0, true, false).strategy,
        Some(ContextMatchStrategy::Prefix)
    );
    assert_eq!(
        find_context_line(
            &["const msg = \"Hello – World\";"],
            "const msg = \"Hello - World\";",
            0,
            true,
            false
        )
        .index,
        Some(0)
    );
    assert_eq!(
        find_context_line(
            &["function calculateTotalWithTax(items, taxRate) {"],
            "function calculateTotalWithTax(items",
            0,
            true,
            false
        )
        .strategy,
        Some(ContextMatchStrategy::Prefix)
    );
    assert_eq!(
        find_context_line(
            &["// comment: calculateTotal here"],
            "calculateTotal",
            0,
            true,
            false
        )
        .strategy,
        Some(ContextMatchStrategy::Substring)
    );
    let fuzzy = find_context_line(
        &["functoin calclateTotal(itms) {"],
        "function calculateTotal(items) {",
        0,
        true,
        false,
    );
    assert_eq!(fuzzy.strategy, Some(ContextMatchStrategy::Fuzzy));
    assert!(fuzzy.confidence > 0.8);
}
