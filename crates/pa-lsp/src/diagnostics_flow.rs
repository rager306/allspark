//! Diagnostics glue (E6 / L12 / L13 / L20): the delta a *before/after* edit
//! produces, the freshness gate that drops stale or orphaned publishes, and the
//! workspace-wide checker runner.
//!
//! FORK-ONLY (allspark). Synthesizes:
//! - Serena's `DiagnosticsDiff` (snapshot before, diff after, only the new
//!   identities survive) — E6, and the LSP half of the edit pipeline.
//! - oh-my-pi's document-version freshness + `filterOrphanProjectDiagnostics`
//!   — L13.
//! - oh-my-pi's `workspace-diagnostics.ts`: detect the project language, run at
//!   most [`MAX_CONCURRENT_CHECKERS`] checkers, aggregate their output — L20.
//!
//! The delta, the freshness gate, and the language/checker *selection* are all
//! pure so they unit-test with no server or compiler; only
//! [`run_checkers`] spawns processes.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

use lsp_types::Diagnostic;

use crate::ledger::{self, DiagnosticIdentity};

/// E6: diagnostics present in `after` that were absent from the `before`
/// snapshot (Serena's `DiagnosticsDiff`, minus the symbol attribution which the
/// caller layers on with [`crate::symbols`]).
///
/// This is the edit-safe primitive: an edit snapshots identities *before* the
/// write and diffs *after*, so only the diagnostics the edit introduced are
/// surfaced. Diagnostics that merely moved count as new (range is part of the
/// identity — see [`crate::ledger`]).
#[must_use]
pub fn diagnostics_delta<S: std::hash::BuildHasher>(
    before: &HashSet<DiagnosticIdentity, S>,
    after: &[Diagnostic],
) -> Vec<Diagnostic> {
    ledger::new_since(before, after)
}

/// E6: snapshot the identities of a diagnostic set, for a later delta.
#[must_use]
pub fn snapshot(diagnostics: &[Diagnostic]) -> HashSet<DiagnosticIdentity> {
    diagnostics
        .iter()
        .map(DiagnosticIdentity::from_diagnostic)
        .collect()
}

/// A before/after diagnostics diff bound to the owning symbols (E6 + L9).
///
/// The `symbol_of` mapper is supplied by the caller (over
/// [`crate::symbols::resolve`] / [`crate::symbol_layer`]), keeping this module
/// free of any symbol source.
#[must_use]
pub fn delta_to_symbols<S: std::hash::BuildHasher>(
    before: &HashSet<DiagnosticIdentity, S>,
    after: &[Diagnostic],
    symbol_of: impl Fn(&Diagnostic) -> Option<String>,
) -> Vec<ledger::OwnedDiagnostic> {
    ledger::diagnostics_to_symbol(&diagnostics_delta(before, after), symbol_of)
}

/// The default for E6: post-edit diagnostics are **off** unless a host opts in.
///
/// Serena ships `ENABLE_DIAGNOSTICS_DEFAULT = False` (`lsp_diagnostics.py:211`)
/// because a single edit often *intentionally* breaks compilation (a signature
/// change fixed by the next edit), so surfacing the fresh errors on every write
/// is noise. The fork keeps the same posture: opt in per host, never impose.
pub const DIAGNOSTICS_AFTER_EDIT_DEFAULT: bool = false;

/// E6 orchestrator: capture a diagnostics snapshot before an edit, then diff it
/// against the diagnostics after the write, keeping only the new identities.
///
/// Mirrors Serena's `EditingToolWithDiagnostics.diagnostics_context`
/// (`tools_base.py:433`): a host takes a [`begin`](Self::begin) snapshot, runs
/// the edit, then calls [`diff`](Self::diff) with the post-edit diagnostics. The
/// whole thing is a no-op when [`enabled`](Self::enabled) is false, so a host
/// can wire it in unconditionally without changing behaviour until it opts in.
#[derive(Debug, Clone)]
pub struct DiagnosticsAfterEdit {
    enabled: bool,
    before: Option<HashSet<DiagnosticIdentity>>,
}

impl DiagnosticsAfterEdit {
    /// A context that is **off** by default (Serena parity).
    #[must_use]
    pub fn new() -> Self {
        Self {
            enabled: DIAGNOSTICS_AFTER_EDIT_DEFAULT,
            before: None,
        }
    }

    /// A context with an explicit enabled flag (the host's opt-in).
    #[must_use]
    pub fn with_enabled(enabled: bool) -> Self {
        Self {
            enabled,
            before: None,
        }
    }

    /// Whether post-edit diagnostics are active.
    #[must_use]
    pub fn enabled(&self) -> bool {
        self.enabled
    }

    /// Snapshot the pre-edit diagnostics. A no-op when disabled.
    pub fn begin(&mut self, diagnostics: &[Diagnostic]) {
        if self.enabled {
            self.before = Some(snapshot(diagnostics));
        }
    }

    /// Diff the post-edit diagnostics against the pre-edit snapshot, returning
    /// only the new ones. Returns nothing when disabled or when
    /// [`begin`](Self::begin) was never called.
    #[must_use]
    pub fn diff(&self, after: &[Diagnostic]) -> Vec<Diagnostic> {
        if !self.enabled {
            return Vec::new();
        }
        match &self.before {
            Some(before) => diagnostics_delta(before, after),
            None => Vec::new(),
        }
    }

    /// The pre-edit snapshot, when one was taken.
    #[must_use]
    pub fn before(&self) -> Option<&HashSet<DiagnosticIdentity>> {
        self.before.as_ref()
    }
}

impl Default for DiagnosticsAfterEdit {
    fn default() -> Self {
        Self::new()
    }
}

// ───────────────────────────── L13 freshness ─────────────────────────────

/// The TypeScript diagnostic codes oh-my-pi treats as *orphan* when they appear
/// outside a project root (a file pulled into a server session that has no
/// `tsconfig.json` ancestor): cannot-find-module and friends.
pub const TS_ORPHAN_CODES: &[&str] = &["1375", "1378", "2307", "2580", "2591", "2792", "2867"];

/// L13: the document-version gate for push diagnostics.
///
/// A `publishDiagnostics` that carries a version older than the version the
/// client last sent is stale — the server computed it against a document the
/// client has already replaced. `None` (the server did not version the publish)
/// is accepted, since many servers omit the field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FreshnessGate {
    doc_version: i32,
}

impl FreshnessGate {
    /// A gate for a document currently at `doc_version`.
    #[must_use]
    pub fn new(doc_version: i32) -> Self {
        Self { doc_version }
    }

    /// Whether a publish at `published_version` is fresh for this document.
    ///
    /// `None` is accepted; a version below the current document version is
    /// rejected as stale.
    #[must_use]
    pub fn accepts(&self, published_version: Option<i32>) -> bool {
        match published_version {
            None => true,
            Some(version) => version >= self.doc_version,
        }
    }

    /// Keep only the diagnostics whose publish version is fresh.
    ///
    /// Convenience for the pull path, which knows the document version but not
    /// the publish version: a report for an older version is dropped whole.
    #[must_use]
    pub fn filter_report<T>(
        &self,
        published_version: Option<i32>,
        report: T,
    ) -> Option<T> {
        self.accepts(published_version).then_some(report)
    }
}

/// L13: whether `diagnostic` is an *orphan* project diagnostic — one of the
/// [`TS_ORPHAN_CODES`] whose file sits outside every `markers` ancestor under
/// `root`.
///
/// A TypeScript server can pull a stray file into a session with no
/// `tsconfig.json`/`package.json` above it, then report "cannot find module"
/// for every import. Those are noise; the real project's diagnostics come from
/// a file that *does* sit under a root marker.
#[must_use]
pub fn is_orphan_project_diagnostic(
    diagnostic: &Diagnostic,
    file: &Path,
    root: &Path,
    markers: &[&str],
) -> bool {
    let code = diagnostic.code.as_ref().map(|code| match code {
        lsp_types::NumberOrString::Number(n) => n.to_string(),
        lsp_types::NumberOrString::String(s) => s.clone(),
    });
    let Some(code) = code else {
        return false;
    };
    if !TS_ORPHAN_CODES.contains(&code.as_str()) {
        return false;
    }
    !has_marker_ancestor(file, root, markers)
}

/// Whether `file`'s directory (or any ancestor up to and including `root`) holds
/// one of `markers`.
#[must_use]
pub fn has_marker_ancestor(file: &Path, root: &Path, markers: &[&str]) -> bool {
    let start = file.parent().unwrap_or(file);
    for dir in start.ancestors() {
        if markers.iter().any(|marker| dir.join(marker).exists()) {
            return true;
        }
        if dir == root {
            break;
        }
    }
    false
}

/// L13: drop orphan project diagnostics from a set, given the workspace root and
/// the TypeScript root markers.
#[must_use]
pub fn filter_orphan_project_diagnostics(
    diagnostics: &[Diagnostic],
    file: &Path,
    root: &Path,
    markers: &[&str],
) -> Vec<Diagnostic> {
    diagnostics
        .iter()
        .filter(|diagnostic| !is_orphan_project_diagnostic(diagnostic, file, root, markers))
        .cloned()
        .collect()
}

/// The default TypeScript/JavaScript root markers.
pub const TS_ROOT_MARKERS: &[&str] = &["tsconfig.json", "jsconfig.json", "package.json"];

// ─────────────────────────── L20 workspace diagnostics ───────────────────

/// At most this many checkers run for a workspace request (oh-my-pi's
/// `MAX_CONCURRENT_CHECKERS`).
pub const MAX_CONCURRENT_CHECKERS: usize = 2;

/// Default per-checker timeout.
pub const CHECKER_TIMEOUT: Duration = Duration::from_secs(120);

/// A language the workspace detector recognizes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WorkspaceLanguage {
    /// Rust (`Cargo.toml`).
    Rust,
    /// TypeScript/JavaScript (`tsconfig.json` / `package.json`).
    TypeScript,
    /// Go (`go.mod` / `go.work`).
    Go,
    /// Python (`pyproject.toml` / `setup.py` / `requirements.txt`).
    Python,
}

impl WorkspaceLanguage {
    /// The detection markers, nearest wins.
    #[must_use]
    pub fn markers(self) -> &'static [&'static str] {
        match self {
            WorkspaceLanguage::Rust => &["Cargo.toml"],
            WorkspaceLanguage::TypeScript => &["tsconfig.json", "package.json"],
            WorkspaceLanguage::Go => &["go.mod", "go.work"],
            WorkspaceLanguage::Python => &["pyproject.toml", "setup.py", "requirements.txt"],
        }
    }

    /// The human name.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            WorkspaceLanguage::Rust => "rust",
            WorkspaceLanguage::TypeScript => "typescript",
            WorkspaceLanguage::Go => "go",
            WorkspaceLanguage::Python => "python",
        }
    }
}

/// A checker: a program + args that emit diagnostics for a language.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckerSpec {
    /// The language this checker covers.
    pub language: WorkspaceLanguage,
    /// The program to run (resolved on `PATH`).
    pub program: String,
    /// The argv.
    pub args: Vec<String>,
    /// A short label for output.
    pub name: String,
}

/// The languages whose markers are present directly under `root`.
///
/// Detection is order-stable (Rust, TypeScript, Go, Python) so the selection is
/// deterministic.
#[must_use]
pub fn detect_languages(root: &Path) -> Vec<WorkspaceLanguage> {
    let mut found = Vec::new();
    for language in [
        WorkspaceLanguage::Rust,
        WorkspaceLanguage::TypeScript,
        WorkspaceLanguage::Go,
        WorkspaceLanguage::Python,
    ] {
        if language
            .markers()
            .iter()
            .any(|marker| root.join(marker).exists())
        {
            found.push(language);
        }
    }
    found
}

/// The checker for a language, if one is known.
#[must_use]
pub fn checker_for(language: WorkspaceLanguage) -> CheckerSpec {
    match language {
        WorkspaceLanguage::Rust => CheckerSpec {
            language,
            program: "cargo".to_string(),
            args: vec!["check".to_string(), "--message-format=short".to_string()],
            name: "cargo check".to_string(),
        },
        WorkspaceLanguage::TypeScript => CheckerSpec {
            language,
            program: "tsc".to_string(),
            args: vec!["--noEmit".to_string()],
            name: "tsc".to_string(),
        },
        WorkspaceLanguage::Go => CheckerSpec {
            language,
            program: "go".to_string(),
            args: vec!["build".to_string(), "./...".to_string()],
            name: "go build".to_string(),
        },
        WorkspaceLanguage::Python => CheckerSpec {
            language,
            program: "pyright".to_string(),
            args: vec!["--outputjson".to_string()],
            name: "pyright".to_string(),
        },
    }
}

/// Select up to [`MAX_CONCURRENT_CHECKERS`] checkers for `root`.
///
/// Detection runs first; only checkers whose binary is present (per `available`)
/// are returned. `available` is injected so the selection is testable without a
/// real `PATH`.
#[must_use]
pub fn select_checkers(
    root: &Path,
    available: impl Fn(&str) -> bool,
) -> Vec<CheckerSpec> {
    let mut specs = Vec::new();
    for language in detect_languages(root) {
        if specs.len() >= MAX_CONCURRENT_CHECKERS {
            break;
        }
        let spec = checker_for(language);
        if available(&spec.program) {
            specs.push(spec);
        }
    }
    specs
}

/// Whether `program` resolves on `PATH` (L20 binary detection).
#[must_use]
pub fn binary_on_path(program: &str) -> bool {
    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&path).any(|dir| {
        let candidate = dir.join(program);
        candidate.is_file() && is_executable(&candidate)
    })
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).is_ok_and(|meta| meta.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
fn is_executable(path: &Path) -> bool {
    path.is_file()
}

/// One aggregated diagnostic from a workspace checker.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct WorkspaceDiagnostic {
    /// The file the diagnostic is in (absolute when the checker reported it so).
    pub file: PathBuf,
    /// 1-based line.
    pub line: u32,
    /// 1-based column.
    pub column: u32,
    /// `error` / `warning` / `info`.
    pub severity: String,
    /// The message text.
    pub message: String,
    /// The checker code, when the tool emitted one.
    pub code: Option<String>,
    /// Which checker produced it.
    pub checker: String,
}

/// The result of a workspace diagnostics run.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct WorkspaceReport {
    /// Every aggregated diagnostic.
    pub diagnostics: Vec<WorkspaceDiagnostic>,
    /// The checkers that ran (by name).
    pub checkers: Vec<String>,
    /// Checkers that were skipped because their binary was missing.
    pub skipped: Vec<String>,
}

impl WorkspaceReport {
    /// Diagnostics per severity, for a compact summary line.
    #[must_use]
    pub fn count_by_severity(&self) -> Vec<(String, usize)> {
        let mut order: Vec<String> = Vec::new();
        let mut counts: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
        for diagnostic in &self.diagnostics {
            let key = diagnostic.severity.clone();
            if !counts.contains_key(&key) {
                order.push(key.clone());
            }
            *counts.entry(key).or_insert(0) += 1;
        }
        order
            .into_iter()
            .map(|severity| {
                let count = counts.get(&severity).copied().unwrap_or(0);
                (severity, count)
            })
            .collect()
    }
}

/// Run the workspace checkers for `root`, aggregating their diagnostics.
///
/// Up to [`MAX_CONCURRENT_CHECKERS`] checkers run concurrently. A checker that
/// cannot be spawned or times out contributes nothing (its name is still
/// recorded), so one broken tool never fails the whole request.
pub async fn run_checkers(
    root: &Path,
    specs: &[CheckerSpec],
    timeout: Duration,
) -> WorkspaceReport {
    let mut report = WorkspaceReport::default();
    let mut tasks = Vec::new();
    for spec in specs.iter().take(MAX_CONCURRENT_CHECKERS) {
        report.checkers.push(spec.name.clone());
        tasks.push(run_one(root, spec.clone(), timeout));
    }
    let results = futures::future::join_all(tasks).await;
    for diagnostics in results.into_iter().flatten() {
        report.diagnostics.extend(diagnostics);
    }
    report
}

/// Run one checker and parse its output; `None` when it cannot be spawned.
async fn run_one(
    root: &Path,
    spec: CheckerSpec,
    timeout: Duration,
) -> Option<Vec<WorkspaceDiagnostic>> {
    let output = tokio::time::timeout(
        timeout,
        tokio::process::Command::new(&spec.program)
            .args(&spec.args)
            .current_dir(root)
            .output(),
    )
    .await
    .ok()?
    .ok()?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    Some(parse_checker_output(&spec, &stdout, &stderr, root))
}

/// Parse a checker's combined output into aggregated diagnostics.
///
/// Rust (`cargo check --message-format=short`), Go (`go build`), and TypeScript
/// (`tsc`) all emit `path:line:col: severity...` or `path(line,col): ...`
/// lines; pyright emits JSON. The parser is chosen by the checker program.
#[must_use]
pub fn parse_checker_output(
    spec: &CheckerSpec,
    stdout: &str,
    stderr: &str,
    root: &Path,
) -> Vec<WorkspaceDiagnostic> {
    match spec.program.as_str() {
        "pyright" => parse_pyright_json(stdout, &spec.name, root),
        // tsc writes its `path(line,col): ...` lines to stdout; some wrappers
        // relay them to stderr, so both streams are scanned.
        "tsc" => {
            let mut diagnostics = parse_tsc(stdout, &spec.name, root);
            diagnostics.extend(parse_tsc(stderr, &spec.name, root));
            diagnostics
        }
        // cargo (stderr) and go (stderr) share the `path:line:col: msg` shape;
        // a wrapper may relay to stdout, so both streams are scanned.
        _ => {
            let mut diagnostics = parse_colon_lines(stderr, &spec.name, root);
            diagnostics.extend(parse_colon_lines(stdout, &spec.name, root));
            diagnostics
        }
    }
}

/// Parse `path:line:col: message` lines (cargo short, go build).
#[must_use]
pub fn parse_colon_lines(output: &str, checker: &str, root: &Path) -> Vec<WorkspaceDiagnostic> {
    let mut out = Vec::new();
    for line in output.lines() {
        if let Some(diagnostic) = parse_colon_line(line, checker, root) {
            out.push(diagnostic);
        }
    }
    out
}

fn parse_colon_line(line: &str, checker: &str, root: &Path) -> Option<WorkspaceDiagnostic> {
    // `src/main.rs:3:5: error: message` or `src/main.go:3:5: message`.
    let mut parts = line.splitn(4, ':');
    let file = parts.next()?.trim();
    if file.is_empty() || file.contains(' ') && !file.contains('/') && !file.contains('.') {
        return None;
    }
    let line_no: u32 = parts.next()?.trim().parse().ok()?;
    let col_no: u32 = parts.next()?.trim().parse().ok()?;
    let rest = parts.next()?.trim();
    let (severity, message) = split_severity(rest);
    Some(WorkspaceDiagnostic {
        file: absolutize(root, file),
        line: line_no,
        column: col_no,
        severity,
        message,
        code: None,
        checker: checker.to_string(),
    })
}

/// Parse `path(line,col): error TSxxxx: message` lines (tsc).
#[must_use]
pub fn parse_tsc(output: &str, checker: &str, root: &Path) -> Vec<WorkspaceDiagnostic> {
    let mut out = Vec::new();
    for line in output.lines() {
        if let Some(diagnostic) = parse_tsc_line(line, checker, root) {
            out.push(diagnostic);
        }
    }
    out
}

fn parse_tsc_line(line: &str, checker: &str, root: &Path) -> Option<WorkspaceDiagnostic> {
    // `src/a.ts(3,5): error TS2322: Type 'x' is not assignable to 'y'.`
    let open = line.find('(')?;
    let close = line[open..].find(')')? + open;
    let file = &line[..open];
    let coords = &line[open + 1..close];
    let mut coord = coords.split(',');
    let line_no: u32 = coord.next()?.trim().parse().ok()?;
    let col_no: u32 = coord.next()?.trim().parse().ok()?;
    let rest = line[close + 1..].trim_start_matches([':', ' ']);
    let (severity, message) = split_severity(rest);
    let code = extract_ts_code(&message);
    Some(WorkspaceDiagnostic {
        file: absolutize(root, file),
        line: line_no,
        column: col_no,
        severity,
        message,
        code,
        checker: checker.to_string(),
    })
}

/// Extract `TSxxxx` from a tsc message.
fn extract_ts_code(message: &str) -> Option<String> {
    let start = message.find("TS")?;
    let rest = &message[start + 2..];
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    if digits.is_empty() {
        None
    } else {
        Some(format!("TS{digits}"))
    }
}

/// Parse pyright's `--outputjson` diagnostics.
#[must_use]
pub fn parse_pyright_json(output: &str, checker: &str, root: &Path) -> Vec<WorkspaceDiagnostic> {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(output) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let Some(diagnostics) = value.get("generalDiagnostics").and_then(|v| v.as_array()) else {
        return out;
    };
    for diagnostic in diagnostics {
        let Some(range) = diagnostic.get("range") else {
            continue;
        };
        let start = range.get("start").cloned().unwrap_or(serde_json::Value::Null);
        let file = diagnostic
            .get("file")
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        let severity = diagnostic
            .get("severity")
            .and_then(|v| v.as_str())
            .unwrap_or("error");
        let message = diagnostic
            .get("message")
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        let rule = diagnostic.get("rule").and_then(|v| v.as_str());
        // pyright ranges are 0-based; lift to 1-based, clamping a runaway
        // value rather than truncating it.
        let line = u32::try_from(
            start.get("line").and_then(serde_json::Value::as_u64).unwrap_or(0),
        )
        .unwrap_or(u32::MAX)
        .saturating_add(1);
        let column = u32::try_from(
            start
                .get("character")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0),
        )
        .unwrap_or(u32::MAX)
        .saturating_add(1);
        out.push(WorkspaceDiagnostic {
            file: absolutize(root, file),
            line,
            column,
            severity: severity.to_string(),
            message: message.to_string(),
            code: rule.map(str::to_string),
            checker: checker.to_string(),
        });
    }
    out
}

/// Split a leading `error:`/`warning:`/`info:`/`note:` from a message.
fn split_severity(rest: &str) -> (String, String) {
    for (prefix, severity) in [
        ("error", "error"),
        ("warning", "warning"),
        ("info", "info"),
        ("note", "note"),
    ] {
        if let Some(tail) = rest.strip_prefix(prefix) {
            let tail = tail.trim_start_matches([':', ' ']).trim();
            return (severity.to_string(), tail.to_string());
        }
    }
    ("error".to_string(), rest.to_string())
}

/// Resolve a checker-reported path against `root` when it is relative.
fn absolutize(root: &Path, file: &str) -> PathBuf {
    let path = Path::new(file);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lsp_types::{DiagnosticSeverity, NumberOrString, Position, Range};

    fn diag(message: &str, line: u32, code: Option<&str>) -> Diagnostic {
        Diagnostic {
            range: Range::new(Position::new(line, 0), Position::new(line, 5)),
            severity: Some(DiagnosticSeverity::ERROR),
            code: code.map(|c| NumberOrString::String(c.to_string())),
            code_description: None,
            source: Some("test".into()),
            message: message.into(),
            related_information: None,
            tags: None,
            data: None,
        }
    }

    #[test]
    fn delta_returns_only_new_diagnostics() {
        let before = vec![diag("a", 1, None)];
        let snapshot = snapshot(&before);
        let after = vec![
            diag("a", 1, None),
            diag("b", 2, None),
            diag("b", 2, None), // duplicate in `after` collapses
        ];
        let delta = diagnostics_delta(&snapshot, &after);
        assert_eq!(delta.len(), 1);
        assert_eq!(delta[0].message, "b");
    }

    #[test]
    fn after_edit_context_is_off_by_default() {
        let mut ctx = DiagnosticsAfterEdit::new();
        assert!(!ctx.enabled(), "Serena parity: opt-in only");
        ctx.begin(&[diag("pre-existing", 1, None)]);
        // Disabled: the diff is empty even though a new diagnostic appeared.
        let after = vec![diag("pre-existing", 1, None), diag("new", 2, None)];
        assert_eq!(ctx.diff(&after).len(), 0);
        assert!(ctx.before().is_none(), "a disabled context never snapshots");
    }

    #[test]
    fn after_edit_context_surfaces_only_new_diagnostics() {
        let mut ctx = DiagnosticsAfterEdit::with_enabled(true);
        ctx.begin(&[diag("pre-existing", 1, None)]);
        assert!(ctx.before().is_some());
        let after = vec![diag("pre-existing", 1, None), diag("introduced", 2, None)];
        let fresh = ctx.diff(&after);
        assert_eq!(fresh.len(), 1);
        assert_eq!(fresh[0].message, "introduced");
    }

    #[test]
    fn after_edit_context_without_begin_yields_nothing() {
        let ctx = DiagnosticsAfterEdit::with_enabled(true);
        assert_eq!(ctx.diff(&[diag("x", 1, None)]).len(), 0);
    }

    #[test]
    fn delta_is_empty_when_nothing_changed() {
        let before = snapshot(&[diag("a", 1, None)]);
        let after = vec![diag("a", 1, None)];
        assert_eq!(diagnostics_delta(&before, &after).len(), 0);
    }

    #[test]
    fn freshness_gate_rejects_a_stale_version() {
        let gate = FreshnessGate::new(5);
        assert!(gate.accepts(None), "unversioned publishes are accepted");
        assert!(gate.accepts(Some(5)));
        assert!(gate.accepts(Some(6)));
        assert!(!gate.accepts(Some(4)), "an older document version is stale");
    }

    #[test]
    fn freshness_filter_drops_a_stale_report() {
        let gate = FreshnessGate::new(3);
        assert_eq!(gate.filter_report(Some(2), "report"), None);
        assert_eq!(gate.filter_report(Some(3), "report"), Some("report"));
    }

    #[test]
    fn orphan_project_diagnostics_are_dropped_outside_a_marker() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::write(root.join("tsconfig.json"), "{}").unwrap();
        // A file under the root: has a marker ancestor.
        let inside = root.join("src/app.ts");
        std::fs::create_dir_all(inside.parent().unwrap()).unwrap();
        std::fs::write(&inside, "").unwrap();
        // A stray file outside the project.
        let stray_dir = tempfile::tempdir().unwrap();
        let stray = stray_dir.path().join("stray.ts");
        std::fs::write(&stray, "").unwrap();

        let orphan = diag("Cannot find module 'x'", 1, Some("2307"));
        let normal = diag("real error", 1, Some("1000"));

        assert!(!is_orphan_project_diagnostic(&orphan, &inside, root, TS_ROOT_MARKERS));
        assert!(is_orphan_project_diagnostic(&orphan, &stray, root, TS_ROOT_MARKERS));
        // A non-orphan code is never dropped.
        assert!(!is_orphan_project_diagnostic(&normal, &stray, root, TS_ROOT_MARKERS));

        let kept = filter_orphan_project_diagnostics(
            &[orphan, normal],
            &stray,
            root,
            TS_ROOT_MARKERS,
        );
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].message, "real error");
    }

    #[test]
    fn language_detection_is_order_stable() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::write(root.join("Cargo.toml"), "").unwrap();
        std::fs::write(root.join("go.mod"), "").unwrap();
        assert_eq!(
            detect_languages(root),
            vec![WorkspaceLanguage::Rust, WorkspaceLanguage::Go]
        );
    }

    #[test]
    fn checker_selection_caps_at_two_and_skips_missing_binaries() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::write(root.join("Cargo.toml"), "").unwrap();
        std::fs::write(root.join("go.mod"), "").unwrap();
        std::fs::write(root.join("pyproject.toml"), "").unwrap();
        std::fs::write(root.join("package.json"), "").unwrap();
        // Every binary present: capped at two, first two languages win.
        let all = select_checkers(root, |_| true);
        assert_eq!(all.len(), MAX_CONCURRENT_CHECKERS);
        assert_eq!(all[0].language, WorkspaceLanguage::Rust);
        assert_eq!(all[1].language, WorkspaceLanguage::TypeScript);
        // Only go's binary present: rust/ts skipped, go selected.
        let only_go = select_checkers(root, |program| program == "go");
        assert_eq!(only_go.len(), 1);
        assert_eq!(only_go[0].language, WorkspaceLanguage::Go);
    }

    #[test]
    fn parses_cargo_short_output() {
        let out = "src/main.rs:3:5: error: cannot find value `x`\nsrc/lib.rs:10:1: warning: unused";
        let parsed = parse_colon_lines(out, "cargo check", Path::new("/proj"));
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].file, PathBuf::from("/proj/src/main.rs"));
        assert_eq!(parsed[0].line, 3);
        assert_eq!(parsed[0].column, 5);
        assert_eq!(parsed[0].severity, "error");
        assert!(parsed[0].message.contains("cannot find value"));
        assert_eq!(parsed[1].severity, "warning");
    }

    #[test]
    fn parses_tsc_output() {
        let out = "src/a.ts(3,5): error TS2322: Type 'number' is not assignable to 'string'.";
        let parsed = parse_tsc(out, "tsc", Path::new("/proj"));
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].file, PathBuf::from("/proj/src/a.ts"));
        assert_eq!(parsed[0].line, 3);
        assert_eq!(parsed[0].column, 5);
        assert_eq!(parsed[0].severity, "error");
        assert_eq!(parsed[0].code.as_deref(), Some("TS2322"));
    }

    #[test]
    fn parses_pyright_json() {
        let json = r#"{
            "generalDiagnostics": [
                {
                    "file": "/proj/a.py",
                    "severity": "error",
                    "message": "x is not defined",
                    "range": {"start": {"line": 4, "character": 2}},
                    "rule": "reportUndefinedVariable"
                }
            ]
        }"#;
        let parsed = parse_pyright_json(json, "pyright", Path::new("/proj"));
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].line, 5, "pyright 0-based lines are lifted to 1-based");
        assert_eq!(parsed[0].column, 3);
        assert_eq!(parsed[0].code.as_deref(), Some("reportUndefinedVariable"));
    }

    #[test]
    fn malformed_checker_output_is_ignored() {
        assert_eq!(
            parse_colon_lines("not a diagnostic", "cargo check", Path::new("/p")).len(),
            0
        );
        assert_eq!(parse_tsc("garbage", "tsc", Path::new("/p")).len(), 0);
        assert_eq!(parse_pyright_json("{ not json", "pyright", Path::new("/p")).len(), 0);
    }

    #[tokio::test]
    async fn run_checkers_aggregates_a_fake_checker() {
        // A hermetic checker: `/bin/sh` printing two `path:line:col: msg` rows.
        let spec = CheckerSpec {
            language: WorkspaceLanguage::Rust,
            program: "sh".to_string(),
            args: vec![
                "-c".to_string(),
                "printf 'src/a.rs:1:2: error: boom\\nsrc/b.rs:3:4: warning: eh\\n'".to_string(),
            ],
            name: "fake".to_string(),
        };
        let root = tempfile::tempdir().unwrap();
        let report = run_checkers(root.path(), std::slice::from_ref(&spec), CHECKER_TIMEOUT).await;
        assert_eq!(report.checkers, vec!["fake".to_string()]);
        assert_eq!(report.diagnostics.len(), 2);
        assert_eq!(report.diagnostics[0].severity, "error");
        assert!(report.diagnostics[0].file.ends_with("src/a.rs"));
        assert_eq!(report.diagnostics[1].severity, "warning");
    }

    #[tokio::test]
    async fn run_checkers_survives_a_missing_binary() {
        let spec = CheckerSpec {
            language: WorkspaceLanguage::Rust,
            program: "definitely-not-a-real-binary-xyz".to_string(),
            args: Vec::new(),
            name: "ghost".to_string(),
        };
        let root = tempfile::tempdir().unwrap();
        let report = run_checkers(root.path(), std::slice::from_ref(&spec), CHECKER_TIMEOUT).await;
        // The checker is recorded but contributes nothing.
        assert_eq!(report.checkers, vec!["ghost".to_string()]);
        assert_eq!(report.diagnostics.len(), 0);
    }

    #[test]
    fn report_counts_by_severity() {
        let report = WorkspaceReport {
            diagnostics: vec![
                WorkspaceDiagnostic {
                    file: PathBuf::from("a"),
                    line: 1,
                    column: 1,
                    severity: "error".into(),
                    message: "x".into(),
                    code: None,
                    checker: "cargo check".into(),
                },
                WorkspaceDiagnostic {
                    file: PathBuf::from("b"),
                    line: 1,
                    column: 1,
                    severity: "error".into(),
                    message: "y".into(),
                    code: None,
                    checker: "cargo check".into(),
                },
                WorkspaceDiagnostic {
                    file: PathBuf::from("c"),
                    line: 1,
                    column: 1,
                    severity: "warning".into(),
                    message: "z".into(),
                    code: None,
                    checker: "cargo check".into(),
                },
            ],
            ..WorkspaceReport::default()
        };
        assert_eq!(
            report.count_by_severity(),
            vec![("error".to_string(), 2), ("warning".to_string(), 1)]
        );
    }
}
