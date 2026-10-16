//! O10: bash output minimizer.
//!
//! Typed commands (`cargo`, `go`, `git`, `docker`, ...) emit mostly noise; a
//! filter table collapses the output to the interesting lines (errors,
//! warnings, summaries) and leaves the full text addressable as
//! `artifact://...`. This is the port of oh-my-pi's 25-filter minimizer
//! (pi-shell), reduced to the filter table and the reduction rules — no shell.

use std::collections::BTreeSet;

/// Filters keep lines carrying one of these markers.
const KEEP_MARKERS: &[&str] = &[
    "error",
    "Error",
    "ERROR",
    "warning",
    "Warning",
    "WARNING",
    "fail",
    "FAIL",
    "Fail",
    "panic",
    "PANIC",
    "fatal",
    "FATAL",
    "cannot",
    "Cannot",
    "undefined",
    "not found",
    "No such file",
    "abort",
    "ABORT",
    "E0",
    "TS0",
    "expected",
    "-->",
];

/// Lines that are pure noise for every filter.
const DROP_MARKERS: &[&str] = &[
    "Compiling ",
    "Finished ",
    "Running ",
    "Blocking waiting for file lock",
    "Downloading ",
    "Downloaded ",
    "Updating ",
    "Adding ",
    "note: build finished",
];

/// How many trailing context lines a reduction keeps after the last kept line.
const TAIL_CONTEXT_LINES: usize = 2;

/// The bash-output minimizer.
#[derive(Debug, Clone)]
pub struct BashMinimizer {
    /// Disabled passes output through untouched.
    pub enabled: bool,
    /// Hard cap on kept lines before eliding the middle.
    pub max_lines: usize,
    /// The filter table.
    pub filters: Vec<Filter>,
}

impl Default for BashMinimizer {
    fn default() -> Self {
        Self {
            enabled: true,
            max_lines: 120,
            filters: builtin_filters(),
        }
    }
}

/// The result of a minimization pass.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MinimizeResult {
    /// The reduced output.
    pub output: String,
    /// The `artifact://` reference to the full original, when a filter fired.
    pub original_ref: Option<String>,
    /// The name of the filter that fired.
    pub filter: Option<String>,
    /// Whether the output was actually reduced.
    pub reduced: bool,
}

/// One command filter: a command prefix and a reduction.
#[derive(Debug, Clone)]
pub struct Filter {
    /// Filter name (`cargo`, `go`, `git`, `docker`, ...).
    pub name: &'static str,
    /// The command prefix tokens that select this filter (matched on the
    /// command's leading tokens).
    pub command_prefix: &'static [&'static str],
}

impl BashMinimizer {
    /// The minimizer with the built-in filter table.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Find the filter whose command prefix matches `command`.
    #[must_use]
    pub fn select_filter(&self, command: &str) -> Option<&Filter> {
        let tokens: Vec<&str> = command.split_whitespace().collect();
        self.filters.iter().find(|filter| {
            !filter.command_prefix.is_empty()
                && filter.command_prefix.len() <= tokens.len()
                && filter
                    .command_prefix
                    .iter()
                    .zip(tokens.iter())
                    .all(|(expected, actual)| expected == actual)
        })
    }

    /// Minimize the output of `command`.
    ///
    /// When a filter matches and the output is large enough to be worth
    /// reducing, the interesting lines are kept and the original is
    /// referenced as `artifact://bash/<id>`.
    #[must_use]
    pub fn minimize(&self, command: &str, output: &str) -> MinimizeResult {
        if !self.enabled {
            return MinimizeResult {
                output: output.to_string(),
                original_ref: None,
                filter: None,
                reduced: false,
            };
        }
        let Some(filter) = self.select_filter(command) else {
            return MinimizeResult {
                output: output.to_string(),
                original_ref: None,
                filter: None,
                reduced: false,
            };
        };
        let reduced = reduce(output, self.max_lines);
        if reduced == output {
            return MinimizeResult {
                output: output.to_string(),
                original_ref: None,
                filter: Some(filter.name.to_string()),
                reduced: false,
            };
        }
        let original_ref = artifact_ref(output);
        let mut rendered = format!("[{}] {}\n", filter.name, original_ref);
        rendered.push_str(&reduced);
        MinimizeResult {
            output: rendered,
            original_ref: Some(original_ref),
            filter: Some(filter.name.to_string()),
            reduced: true,
        }
    }
}

/// Reduce text to the interesting lines, eliding the middle past `max_lines`.
#[must_use]
pub fn reduce(text: &str, max_lines: usize) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let mut kept: Vec<&str> = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        if DROP_MARKERS.iter().any(|marker| line.contains(marker)) {
            continue;
        }
        let interesting = KEEP_MARKERS.iter().any(|marker| line.contains(marker));
        let trailing_context = index + TAIL_CONTEXT_LINES >= lines.len();
        if interesting || trailing_context {
            kept.push(line);
        }
    }
    if kept.len() > max_lines {
        let head = max_lines / 2;
        let tail = max_lines - head;
        let mut elided: Vec<&str> = kept[..head].to_vec();
        elided.push("... (middle elided by bash minimizer) ...");
        elided.extend_from_slice(&kept[kept.len() - tail..]);
        kept = elided;
    }
    kept.join("\n")
}

/// A stable `artifact://` reference for the full output.
#[must_use]
pub fn artifact_ref(original: &str) -> String {
    use sha2::Digest as _;
    use sha2::Sha256;
    use std::fmt::Write as _;
    let mut hasher = Sha256::new();
    hasher.update(original.as_bytes());
    let digest = hasher.finalize();
    let mut hex = String::with_capacity(16);
    for byte in digest.iter().take(8) {
        let _ = write!(hex, "{byte:02x}");
    }
    format!("artifact://bash/{hex}")
}

/// The built-in filter table (25 typed commands).
#[must_use]
pub fn builtin_filters() -> Vec<Filter> {
    const TABLE: &[(&str, &[&str])] = &[
        ("cargo-build", &["cargo", "build"]),
        ("cargo-test", &["cargo", "test"]),
        ("cargo-check", &["cargo", "check"]),
        ("cargo-clippy", &["cargo", "clippy"]),
        ("cargo-fmt", &["cargo", "fmt"]),
        ("go-build", &["go", "build"]),
        ("go-test", &["go", "test"]),
        ("go-vet", &["go", "vet"]),
        ("git-status", &["git", "status"]),
        ("git-diff", &["git", "diff"]),
        ("git-log", &["git", "log"]),
        ("git-add", &["git", "add"]),
        ("git-commit", &["git", "commit"]),
        ("git-push", &["git", "push"]),
        ("docker-ps", &["docker", "ps"]),
        ("docker-build", &["docker", "build"]),
        ("docker-logs", &["docker", "logs"]),
        ("docker-compose", &["docker", "compose"]),
        ("npm-install", &["npm", "install"]),
        ("npm-test", &["npm", "test"]),
        ("yarn", &["yarn"]),
        ("pnpm", &["pnpm"]),
        ("pip-install", &["pip", "install"]),
        ("pytest", &["pytest"]),
        ("make", &["make"]),
    ];
    TABLE
        .iter()
        .map(|(name, prefix)| Filter {
            name,
            command_prefix: prefix,
        })
        .collect()
}

/// The distinct filter names in the built-in table (a convenience for tests
/// and diagnostics).
#[must_use]
pub fn filter_names() -> BTreeSet<&'static str> {
    builtin_filters().iter().map(|f| f.name).collect()
}
