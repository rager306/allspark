//! Line-sequence placement: the exact-to-character fallback ladder, and the
//! best-scoring placement a caller reports when the ladder misses.

use crate::text::{js_trim, js_trim_end, js_trim_start, normalize_for_fuzzy, normalize_unicode};

use super::block::{FindMatchOptions, find_match};
use super::distance::similarity;
use super::matches::{IndexedMatches, collect_indexed_matches};
use super::{
    CHARACTER_MATCH_THRESHOLD, DOMINANT_FUZZY_DELTA, DOMINANT_FUZZY_MIN_CONFIDENCE,
    MAX_RECORDED_MATCHES, PARTIAL_MATCH_MIN_LENGTH, PARTIAL_MATCH_MIN_RATIO,
    SEQUENCE_FUZZY_THRESHOLD,
};

/// Strategy which located a line sequence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SequenceMatchStrategy {
    Exact,
    TrimTrailing,
    Trim,
    CommentPrefix,
    Unicode,
    Prefix,
    Substring,
    Fuzzy,
    FuzzyDominant,
    Character,
}

/// Result of a line-sequence search.
#[derive(Debug, Clone, PartialEq)]
pub struct SequenceSearchResult {
    pub index: Option<usize>,
    pub confidence: f64,
    pub match_count: Option<usize>,
    pub match_indices: Option<Vec<usize>>,
    pub strategy: Option<SequenceMatchStrategy>,
}

fn sequence_result(
    matches: &IndexedMatches,
    confidence: f64,
    strategy: SequenceMatchStrategy,
    ambiguous: bool,
) -> Option<SequenceSearchResult> {
    Some(SequenceSearchResult {
        index: Some(matches.first_match?),
        confidence,
        match_count: ambiguous.then_some(matches.match_count),
        match_indices: ambiguous.then(|| matches.match_indices.clone()),
        strategy: Some(strategy),
    })
}

const fn no_sequence_match(confidence: f64, match_count: Option<usize>) -> SequenceSearchResult {
    SequenceSearchResult {
        index: None,
        confidence,
        match_count,
        match_indices: None,
        strategy: None,
    }
}

fn matches_at<T>(
    lines: &[T],
    pattern: &[T],
    index: usize,
    mut compare: impl FnMut(&T, &T) -> bool,
) -> bool {
    pattern
        .iter()
        .enumerate()
        .all(|(offset, expected)| compare(&lines[index + offset], expected))
}

fn fuzzy_score_at(lines: &[String], pattern: &[String], index: usize, min_score: f64) -> f64 {
    let count = pattern.len();
    let mut total = 0.0;
    for (offset, pat) in pattern.iter().enumerate() {
        let line = &lines[index + offset];
        if line == pat {
            total += 1.0;
            continue;
        }
        let remaining = count - offset - 1;
        let line_len = line.chars().count();
        let pat_len = pat.chars().count();
        let max_len = line_len.max(pat_len);
        let upper_bound = if max_len == 0 {
            1.0
        } else {
            1.0 - line_len.abs_diff(pat_len) as f64 / max_len as f64
        };
        if (total + upper_bound + remaining as f64) / (count as f64) < min_score {
            return total / count as f64;
        }
        if upper_bound > 0.0 {
            total += similarity(line, pat);
        }
        if (total + remaining as f64) / (count as f64) < min_score {
            return total / count as f64;
        }
    }
    total / count as f64
}

fn norm_starts_with(line: &str, pattern: &str) -> bool {
    if pattern.is_empty() {
        line.is_empty()
    } else {
        line.starts_with(pattern)
    }
}

fn norm_includes(line: &str, pattern: &str) -> bool {
    let pattern_len = pattern.chars().count();
    let line_len = line.chars().count();
    if pattern.is_empty() {
        return line.is_empty();
    }
    pattern_len >= PARTIAL_MATCH_MIN_LENGTH
        && line.contains(pattern)
        && pattern_len as f64 / line_len.max(1) as f64 >= PARTIAL_MATCH_MIN_RATIO
}

fn strip_comment_prefix(line: &str) -> &str {
    let trimmed = js_trim_start(line);
    let without = if let Some(rest) = trimmed.strip_prefix("/*") {
        rest
    } else if let Some(rest) = trimmed.strip_prefix("*/") {
        rest
    } else if let Some(rest) = trimmed.strip_prefix("//") {
        rest
    } else if let Some(rest) = trimmed.strip_prefix('*') {
        rest
    } else if let Some(rest) = trimmed.strip_prefix('#') {
        rest
    } else if let Some(rest) = trimmed.strip_prefix(';') {
        rest
    } else if let Some(rest) = trimmed.strip_prefix("/ ") {
        rest
    } else {
        trimmed
    };
    js_trim_start(without)
}

fn run_sequence_passes(
    lines: &[&str],
    pattern: &[&str],
    from: usize,
    to: usize,
    allow_fuzzy: bool,
    lines_normalized: &[String],
    pattern_normalized: &[String],
) -> Option<SequenceSearchResult> {
    let exact = collect_indexed_matches(from, to, |index| {
        matches_at(lines, pattern, index, |a, b| a == b)
    });
    if let Some(result) = sequence_result(&exact, 1.0, SequenceMatchStrategy::Exact, false) {
        return Some(result);
    }
    let trailing = collect_indexed_matches(from, to, |index| {
        matches_at(lines, pattern, index, |a, b| {
            js_trim_end(a) == js_trim_end(b)
        })
    });
    if let Some(result) =
        sequence_result(&trailing, 0.99, SequenceMatchStrategy::TrimTrailing, false)
    {
        return Some(result);
    }
    let trimmed = collect_indexed_matches(from, to, |index| {
        matches_at(lines, pattern, index, |a, b| js_trim(a) == js_trim(b))
    });
    if let Some(result) = sequence_result(&trimmed, 0.98, SequenceMatchStrategy::Trim, false) {
        return Some(result);
    }
    let comments = collect_indexed_matches(from, to, |index| {
        matches_at(lines, pattern, index, |a, b| {
            strip_comment_prefix(a) == strip_comment_prefix(b)
        })
    });
    if let Some(result) = sequence_result(
        &comments,
        0.975,
        SequenceMatchStrategy::CommentPrefix,
        false,
    ) {
        return Some(result);
    }
    let unicode = collect_indexed_matches(from, to, |index| {
        matches_at(lines, pattern, index, |a, b| {
            normalize_unicode(a) == normalize_unicode(b)
        })
    });
    if let Some(result) = sequence_result(&unicode, 0.97, SequenceMatchStrategy::Unicode, false) {
        return Some(result);
    }
    if !allow_fuzzy {
        return None;
    }
    let prefix = collect_indexed_matches(from, to, |index| {
        matches_at(lines_normalized, pattern_normalized, index, |a, b| {
            norm_starts_with(a, b)
        })
    });
    if let Some(result) = sequence_result(&prefix, 0.965, SequenceMatchStrategy::Prefix, true) {
        return Some(result);
    }
    let substring = collect_indexed_matches(from, to, |index| {
        matches_at(lines_normalized, pattern_normalized, index, |a, b| {
            norm_includes(a, b)
        })
    });
    sequence_result(&substring, 0.94, SequenceMatchStrategy::Substring, true)
}

/// Locate `pattern` lines through the exact-to-character fallback ladder.
pub fn seek_sequence(
    lines: &[&str],
    pattern: &[&str],
    start: usize,
    eof: bool,
    allow_fuzzy: bool,
) -> SequenceSearchResult {
    if pattern.is_empty() {
        return SequenceSearchResult {
            index: Some(start),
            confidence: 1.0,
            match_count: None,
            match_indices: None,
            strategy: Some(SequenceMatchStrategy::Exact),
        };
    }
    if pattern.len() > lines.len() {
        return no_sequence_match(0.0, None);
    }
    let max_start = lines.len() - pattern.len();
    let search_start = if eof { max_start } else { start };
    let lines_normalized: Vec<String> =
        lines.iter().map(|line| normalize_for_fuzzy(line)).collect();
    let pattern_normalized: Vec<String> = pattern
        .iter()
        .map(|line| normalize_for_fuzzy(line))
        .collect();
    if let Some(result) = run_sequence_passes(
        lines,
        pattern,
        search_start,
        max_start,
        allow_fuzzy,
        &lines_normalized,
        &pattern_normalized,
    ) {
        return result;
    }
    if eof
        && search_start > start
        && let Some(result) = run_sequence_passes(
            lines,
            pattern,
            start,
            max_start,
            allow_fuzzy,
            &lines_normalized,
            &pattern_normalized,
        )
    {
        return result;
    }
    if !allow_fuzzy {
        return no_sequence_match(0.0, None);
    }

    let mut best_score = 0.0;
    let mut second_best_score = 0.0;
    let mut best_index = None;
    let mut fuzzy_matches = IndexedMatches::default();
    let fuzzy_bail = SEQUENCE_FUZZY_THRESHOLD - DOMINANT_FUZZY_DELTA;
    let mut score_range = |from: usize, to: usize| {
        if from > to {
            return;
        }
        for index in from..=to {
            let score = fuzzy_score_at(&lines_normalized, &pattern_normalized, index, fuzzy_bail);
            if score >= SEQUENCE_FUZZY_THRESHOLD {
                fuzzy_matches.first_match.get_or_insert(index);
                fuzzy_matches.match_count += 1;
                if fuzzy_matches.match_indices.len() < MAX_RECORDED_MATCHES {
                    fuzzy_matches.match_indices.push(index);
                }
            }
            if score > best_score {
                second_best_score = best_score;
                best_score = score;
                best_index = Some(index);
            } else if score > second_best_score {
                second_best_score = score;
            }
        }
    };
    score_range(search_start, max_start);
    if eof && search_start > start {
        score_range(start, search_start - 1);
    }
    if let Some(index) = best_index.filter(|_| best_score >= SEQUENCE_FUZZY_THRESHOLD) {
        let dominant = fuzzy_matches.match_count > 1
            && best_score >= DOMINANT_FUZZY_MIN_CONFIDENCE
            && best_score - second_best_score >= DOMINANT_FUZZY_DELTA;
        return SequenceSearchResult {
            index: Some(index),
            confidence: best_score,
            match_count: Some(if dominant {
                1
            } else {
                fuzzy_matches.match_count
            }),
            match_indices: Some(fuzzy_matches.match_indices),
            strategy: Some(if dominant {
                SequenceMatchStrategy::FuzzyDominant
            } else {
                SequenceMatchStrategy::Fuzzy
            }),
        };
    }

    let pattern_text = pattern.join("\n");
    let content_text = lines.get(start..).unwrap_or_default().join("\n");
    let outcome = find_match(
        &content_text,
        &pattern_text,
        &FindMatchOptions {
            allow_fuzzy: true,
            threshold: Some(CHARACTER_MATCH_THRESHOLD),
            excluded_ranges: &[],
        },
    );
    if let Some(matched) = outcome.matched {
        let line_index = start
            + content_text[..matched.start_index]
                .bytes()
                .filter(|byte| *byte == b'\n')
                .count();
        return SequenceSearchResult {
            index: Some(line_index),
            confidence: matched.confidence,
            match_count: Some(outcome.occurrences.or(outcome.fuzzy_matches).unwrap_or(1)),
            match_indices: None,
            strategy: Some(SequenceMatchStrategy::Character),
        };
    }
    no_sequence_match(best_score, outcome.occurrences.or(outcome.fuzzy_matches))
}

/// Best-scoring placement of `pattern` regardless of threshold.
pub fn find_closest_sequence_match(
    lines: &[&str],
    pattern: &[&str],
    start: Option<usize>,
    eof: bool,
) -> (Option<usize>, f64, SequenceMatchStrategy) {
    let start = start.unwrap_or(0);
    if pattern.is_empty() {
        return (Some(start), 1.0, SequenceMatchStrategy::Exact);
    }
    if pattern.len() > lines.len() {
        return (None, 0.0, SequenceMatchStrategy::Fuzzy);
    }
    let max_start = lines.len() - pattern.len();
    let search_start = if eof { max_start } else { start };
    let lines_normalized: Vec<String> =
        lines.iter().map(|line| normalize_for_fuzzy(line)).collect();
    let pattern_normalized: Vec<String> = pattern
        .iter()
        .map(|line| normalize_for_fuzzy(line))
        .collect();
    let mut best_index = None;
    let mut best_score = 0.0;
    if search_start <= max_start {
        for index in search_start..=max_start {
            let score = fuzzy_score_at(&lines_normalized, &pattern_normalized, index, best_score);
            if score > best_score {
                best_score = score;
                best_index = Some(index);
            }
        }
    }
    if eof && search_start > start {
        for index in start..search_start {
            let score = fuzzy_score_at(&lines_normalized, &pattern_normalized, index, best_score);
            if score > best_score {
                best_score = score;
                best_index = Some(index);
            }
        }
    }
    (best_index, best_score, SequenceMatchStrategy::Fuzzy)
}
