//! Diagnostics tracking: new-vs-seen diffing, keyed to symbols where possible.
//!
//! FORK-ONLY (allspark). Synthesizes two upstream designs:
//! - oh-my-pi's `DiagnosticsLedger`: a per-file set of seen diagnostic
//!   identities, so repeated polls only surface *fresh* diagnostics.
//! - Serena's `DiagnosticsDiff` + `GroupedDiagnostics`: compare a snapshot
//!   taken before an edit against the diagnostics after it, and attribute each
//!   new diagnostic to the symbol that owns it.
//!
//! The identity here follows Serena: message + range + severity + code + source.
//! Range is part of the identity so a diagnostic that merely *moved* counts as
//! new, which is what a reader wants after an edit.

use std::collections::{HashMap, HashSet};

use lsp_types::Diagnostic;

/// A diagnostic's identity: equal diagnostics collapse to one ledger entry.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DiagnosticIdentity {
    /// The diagnostic's message text.
    pub message: String,
    /// Start line (0-based, LSP coordinates).
    pub start_line: u32,
    /// Start character (0-based, UTF-16 units).
    pub start_character: u32,
    /// End line (0-based).
    pub end_line: u32,
    /// End character (0-based, UTF-16 units).
    pub end_character: u32,
    /// The protocol severity code (1=Error..4=Hint), when the server set one.
    pub severity: Option<i32>,
    /// The server-supplied diagnostic code, when present.
    pub code: Option<String>,
    /// The server that produced the diagnostic, when present.
    pub source: Option<String>,
}

impl DiagnosticIdentity {
    /// Build an identity from a wire diagnostic.
    #[must_use]
    pub fn from_diagnostic(diagnostic: &Diagnostic) -> Self {
        let start = diagnostic.range.start;
        let end = diagnostic.range.end;
        Self {
            message: diagnostic.message.clone(),
            start_line: start.line,
            start_character: start.character,
            end_line: end.line,
            end_character: end.character,
            severity: severity_code(diagnostic.severity),
            code: diagnostic.code.as_ref().map(code_to_string),
            source: diagnostic.source.clone(),
        }
    }
}

fn code_to_string(code: &lsp_types::NumberOrString) -> String {
    match code {
        lsp_types::NumberOrString::Number(n) => n.to_string(),
        lsp_types::NumberOrString::String(s) => s.clone(),
    }
}

/// The numeric code behind a `DiagnosticSeverity`.
///
/// `lsp_types::DiagnosticSeverity` is a transparent newtype whose inner `i32`
/// is private, so the number is recovered through its serde representation
/// rather than a field access.
#[must_use]
pub fn severity_code(severity: Option<lsp_types::DiagnosticSeverity>) -> Option<i32> {
    let severity = severity?;
    serde_json::to_value(severity)
        .ok()
        .and_then(|value| value.as_i64())
        .and_then(|code| i32::try_from(code).ok())
}

/// Human-readable severity name (Serena's `_diagnostic_severity_name`).
#[must_use]
pub fn severity_name(severity: Option<i32>) -> &'static str {
    match severity {
        Some(1) => "Error",
        Some(2) => "Warning",
        Some(3) => "Information",
        Some(4) => "Hint",
        _ => "Unknown",
    }
}

/// A per-file ledger of diagnostics already reported.
#[derive(Debug, Default)]
pub struct DiagnosticsLedger {
    seen: HashMap<String, HashSet<DiagnosticIdentity>>,
}

impl DiagnosticsLedger {
    /// An empty ledger with no recorded diagnostics.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Record `diagnostics` for `path` and return only those not seen before.
    ///
    /// The ledger is replaced with the current set, so a diagnostic that
    /// disappears and later reappears is fresh again (matching oh-my-pi).
    pub fn reduce(&mut self, path: &str, diagnostics: &[Diagnostic]) -> Vec<Diagnostic> {
        let previous = self.seen.get(path);
        let mut current = HashSet::new();
        let mut fresh = Vec::new();
        for diagnostic in diagnostics {
            let identity = DiagnosticIdentity::from_diagnostic(diagnostic);
            if previous.is_none_or(|prev| !prev.contains(&identity)) {
                fresh.push(diagnostic.clone());
            }
            current.insert(identity);
        }
        if current.is_empty() {
            self.seen.remove(path);
        } else {
            self.seen.insert(path.to_string(), current);
        }
        fresh
    }

    /// Snapshot the current identities for `path`, for a before/after diff.
    #[must_use]
    pub fn snapshot(&self, path: &str) -> HashSet<DiagnosticIdentity> {
        self.seen.get(path).cloned().unwrap_or_default()
    }
}

/// A diagnostics snapshot taken *before* an edit: the identities plus the
/// publish generation they were read at (Serena's
/// `PublishedDiagnosticsSnapshot`, `lsp_diagnostics.py:60`).
///
/// The generation is what makes the snapshot fresh-safe (L13): a later
/// `publishDiagnostics` whose generation has not advanced cannot have been
/// computed against the post-edit document, so diffing against it would be
/// meaningless. [`accepts_generation`](Self::accepts_generation) gates that.
#[derive(Debug, Clone, Default)]
pub struct DiagnosticsSnapshot {
    /// The publish generation the snapshot was read at.
    pub generation: u64,
    /// The diagnostic identities observed.
    pub identities: HashSet<DiagnosticIdentity>,
}

impl DiagnosticsSnapshot {
    /// Capture `diagnostics` at `generation`.
    #[must_use]
    pub fn capture(diagnostics: &[Diagnostic], generation: u64) -> Self {
        Self {
            generation,
            identities: diagnostics
                .iter()
                .map(DiagnosticIdentity::from_diagnostic)
                .collect(),
        }
    }

    /// Whether a publish at `generation` is fresh relative to this snapshot.
    ///
    /// A generation at or beyond the snapshot's is accepted; an older one is
    /// stale (the server computed it against a document this snapshot already
    /// supersedes).
    #[must_use]
    pub fn accepts_generation(&self, generation: u64) -> bool {
        generation >= self.generation
    }

    /// The new diagnostics in `after` relative to this snapshot, or an empty
    /// list when `after_generation` is stale.
    #[must_use]
    pub fn diff(&self, after: &[Diagnostic], after_generation: u64) -> Vec<Diagnostic> {
        if !self.accepts_generation(after_generation) {
            return Vec::new();
        }
        new_since(&self.identities, after)
    }
}

/// Compute diagnostics in `after` that are absent from a `before` snapshot
/// (Serena's `DiagnosticsDiff`, minus the symbol attribution which the caller
/// layers on with `symbols::resolve`).
#[must_use]
pub fn new_since<S: std::hash::BuildHasher>(
    before: &HashSet<DiagnosticIdentity, S>,
    after: &[Diagnostic],
) -> Vec<Diagnostic> {
    let mut seen = HashSet::new();
    after
        .iter()
        .filter(|diagnostic| {
            let identity = DiagnosticIdentity::from_diagnostic(diagnostic);
            !before.contains(&identity) && seen.insert(identity)
        })
        .cloned()
        .collect()
}

/// Group diagnostics for output as `severity -> name_path -> [diagnostic]`
/// (Serena's `GroupedDiagnostics`, keyed without the outer file since callers
/// pass one file's worth at a time).
#[must_use]
pub fn group_by_severity_and_symbol(
    diagnostics: &[Diagnostic],
    symbol_of: impl Fn(&Diagnostic) -> String,
) -> Vec<(String, String, Diagnostic)> {
    let mut out = Vec::new();
    for diagnostic in diagnostics {
        let severity = severity_name(severity_code(diagnostic.severity)).to_string();
        let name_path = symbol_of(diagnostic);
        out.push((severity, name_path, diagnostic.clone()));
    }
    out
}

/// A diagnostic bound to the symbol that owns it (L9, Serena's
/// `find_diagnostic_owner_symbol`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwnedDiagnostic {
    /// The owning symbol's `Class/method` name path, or `<file>` when the
    /// diagnostic sits outside every symbol (top-level / prelude text).
    pub name_path: String,
    /// The severity name (`Error`/`Warning`/...).
    pub severity: String,
    /// The protocol severity code (1=Error..4=Hint), when the server set one.
    pub severity_code: Option<i32>,
    /// The diagnostic's 0-based start line.
    pub line: u32,
    /// The diagnostic's 0-based start character.
    pub character: u32,
    /// The diagnostic message.
    pub message: String,
    /// The server-supplied code, when present.
    pub code: Option<String>,
    /// The producing server, when present.
    pub source: Option<String>,
}

/// The bucket name for a diagnostic that owns no symbol (Serena's
/// `FILE_LEVEL_DIAGNOSTIC_BUCKET`).
pub const FILE_LEVEL_BUCKET: &str = "<file>";

/// Diagnostics grouped as `file -> severity -> name_path -> [diagnostic]`
/// (Serena's `GroupedDiagnostics`, L9).
///
/// `symbol_of` maps a diagnostic to its owning symbol's name path — the caller
/// layers that on top of [`crate::symbols::resolve`] /
/// [`crate::symbol_layer`], so this function stays free of any symbol source.
#[must_use]
pub fn group_by_file_severity_symbol(
    diagnostics: &[(String, Diagnostic)],
    symbol_of: impl Fn(&Diagnostic) -> Option<String>,
) -> Vec<(String, String, String, Diagnostic)> {
    let mut out = Vec::new();
    for (file, diagnostic) in diagnostics {
        let severity = severity_name(severity_code(diagnostic.severity)).to_string();
        let name_path = symbol_of(diagnostic).unwrap_or_else(|| FILE_LEVEL_BUCKET.to_string());
        out.push((file.clone(), severity, name_path, diagnostic.clone()));
    }
    out
}

/// Attribute each diagnostic to its owning symbol and lift it to that symbol's
/// name path, dropping the low-level (file-level) detail in favour of the
/// symbol bucket (Serena's `DiagnosticsDiff` symbol attribution).
#[must_use]
pub fn diagnostics_to_symbol(
    diagnostics: &[Diagnostic],
    symbol_of: impl Fn(&Diagnostic) -> Option<String>,
) -> Vec<OwnedDiagnostic> {
    diagnostics
        .iter()
        .map(|diagnostic| OwnedDiagnostic {
            name_path: symbol_of(diagnostic).unwrap_or_else(|| FILE_LEVEL_BUCKET.to_string()),
            severity: severity_name(severity_code(diagnostic.severity)).to_string(),
            severity_code: severity_code(diagnostic.severity),
            line: diagnostic.range.start.line,
            character: diagnostic.range.start.character,
            message: diagnostic.message.clone(),
            code: diagnostic.code.as_ref().map(code_to_string),
            source: diagnostic.source.clone(),
        })
        .collect()
}

/// Roll a set of owned diagnostics up to `severity -> name_path -> count`,
/// which is the progressive-shortening view (L17) of the full list.
#[must_use]
pub fn count_by_severity_and_symbol(
    owned: &[OwnedDiagnostic],
) -> Vec<(String, String, usize)> {
    let mut order: Vec<(String, String)> = Vec::new();
    let mut counts: HashMap<(String, String), usize> = HashMap::new();
    for diagnostic in owned {
        let key = (diagnostic.severity.clone(), diagnostic.name_path.clone());
        match counts.entry(key.clone()) {
            std::collections::hash_map::Entry::Vacant(entry) => {
                order.push(key);
                entry.insert(1);
            }
            std::collections::hash_map::Entry::Occupied(mut entry) => {
                *entry.get_mut() += 1;
            }
        }
    }
    order
        .into_iter()
        .map(|key| {
            let count = counts.get(&key).copied().unwrap_or(0);
            (key.0, key.1, count)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use lsp_types::{DiagnosticSeverity, Position, Range};

    fn diag(message: &str, line: u32, severity: DiagnosticSeverity) -> Diagnostic {
        Diagnostic {
            range: Range::new(Position::new(line, 0), Position::new(line, 5)),
            severity: Some(severity),
            code: None,
            code_description: None,
            source: Some("test".into()),
            message: message.into(),
            related_information: None,
            tags: None,
            data: None,
        }
    }

    #[test]
    fn ledger_reports_only_fresh_diagnostics() {
        let mut ledger = DiagnosticsLedger::new();
        let first = vec![diag("a", 1, DiagnosticSeverity::ERROR)];
        assert_eq!(ledger.reduce("f.rs", &first).len(), 1);
        // Same diagnostic again: nothing fresh.
        assert_eq!(ledger.reduce("f.rs", &first).len(), 0);
        // A new one appears alongside the old.
        let both = vec![
            diag("a", 1, DiagnosticSeverity::ERROR),
            diag("b", 2, DiagnosticSeverity::WARNING),
        ];
        let fresh = ledger.reduce("f.rs", &both);
        assert_eq!(fresh.len(), 1);
        assert_eq!(fresh[0].message, "b");
    }

    #[test]
    fn disappearing_then_returning_counts_as_new() {
        let mut ledger = DiagnosticsLedger::new();
        let one = vec![diag("a", 1, DiagnosticSeverity::ERROR)];
        assert_eq!(ledger.reduce("f.rs", &one).len(), 1);
        assert_eq!(ledger.reduce("f.rs", &[]).len(), 0);
        // It returns: fresh again because the ledger cleared.
        assert_eq!(ledger.reduce("f.rs", &one).len(), 1);
    }

    #[test]
    fn snapshot_diff_rejects_a_stale_generation() {
        let before = vec![diag("a", 1, DiagnosticSeverity::ERROR)];
        let snap = DiagnosticsSnapshot::capture(&before, 5);
        let after = vec![
            diag("a", 1, DiagnosticSeverity::ERROR),
            diag("b", 2, DiagnosticSeverity::ERROR),
        ];
        // A publish at the same or a newer generation is diffed.
        assert_eq!(snap.diff(&after, 5).len(), 1);
        assert_eq!(snap.diff(&after, 6).len(), 1);
        // An older generation is stale: nothing is reported.
        assert_eq!(snap.diff(&after, 4).len(), 0);
        assert!(!snap.accepts_generation(4));
        assert!(snap.accepts_generation(5));
    }

    #[test]
    fn new_since_only_returns_added_diagnostics() {
        let before = [diag("a", 1, DiagnosticSeverity::ERROR)];
        let snapshot: HashSet<_> = before
            .iter()
            .map(DiagnosticIdentity::from_diagnostic)
            .collect();
        let after = vec![
            diag("a", 1, DiagnosticSeverity::ERROR),
            diag("b", 2, DiagnosticSeverity::ERROR),
        ];
        let added = new_since(&snapshot, &after);
        assert_eq!(added.len(), 1);
        assert_eq!(added[0].message, "b");
    }

    #[test]
    fn severity_codes_match_the_protocol() {
        use lsp_types::DiagnosticSeverity;
        assert_eq!(severity_code(Some(DiagnosticSeverity::ERROR)), Some(1));
        assert_eq!(severity_code(Some(DiagnosticSeverity::HINT)), Some(4));
        assert_eq!(severity_code(None), None);
    }

    #[test]
    fn severity_names_match_the_protocol() {
        assert_eq!(severity_name(Some(1)), "Error");
        assert_eq!(severity_name(Some(4)), "Hint");
        assert_eq!(severity_name(None), "Unknown");
    }

    #[test]
    fn diagnostics_are_attributed_to_their_owning_symbol() {
        let diags = vec![
            diag("in method", 1, DiagnosticSeverity::ERROR),
            diag("outside any symbol", 0, DiagnosticSeverity::WARNING),
        ];
        let owned = diagnostics_to_symbol(&diags, |d| {
            (d.range.start.line == 1).then(|| "MyClass/method".to_string())
        });
        assert_eq!(owned[0].name_path, "MyClass/method");
        assert_eq!(owned[0].severity, "Error");
        assert_eq!(owned[0].severity_code, Some(1));
        assert_eq!(owned[1].name_path, FILE_LEVEL_BUCKET);
        assert_eq!(owned[1].severity, "Warning");
    }

    #[test]
    fn counts_roll_up_severity_and_symbol() {
        let diags = vec![
            diag("a", 1, DiagnosticSeverity::ERROR),
            diag("b", 1, DiagnosticSeverity::ERROR),
            diag("c", 2, DiagnosticSeverity::WARNING),
        ];
        let owned = diagnostics_to_symbol(&diags, |d| {
            Some(if d.range.start.line == 1 {
                "Sym".to_string()
            } else {
                "Other".to_string()
            })
        });
        let counts = count_by_severity_and_symbol(&owned);
        assert_eq!(
            counts,
            vec![
                ("Error".to_string(), "Sym".to_string(), 2),
                ("Warning".to_string(), "Other".to_string(), 1),
            ]
        );
    }

    #[test]
    fn group_by_file_severity_symbol_keeps_file_and_buckets() {
        let rows = vec![
            ("a.rs".to_string(), diag("x", 1, DiagnosticSeverity::ERROR)),
            ("a.rs".to_string(), diag("y", 0, DiagnosticSeverity::ERROR)),
        ];
        let grouped = group_by_file_severity_symbol(&rows, |d| {
            (d.range.start.line == 1).then(|| "Sym".to_string())
        });
        assert_eq!(grouped[0].0, "a.rs");
        assert_eq!(grouped[0].1, "Error");
        assert_eq!(grouped[0].2, "Sym");
        assert_eq!(grouped[1].2, FILE_LEVEL_BUCKET);
    }
}
