/// Structural search with ast-grep over `root`.
///
/// `pa-ast` has carried a complete structural-search layer since the port
/// (`compile_search_patterns`, `collect_matches`, `collect_matched_files`,
/// `has_glob_syntax`, the six-step strictness ladder) and nothing called it: eleven of
/// its thirteen entry points had no caller outside the crate. That is the one capability
/// the fork paid for and never exposed.
///
/// It lives HERE, not in `pa-core`, because `pa-graph` already depends on `pa-ast` for
/// symbol extraction: reaching the layer from `pa-core` would have meant a new crate
/// edge for the tool surface, which is an architectural change for a search.
#[derive(Debug, Clone)]
pub struct PatternSearch {
    /// The structural pattern (`$A`, `$$$`, `$$$ARGS` are ast-grep metavariables).
    pub pattern: String,
    /// Glob or path narrowing the walk; `**/*` by default.
    pub glob: Option<String>,
    /// Language override; inferred per file when absent.
    pub lang: Option<String>,
    /// Maximum matches returned.
    pub limit: usize,
}

use crate::Result;
/// What one structural match found.
#[derive(Debug, Clone, serde::Serialize)]
pub struct PatternHit {
    pub path: String,
    pub line: usize,
    pub column: usize,
    pub text: String,
}

/// The result of one [`search_pattern`] run.
#[derive(Debug, Clone, serde::Serialize)]
pub struct PatternReport {
    pub hits: Vec<PatternHit>,
    pub scanned_files: usize,
    pub skipped_files: usize,
    pub truncated: bool,
}

/// Run `request` under `root`, and report what it saw as well as what it found.
///
/// # Errors
/// Fails when the walk fails or the pattern does not compile for a file whose language
/// could be resolved — a pattern that is valid Rust and invalid Python is a real error,
/// not a skip, because the caller asked for it explicitly.
pub fn search_pattern(root: &std::path::Path, request: &PatternSearch) -> Result<PatternReport> {
    let mut files: Vec<std::path::PathBuf> = Vec::new();
    let glob = request.glob.clone().unwrap_or_else(|| "**/*".to_string());
    files.extend(
        pa_ast::ops::collect_matched_files(root, &[glob])
            .map_err(|err| crate::GraphError::Parse(format!("pattern walk failed: {err}")))?
            .into_iter()
            .map(|file| file.absolute_path),
    );
    let explicit_lang = request.lang.as_deref();
    let mut hits: Vec<PatternHit> = Vec::new();
    let mut scanned = 0_usize;
    let mut skipped = 0_usize;
    let mut truncated = false;
    for file in &files {
        if hits.len() >= request.limit {
            truncated = true;
            break;
        }
        let Ok(language) = pa_ast::ops::resolve_language(explicit_lang, file) else {
            skipped += 1;
            continue;
        };
        let Ok(source) = std::fs::read_to_string(file) else {
            skipped += 1;
            continue;
        };
        let patterns =
            pa_ast::ops::compile_search_patterns(&request.pattern, language).map_err(|err| {
                crate::GraphError::Parse(format!(
                    "pattern does not compile for {}: {err}",
                    file.display()
                ))
            })?;
        scanned += 1;
        let relative = file
            .strip_prefix(root)
            .unwrap_or(file)
            .to_string_lossy()
            .to_string();
        for found in pa_ast::ops::collect_matches(&source, language, &patterns) {
            if hits.len() >= request.limit {
                truncated = true;
                break;
            }
            hits.push(PatternHit {
                path: relative.clone(),
                line: found.line + 1,
                column: found.column + 1,
                text: found.text,
            });
        }
    }
    Ok(PatternReport {
        hits,
        scanned_files: scanned,
        skipped_files: skipped,
        truncated,
    })
}
