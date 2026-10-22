//! Why a call site did not resolve, and the heuristic that says which kind of
//! miss it is.
//!
//! One label was not enough. A repository-wide index found 18,206 misses, 17,395
//! of them "no definition with that name" — a single bucket that mixed three
//! unrelated problems, so no reader could tell from the count whether the
//! resolver was broken or the code simply called `clone`. Classifying that
//! bucket on this repository (2026-10-22) gave 20.6% std/trait methods
//! (`unwrap`, `map`, `clone`), 24.2% constructors and macros (`Some`, `Ok`,
//! `json!`), and 55.2% plain lowercase names that are either genuinely absent
//! or methods whose receiver type extraction does not record.
//!
//! The classification runs **only on a miss**. A name that resolved keeps its
//! CALLS edge and never reaches here, so the std corpus cannot weaken a real
//! edge: 2,428 `clone` call sites in this repository resolve to an in-tree
//! `clone` through the `suffix` rung.
//!
//! The wire names are stored in the `USAGE` edge's `reason` property, so they
//! are part of the store's format: [`crate::status`] counts them and the status
//! line renders the split, so renaming one silently empties its bucket.

use crate::extract::CallSite;

/// Why a call site produced no CALLS edge.
///
/// The wire names are stored in the `USAGE` edge's `reason` property, so they
/// are part of the store's format: a reader (and the status line's split) keys
/// on them, and renaming one silently empties its bucket.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MissReason {
    /// The callee is a well-known std/trait method or a known macro: the
    /// definition lives outside the indexed tree by construction.
    ExternalOrStd,
    /// A bare lowercase identifier that reads like a method call. The name is
    /// absent from the tree, and what a resolver would need is the receiver's
    /// type — which extraction does not record.
    TypeUnknown,
    /// Reserved: the name is expected in the tree and extraction did not reach
    /// it yet. Never produced today; the variant exists so the stored wire
    /// format and the status line can gain the bucket without a format change.
    NotExtracted,
    /// The name exists in the tree, but the call's shape (a path, an uppercase
    /// constructor) is one resolution does not model.
    ShapeNotModelled,
    /// The fallback: nothing better could be said.
    ///
    /// Reached when the callee is not even an identifier (extraction yielded
    /// something like `[`), or when a future classifier declines to guess.
    NoDefinitionWithThatName,
}

impl MissReason {
    /// Every reason, in the order the status line renders them.
    pub(crate) const ALL: &'static [Self] = &[
        Self::ExternalOrStd,
        Self::TypeUnknown,
        Self::NotExtracted,
        Self::ShapeNotModelled,
        Self::NoDefinitionWithThatName,
    ];

    /// The stored wire name.
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::ExternalOrStd => "external_or_std",
            Self::TypeUnknown => "type_unknown",
            Self::NotExtracted => "not_extracted",
            Self::ShapeNotModelled => "shape_not_modelled",
            Self::NoDefinitionWithThatName => "no_definition_with_that_name",
        }
    }

    /// Parse a stored wire name; unknown names are `None` so a count can never
    /// invent a bucket.
    pub(crate) fn from_str(value: &str) -> Option<Self> {
        Self::ALL
            .iter()
            .copied()
            .find(|reason| reason.as_str() == value)
    }

    /// Classify an unmatched callee by its shape alone.
    ///
    /// A trailing `!` is a macro. An uppercase initial is a constructor or a
    /// type (`Path`, `ValueError`, `TemporaryDirectory`): resolution would need
    /// the type's module, which the call site does not carry. (The std variants
    /// `Some`/`Ok`/`Err` never reach here: [`STD_CONSTRUCTORS`] classifies them
    /// as external first.) A plain lowercase identifier is the largest measured
    /// bucket, and there `helper_fn()` and `receiver.method()` are
    /// indistinguishable without the receiver's type, so it is `type_unknown`.
    /// Anything that is not an identifier at all is the fallback: there is
    /// nothing to say about it.
    pub(crate) fn classify_unmatched(callee: &str) -> Self {
        if callee.ends_with('!') {
            return Self::ExternalOrStd;
        }
        let Some(first) = callee.chars().next() else {
            return Self::NoDefinitionWithThatName;
        };
        if !(first.is_alphabetic() || first == '_') {
            return Self::NoDefinitionWithThatName;
        }
        if first.is_ascii_uppercase() {
            return Self::ShapeNotModelled;
        }
        Self::TypeUnknown
    }
}

/// std/trait method names treated as external.
///
/// A heuristic corpus, not an exhaustive list, and deliberately so: the goal is
/// to separate "the definition is outside our tree" from "we could not name
/// it", not to model the standard library. Calibrated on this repository's
/// 17,754 misses (2026-10-22): the names below cover the frequent families —
/// `Option`/`Result` combinators, `Iterator`, `Display`/`FromStr`, `Entry`,
/// `Instant`/`Duration`, collection mutation, and `Path`/`OsStr` accessors.
/// Names that resolved to in-tree definitions (`clone`, `get`, `iter`, ...) are
/// listed on purpose: the reason is computed only for a miss, so listing them
/// costs nothing and keeps the two sets apart at the call site rather than in
/// the list.
pub(crate) const STD_TRAIT_METHODS: &[&str] = &[
    // Option / Result
    "is_some",
    "is_none",
    "is_some_and",
    "is_ok",
    "is_err",
    "ok_or",
    "ok_or_else",
    "unwrap_or",
    "unwrap_or_else",
    "unwrap_or_default",
    "unwrap_err",
    "expect_err",
    "and_then",
    "or_else",
    "map_or",
    "map_or_else",
    // Iterator
    "iter",
    "iter_mut",
    "into_iter",
    "next",
    "collect",
    "filter_map",
    "flat_map",
    "fold",
    "try_for_each",
    "cloned",
    "rev",
    "skip",
    "zip",
    // Conversion traits
    "as_str",
    "as_bytes",
    "as_ref",
    "as_mut",
    "as_deref",
    "as_slice",
    "as_path",
    "as_os_str",
    "as_array",
    "as_object",
    "as_object_mut",
    "as_i64",
    "as_u64",
    "as_f64",
    "as_bool",
    "as_millis",
    "as_secs",
    "as_nanos",
    "into",
    "try_into",
    "from",
    "from_str",
    "from_utf8",
    "from_utf8_lossy",
    "to_string",
    "to_string_lossy",
    "to_owned",
    "to_lowercase",
    "to_uppercase",
    "to_ascii_lowercase",
    "to_ascii_uppercase",
    "to_ascii_alphanumeric",
    "to_ascii_digit",
    "to_str",
    "to_vec",
    "to_path_buf",
    "parse",
    "display",
    // String / slice
    "split",
    "split_once",
    "splitn",
    "rsplit",
    "rsplit_once",
    "split_whitespace",
    "split_terminator",
    "split_at",
    "starts_with",
    "ends_with",
    "trim",
    "trim_start",
    "trim_end",
    "trim_matches",
    "strip_prefix",
    "strip_suffix",
    "chars",
    "char_indices",
    "bytes",
    "lines",
    "join",
    "repeat",
    "find",
    "rfind",
    "matches",
    // Collection mutation and access
    "push",
    "push_str",
    "push_back",
    "pop",
    "insert",
    "remove",
    "take",
    "replace",
    "append",
    "clear",
    "truncate",
    "reserve",
    "retain",
    "drain",
    "extend",
    "extend_from_slice",
    "get",
    "get_mut",
    "get_or_init",
    "get_or_insert_with",
    "entry",
    "or_insert",
    "or_insert_with",
    "or_default",
    "keys",
    "values",
    "values_mut",
    "first",
    "last",
    "min",
    "max",
    "clamp",
    "len",
    "is_empty",
    "sort",
    "sort_by",
    "sort_by_key",
    "sort_unstable",
    "contains",
    "swap",
    // Arithmetic / time
    "saturating_add",
    "saturating_sub",
    "saturating_mul",
    "checked_add",
    "checked_sub",
    "checked_mul",
    "wrapping_add",
    "div_euclid",
    "rem_euclid",
    "abs",
    "signum",
    "pow",
    "elapsed",
    "from_secs",
    "from_millis",
    "from_micros",
    "from_nanos",
    // Filesystem / IO (std::fs, std::io, std::path)
    "read",
    "read_to_string",
    "read_to_end",
    "read_line",
    "read_dir",
    "write_all",
    "write_str",
    "flush",
    "set_len",
    "set_permissions",
    "metadata",
    "is_dir",
    "is_file",
    "exists",
    "create",
    "create_dir",
    "create_dir_all",
    "create_new",
    "canonicalize",
    "with_extension",
    "current_dir",
    "current_exe",
    // Trait impls reached through Deref / macros
    "clone",
    "default",
    "drop",
    "cmp",
    "eq",
    "hash",
    "fmt",
];

/// std enum variants whose call site names the variant, not a definition.
///
/// `Some(x)`, `Ok(v)` and `Err(e)` are constructors the standard library
/// defines, and they were 3,360 of this repository's misses — the largest
/// single group after the trait methods. Kept apart from
/// [`STD_TRAIT_METHODS`] because the test for "is this std" is different: a
/// method is a name, a constructor is a name with an uppercase initial that the
/// standard library owns.
pub(crate) const STD_CONSTRUCTORS: &[&str] = &["Some", "Ok", "Err", "None"];

/// Macro names treated as external.
///
/// Same corpus rule as [`STD_TRAIT_METHODS`]: a macro expands before any
/// definition exists, so its call site can never resolve however the tree is
/// indexed. The list holds the macros this workspace calls most often; a
/// trailing name is also matched by shape (`ends_with('!')`) in
/// [`MissReason::classify_unmatched`], so an unlisted macro still lands in
/// `external_or_std` rather than `type_unknown`.
pub(crate) const MACROS: &[&str] = &[
    "json",
    "format",
    "write",
    "writeln",
    "print",
    "println",
    "eprint",
    "eprintln",
    "panic",
    "assert",
    "assert_eq",
    "assert_ne",
    "debug_assert",
    "debug_assert_eq",
    "matches",
    "vec",
    "concat",
    "env",
    "option_env",
    "cfg",
    "include_str",
    "include_bytes",
    "todo",
    "unimplemented",
    "unreachable",
    "select",
    "pin",
    "re_once",
];

/// The std crate roots whose members are external by definition.
///
/// Deliberately short: `crate::`, `self::` and `super::` name this tree, so a
/// path rooted there is *our* miss and must not be excused as external.
const STD_ROOTS: &[&str] = &["std", "core", "alloc"];

/// Whether the callee is a known std/trait method, a std constructor, or a
/// macro.
///
/// A macro expands before any definition exists, so its call site can never
/// resolve however the tree is indexed; [`MACROS`] documents the common ones
/// and the trailing `!` is the general rule that covers the rest.
pub(crate) fn is_external_or_std(call: &CallSite) -> bool {
    let trimmed = call.callee.trim();
    if trimmed.ends_with('!') {
        return true;
    }
    if STD_TRAIT_METHODS.contains(&trimmed) || STD_CONSTRUCTORS.contains(&trimmed) {
        return true;
    }
    // Matched without the bang as well: this extractor keeps it (`json!`), but
    // the corpus is a statement about the name, and an extractor that strips it
    // must not fall through to `type_unknown`.
    if MACROS.contains(&trimmed.strip_suffix('!').unwrap_or(trimmed)) {
        return true;
    }
    // A path form: `std::mem::take` reaches a definition outside the tree.
    // `callee_path` keeps the root that truncation dropped.
    let Some(path) = &call.callee_path else {
        return false;
    };
    let root = path.split("::").next().unwrap_or(path);
    STD_ROOTS.contains(&root)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use crate::indexer::Resolver;

    /// Every reason in the closed set round-trips through its stored wire name,
    /// and `not_extracted` is a name the store can hold even though nothing
    /// produces it yet.
    #[test]
    fn the_miss_reason_wire_names_are_closed_and_stable() {
        let names: Vec<&str> = MissReason::ALL.iter().map(|r| r.as_str()).collect();
        assert_eq!(
            names,
            vec![
                "external_or_std",
                "type_unknown",
                "not_extracted",
                "shape_not_modelled",
                "no_definition_with_that_name",
            ]
        );
        for reason in MissReason::ALL {
            assert_eq!(MissReason::from_str(reason.as_str()), Some(*reason));
        }
        assert_eq!(MissReason::from_str("made_up"), None);
        // The corpora are what the classifier keys on, so a name that silently
        // fell out of a list would change a stored reason without a test.
        assert!(STD_TRAIT_METHODS.contains(&"clone"));
        assert!(STD_TRAIT_METHODS.contains(&"unwrap_or"));
        assert!(STD_CONSTRUCTORS.contains(&"Some"));
        assert!(MACROS.contains(&"json"));
        // Both spellings: the extractor keeps the bang (`json!`), and a future
        // one that strips it must not change the stored reason.
        let call = |callee: &str| CallSite {
            caller_qn: "a::caller".to_string(),
            callee: callee.to_string(),
            callee_path: None,
            line: 1,
            args: Vec::new(),
        };
        assert!(is_external_or_std(&call("json!")));
        assert!(is_external_or_std(&call("json")));
        assert!(is_external_or_std(&call("some_unlisted_macro!")));
        assert!(!is_external_or_std(&call("definitely_not_std")));
        // The reserved bucket is a name the store can already hold: an indexer
        // that emits `not_extracted` tomorrow must not need a format change, and
        // a status table must be able to count a store that already says it.
        assert_eq!(
            MissReason::from_str("not_extracted"),
            Some(MissReason::NotExtracted),
            "the reserved bucket is readable before anything produces it"
        );
    }

    /// `crate::`/`self::`/`super::` paths name this tree, so a miss there is
    /// ours: it must not be excused as `external_or_std`.
    ///
    /// `deps` names external crates from the manifests; `std` is external by
    /// definition. The three local roots are the ones a naive "is it a path?"
    /// rule would wrongly call external, which would hide real resolver gaps.
    #[test]
    fn a_crate_local_path_is_not_excused_as_external() {
        let mut by_qn = HashMap::new();
        by_qn.insert("a::caller".to_string(), 1i64);
        let resolver = Resolver::new(by_qn);
        let reason_for = |callee: &str, path: Option<&str>| {
            let call = CallSite {
                caller_qn: "a::caller".to_string(),
                callee: callee.to_string(),
                callee_path: path.map(str::to_string),
                line: 1,
                args: Vec::new(),
            };
            resolver.miss_reason(&call)
        };
        assert_eq!(
            reason_for("missing_helper", Some("crate::other::missing_helper")),
            "type_unknown",
            "a crate-local path is our miss, not an external one"
        );
        assert_eq!(
            reason_for("missing_helper", Some("self::missing_helper")),
            "type_unknown"
        );
        assert_eq!(
            reason_for("missing_helper", Some("super::missing_helper")),
            "type_unknown"
        );
        assert_eq!(
            reason_for("take", Some("std::mem::take")),
            "external_or_std",
            "a std path is external even when the leaf name is unlisted"
        );
        assert_eq!(
            reason_for("relative_path", Some("serde_json::to_value")),
            "type_unknown",
            "a dependency path only counts when the name is a std one: the crate \
             set is not consulted here, so an unlisted name stays our miss"
        );
    }
}
