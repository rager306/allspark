//! Parse-regression auto-repair (E10).
//!
//! An edit can leave a file syntactically broken even though the change looked
//! well-formed in isolation. [`auto_repair`] takes the broken file, the
//! "culprit" hunks that are suspected of causing the regression, and a
//! [`RepairModel`] (a smol language model behind a trait seam), and returns a
//! fixed file **only if** the candidate parses cleanly.
//!
//! The acceptance rule is the whole point: a model proposal is never trusted on
//! its own. It is written out, re-parsed with `pa-ast`, and accepted only when
//! the resulting tree is syntactically valid — otherwise the repair is rejected
//! and the original bytes are left untouched. This keeps a hallucinating model
//! from making a broken file *differently* broken.
//!
//! ```no_run
//! use pa_graph::{auto_repair, BrokenHunk, RepairModel, RepairRequest};
//!
//! struct MyModel;
//! impl RepairModel for MyModel {
//!     fn propose(&self, _req: &RepairRequest) -> Option<String> {
//!         Some("fn f() {}".to_string())
//!     }
//! }
//!
//! let broken = "fn f( {";
//! let hunks = vec![BrokenHunk { start_line: 1, end_line: 1, text: "fn f( {".into() }];
//! let fixed = auto_repair(broken, "rust", &hunks, &MyModel);
//! assert!(fixed.is_some());
//! ```

use serde::{Deserialize, Serialize};

use crate::coverage::{classify_source, ParseHealth};

/// One hunk suspected of causing a parse regression, in file line coordinates.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BrokenHunk {
    /// 1-based inclusive first line.
    pub start_line: u32,
    /// 1-based inclusive last line.
    pub end_line: u32,
    /// The hunk's text as it currently appears in the file.
    pub text: String,
}

impl BrokenHunk {
    /// Whether this hunk contains `line`.
    #[must_use]
    pub const fn contains_line(&self, line: u32) -> bool {
        self.start_line <= line && line <= self.end_line
    }
}

/// Everything a [`RepairModel`] needs to propose a fix.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepairRequest {
    /// The full broken source.
    pub source: String,
    /// Language alias (`rust`, `python`, ...).
    pub language: String,
    /// The culprit hunks, in file order.
    pub hunks: Vec<BrokenHunk>,
    /// The source with the culprit hunks excised — the model's main hint.
    pub context_without_hunks: String,
    /// A short description of what the parser objected to.
    pub diagnosis: String,
}

/// The model seam: anything that can turn a [`RepairRequest`] into a candidate
/// replacement for the culprit hunks.
///
/// Implementations may be a real smol model, a deterministic rule engine, or a
/// test double. `auto_repair` treats every proposal as untrusted and validates
/// it before returning.
pub trait RepairModel {
    /// Propose a replacement for the culprit hunks, or `None` to decline.
    ///
    /// The returned string is spliced back over the hunk line ranges; it is not
    /// required to parse — [`auto_repair`] checks that.
    fn propose(&self, request: &RepairRequest) -> Option<String>;
}

/// A [`RepairModel`] that never proposes anything (the conservative default).
#[derive(Debug, Clone, Copy, Default)]
pub struct NoopRepairModel;

impl RepairModel for NoopRepairModel {
    fn propose(&self, _request: &RepairRequest) -> Option<String> {
        None
    }
}

/// The outcome of a successful repair.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Fixed {
    /// The repaired source that parses cleanly.
    pub source: String,
    /// The replacement the model proposed.
    pub replacement: String,
    /// The hunks that were replaced.
    pub repaired_hunks: Vec<BrokenHunk>,
    /// Parse health of the original file.
    pub before: String,
    /// Parse health of the repaired file.
    pub after: String,
}

/// Attempt to repair a parse regression, accepting the model's proposal only if
/// the result parses cleanly.
///
/// `language` is a `pa-ast` language alias (see
/// [`pa_ast::SupportLang::from_alias`]); an unrecognised alias returns `None`.
/// `hunks` must be non-empty and their line ranges must be inside `source`.
///
/// Returns `None` when the language is unknown, the hunks are empty, the model
/// declines, the splice is out of range, or the proposal does not parse.
#[must_use]
pub fn auto_repair(
    source: &str,
    language: &str,
    hunks: &[BrokenHunk],
    model: &dyn RepairModel,
) -> Option<Fixed> {
    if hunks.is_empty() {
        return None;
    }
    let lang = pa_ast::SupportLang::from_alias(language)?;
    let lines: Vec<&str> = source.split_inclusive('\n').collect();
    let mut sorted = hunks.to_vec();
    sorted.sort_by_key(|h| h.start_line);
    // Every hunk must address real lines.
    let line_count = lines.len() as u32;
    if sorted
        .iter()
        .any(|h| h.start_line == 0 || h.start_line > line_count || h.end_line < h.start_line)
    {
        return None;
    }

    let context = excise_hunks(&lines, &sorted);
    let diagnosis = diagnose(source, lang);
    let request = RepairRequest {
        source: source.to_string(),
        language: language.to_string(),
        hunks: sorted.clone(),
        context_without_hunks: context,
        diagnosis,
    };

    let replacement = model.propose(&request)?;
    let candidate = splice_replacement(&lines, &sorted, &replacement)?;

    // The gate: accept only a candidate that parses cleanly. `Partial`/`Unusable`
    // mean the model did not actually fix the regression.
    let after = classify_source(&candidate, lang);
    if after != ParseHealth::Clean {
        return None;
    }
    let before = classify_source(source, lang);
    Some(Fixed {
        source: candidate,
        replacement,
        repaired_hunks: sorted,
        before: before.detail(),
        after: after.detail(),
    })
}

/// Remove the culprit hunks from the source, leaving the surrounding context.
fn excise_hunks(lines: &[&str], hunks: &[BrokenHunk]) -> String {
    let mut out = String::new();
    for (i, line) in lines.iter().enumerate() {
        let lineno = i as u32 + 1;
        if hunks.iter().any(|h| h.contains_line(lineno)) {
            continue;
        }
        out.push_str(line);
    }
    out
}

/// Replace the culprit hunks with `replacement`, preserving the lines outside
/// them. Returns `None` if the hunks are out of range.
fn splice_replacement(lines: &[&str], hunks: &[BrokenHunk], replacement: &str) -> Option<String> {
    // Hunks are sorted by start; merge any that overlap so the splice is
    // unambiguous.
    let mut merged: Vec<(u32, u32)> = Vec::new();
    for h in hunks {
        if let Some(last) = merged.last_mut() {
            if h.start_line <= last.1 + 1 {
                last.1 = last.1.max(h.end_line);
                continue;
            }
        }
        merged.push((h.start_line, h.end_line));
    }

    let mut out = String::new();
    let mut cursor = 0usize; // next un-copied line index
    for (start, end) in merged {
        if end as usize > lines.len() {
            return None;
        }
        for line in &lines[cursor..(start as usize - 1)] {
            out.push_str(line);
        }
        out.push_str(replacement);
        if !replacement.ends_with('\n') && (end as usize) < lines.len() {
            out.push('\n');
        }
        cursor = end as usize;
    }
    for line in &lines[cursor..] {
        out.push_str(line);
    }
    Some(out)
}

/// A one-line description of the parser's objection, for the model prompt.
fn diagnose(source: &str, lang: pa_ast::SupportLang) -> String {
    let health = classify_source(source, lang);
    match health {
        ParseHealth::Clean => "source already parses cleanly".to_string(),
        _ => format!(
            "{}: {}",
            health.coverage_kind().unwrap_or("degraded"),
            health.detail()
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A model that always returns a fixed replacement.
    struct StaticModel(String);
    impl RepairModel for StaticModel {
        fn propose(&self, _request: &RepairRequest) -> Option<String> {
            Some(self.0.clone())
        }
    }

    /// A model that returns a still-broken replacement.
    struct BrokenModel;
    impl RepairModel for BrokenModel {
        fn propose(&self, _request: &RepairRequest) -> Option<String> {
            Some("fn f( {".to_string())
        }
    }

    #[test]
    fn accepts_only_a_parsing_replacement() {
        let broken = "fn f( {\n";
        let hunks = vec![BrokenHunk {
            start_line: 1,
            end_line: 1,
            text: "fn f( {".into(),
        }];
        let fixed = auto_repair(broken, "rust", &hunks, &StaticModel("fn f() {}".into()))
            .expect("good replacement is accepted");
        // The hunk is the file's only (final) line, so no trailing newline is
        // synthesized; the replacement is spliced verbatim.
        assert_eq!(fixed.source, "fn f() {}");
        assert_eq!(fixed.repaired_hunks.len(), 1);
        assert_eq!(fixed.after, "");
    }

    #[test]
    fn rejects_a_still_broken_replacement() {
        let broken = "fn f( {\n";
        let hunks = vec![BrokenHunk {
            start_line: 1,
            end_line: 1,
            text: "fn f( {".into(),
        }];
        assert!(
            auto_repair(broken, "rust", &hunks, &BrokenModel).is_none(),
            "a replacement that does not parse must be rejected"
        );
    }

    #[test]
    fn noop_model_declines() {
        let hunks = vec![BrokenHunk {
            start_line: 1,
            end_line: 1,
            text: "fn f( {".into(),
        }];
        assert!(auto_repair("fn f( {\n", "rust", &hunks, &NoopRepairModel).is_none());
    }

    #[test]
    fn empty_hunks_or_unknown_language_decline() {
        assert!(auto_repair("fn f() {}\n", "rust", &[], &StaticModel("x".into())).is_none());
        let hunks = vec![BrokenHunk {
            start_line: 1,
            end_line: 1,
            text: "x".into(),
        }];
        assert!(auto_repair("x\n", "not-a-language", &hunks, &StaticModel("y".into())).is_none());
    }

    #[test]
    fn out_of_range_hunks_decline() {
        let hunks = vec![BrokenHunk {
            start_line: 99,
            end_line: 100,
            text: "x".into(),
        }];
        assert!(auto_repair("fn f() {}\n", "rust", &hunks, &StaticModel("y".into())).is_none());
    }

    #[test]
    fn context_omits_the_culprit_hunk() {
        let src = "fn keep() {}\nfn bad( {\nfn tail() {}\n";
        let hunks = vec![BrokenHunk {
            start_line: 2,
            end_line: 2,
            text: "fn bad( {".into(),
        }];
        let context = excise_hunks(&src.split_inclusive('\n').collect::<Vec<_>>(), &hunks);
        assert!(context.contains("fn keep"));
        assert!(context.contains("fn tail"));
        assert!(!context.contains("fn bad"));
    }

    #[test]
    fn multi_line_hunk_replacement_keeps_surroundings() {
        let src = "fn a() {}\nbroken line 1\nbroken line 2\nfn b() {}\n";
        let hunks = vec![BrokenHunk {
            start_line: 2,
            end_line: 3,
            text: "broken".into(),
        }];
        let fixed =
            auto_repair(src, "rust", &hunks, &StaticModel("fn mid() {}".into())).expect("repair");
        assert_eq!(fixed.source, "fn a() {}\nfn mid() {}\nfn b() {}\n");
    }
}
