//! The confidence ladder in isolation, with no store and no files.

use std::collections::HashMap;

use crate::extract::CallSite;

use super::super::resolver::Resolver;

#[test]
fn resolver_ladder_prefers_same_module() {
    let mut by_qn = HashMap::new();
    by_qn.insert("a::m::foo".to_string(), 1i64);
    by_qn.insert("b::n::foo".to_string(), 2i64);
    by_qn.insert("a::m::caller".to_string(), 3i64);
    let resolver = Resolver::new(by_qn);
    let call = CallSite {
        caller_qn: "a::m::caller".to_string(),
        callee: "foo".to_string(),
        callee_path: None,
        line: 1,
        args: Vec::new(),
    };
    let (src, tgt, props) = resolver
        .resolve_call("a::m::caller", "a/m.rs", &call)
        .expect("resolve");
    assert_eq!(src, 3);
    assert_eq!(tgt, 1, "same-module foo must win");
    assert_eq!(
        props.get("strategy").and_then(|v| v.as_str()),
        Some("same_module")
    );
}

#[test]
fn resolver_unique_name_strategy() {
    let mut by_qn = HashMap::new();
    by_qn.insert("a::solo".to_string(), 5i64);
    by_qn.insert("a::caller".to_string(), 6i64);
    let resolver = Resolver::new(by_qn);
    let call = CallSite {
        caller_qn: "a::caller".to_string(),
        callee: "solo".to_string(),
        callee_path: None,
        line: 1,
        args: Vec::new(),
    };
    let (_, tgt, props) = resolver
        .resolve_call("a::caller", "a/m.rs", &call)
        .expect("resolve");
    assert_eq!(tgt, 5);
    assert_eq!(
        props.get("strategy").and_then(|v| v.as_str()),
        Some("unique")
    );
}
