//! Context-line placement: one `@@ context` line's own fallback ladder, ending
//! in a function-signature retry.

use crate::text::{js_trim, normalize_for_fuzzy, normalize_unicode};

use super::distance::similarity;
use super::matches::{IndexedMatches, collect_indexed_matches};
use super::{
    CONTEXT_FUZZY_THRESHOLD, MAX_RECORDED_MATCHES, PARTIAL_MATCH_MIN_LENGTH,
    PARTIAL_MATCH_MIN_RATIO,
};

/// Strategy which located a context line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContextMatchStrategy {
    Exact,
    Trim,
    Unicode,
    Prefix,
    Substring,
    Fuzzy,
}

/// Result of a context-line search.
#[derive(Debug, Clone, PartialEq)]
pub struct ContextLineResult {
    pub index: Option<usize>,
    pub confidence: f64,
    pub match_count: Option<usize>,
    pub match_indices: Option<Vec<usize>>,
    pub strategy: Option<ContextMatchStrategy>,
}

fn context_result(
    matches: &IndexedMatches,
    confidence: f64,
    strategy: ContextMatchStrategy,
) -> Option<ContextLineResult> {
    Some(ContextLineResult {
        index: Some(matches.first_match?),
        confidence,
        match_count: Some(matches.match_count),
        match_indices: Some(matches.match_indices.clone()),
        strategy: Some(strategy),
    })
}

const fn no_context_match(confidence: f64) -> ContextLineResult {
    ContextLineResult {
        index: None,
        confidence,
        match_count: None,
        match_indices: None,
        strategy: None,
    }
}

/// Locate a single `@@ context` line at or after `start_from`.
pub fn find_context_line(
    lines: &[&str],
    context: &str,
    start_from: usize,
    allow_fuzzy: bool,
    skip_function_fallback: bool,
) -> ContextLineResult {
    if lines.is_empty() || start_from >= lines.len() {
        return no_context_match(0.0);
    }
    let end = lines.len() - 1;
    let trimmed_context = js_trim(context);
    let exact = collect_indexed_matches(start_from, end, |index| lines[index] == context);
    if let Some(result) = context_result(&exact, 1.0, ContextMatchStrategy::Exact) {
        return result;
    }
    let trimmed = collect_indexed_matches(start_from, end, |index| {
        js_trim(lines[index]) == trimmed_context
    });
    if let Some(result) = context_result(&trimmed, 0.99, ContextMatchStrategy::Trim) {
        return result;
    }
    let normalized_context = normalize_unicode(context);
    let unicode = collect_indexed_matches(start_from, end, |index| {
        normalize_unicode(lines[index]) == normalized_context
    });
    if let Some(result) = context_result(&unicode, 0.98, ContextMatchStrategy::Unicode) {
        return result;
    }
    if !allow_fuzzy {
        return no_context_match(0.0);
    }
    let context_normalized = normalize_for_fuzzy(context);
    if !context_normalized.is_empty() {
        let prefix = collect_indexed_matches(start_from, end, |index| {
            normalize_for_fuzzy(lines[index]).starts_with(&context_normalized)
        });
        if let Some(result) = context_result(&prefix, 0.96, ContextMatchStrategy::Prefix) {
            return result;
        }
    }
    if context_normalized.chars().count() >= PARTIAL_MATCH_MIN_LENGTH {
        let context_len = context_normalized.chars().count();
        let all_substrings: Vec<(usize, f64)> = (start_from..lines.len())
            .filter_map(|index| {
                let normalized = normalize_for_fuzzy(lines[index]);
                normalized.contains(&context_normalized).then(|| {
                    (
                        index,
                        context_len as f64 / normalized.chars().count().max(1) as f64,
                    )
                })
            })
            .collect();
        let match_indices: Vec<usize> = all_substrings
            .iter()
            .take(MAX_RECORDED_MATCHES)
            .map(|(index, _)| *index)
            .collect();
        if all_substrings.len() == 1 {
            return ContextLineResult {
                index: Some(all_substrings[0].0),
                confidence: 0.94,
                match_count: Some(1),
                match_indices: Some(match_indices),
                strategy: Some(ContextMatchStrategy::Substring),
            };
        }
        let qualifying: Vec<usize> = all_substrings
            .iter()
            .filter_map(|(index, ratio)| (*ratio >= PARTIAL_MATCH_MIN_RATIO).then_some(*index))
            .collect();
        if let Some(&first) = qualifying.first() {
            return ContextLineResult {
                index: Some(first),
                confidence: 0.94,
                match_count: Some(qualifying.len()),
                match_indices: Some(match_indices),
                strategy: Some(ContextMatchStrategy::Substring),
            };
        }
        if all_substrings.len() > 1 {
            return ContextLineResult {
                index: Some(all_substrings[0].0),
                confidence: 0.94,
                match_count: Some(all_substrings.len()),
                match_indices: Some(match_indices),
                strategy: Some(ContextMatchStrategy::Substring),
            };
        }
    }

    let mut best_index = None;
    let mut best_score = 0.0;
    let mut fuzzy_matches = IndexedMatches::default();
    for (index, &line) in lines.iter().enumerate().skip(start_from) {
        let score = similarity(&normalize_for_fuzzy(line), &context_normalized);
        if score >= CONTEXT_FUZZY_THRESHOLD {
            fuzzy_matches.first_match.get_or_insert(index);
            fuzzy_matches.match_count += 1;
            if fuzzy_matches.match_indices.len() < MAX_RECORDED_MATCHES {
                fuzzy_matches.match_indices.push(index);
            }
        }
        if score > best_score {
            best_score = score;
            best_index = Some(index);
        }
    }
    if let Some(index) = best_index.filter(|_| best_score >= CONTEXT_FUZZY_THRESHOLD) {
        return ContextLineResult {
            index: Some(index),
            confidence: best_score,
            match_count: Some(fuzzy_matches.match_count),
            match_indices: Some(fuzzy_matches.match_indices),
            strategy: Some(ContextMatchStrategy::Fuzzy),
        };
    }
    if !skip_function_fallback && trimmed_context.ends_with("()") {
        let base = trimmed_context
            .strip_suffix("()")
            .unwrap_or(trimmed_context);
        let with_paren = format!("{base}(");
        let result = find_context_line(lines, &with_paren, start_from, allow_fuzzy, true);
        if result.index.is_some() || result.match_count.unwrap_or(0) > 0 {
            return result;
        }
        return find_context_line(lines, base, start_from, allow_fuzzy, true);
    }
    no_context_match(best_score)
}
