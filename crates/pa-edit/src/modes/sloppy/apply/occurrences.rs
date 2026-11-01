//! Occurrence search: literal hits, the fuzzy windowed search, and the
//! normalized-offset ↔ source-offset maps a match is translated through.

use std::collections::BTreeSet;

use super::super::types::{NormalizedText, Occurrence, PatternToken};

use crate::fuzzy::levenshtein_within;

pub(super) fn exact_occurrences(content: &str, pattern: &str) -> Vec<Occurrence> {
    if pattern.is_empty() {
        return Vec::new();
    }
    let mut result = Vec::new();
    let mut from = 0;
    while from <= content.len().saturating_sub(pattern.len()) {
        let Some(relative) = content[from..].find(pattern) else {
            break;
        };
        let start = from + relative;
        result.push(Occurrence {
            start,
            end: start + pattern.len(),
            distance: 0,
            punctuation_edits: 0,
        });
        from = start + content[start..].chars().next().map_or(1, char::len_utf8);
    }
    result
}

fn operator_signature(text: &str) -> String {
    text.chars()
        .filter(|character| !(character.is_alphanumeric() || matches!(character, '_' | '$')))
        .collect()
}

fn differs_by_one_punctuation_insertion(left: &str, right: &str) -> bool {
    let left = left.chars().collect::<Vec<_>>();
    let right = right.chars().collect::<Vec<_>>();
    if left.len().abs_diff(right.len()) != 1 {
        return false;
    }
    let (shorter, longer) = if left.len() < right.len() {
        (&left, &right)
    } else {
        (&right, &left)
    };
    let mut short_index = 0;
    let mut inserted = None;
    for character in longer {
        if shorter.get(short_index) == Some(character) {
            short_index += 1;
            continue;
        }
        if inserted.is_some() {
            return false;
        }
        inserted = Some(*character);
    }
    inserted.is_some_and(|character| !matches!(character, '{' | '}' | '(' | ')' | '[' | ']'))
}

pub(super) fn fuzzy_occurrences(
    content: &str,
    pattern: &str,
    allow_punctuation: bool,
) -> Vec<Occurrence> {
    // These are only work limits. JS measured UTF-16 units; byte lengths are
    // intentionally acceptable here.
    if content.is_empty() || content.len() > 50_000 {
        return Vec::new();
    }
    if pattern.len() < 6 {
        return exact_occurrences(content, pattern);
    }
    let limit = 3.min(1.max(((pattern.len() as f64) * 0.12).floor() as usize));
    let seed_length = 5.min(3.max(pattern.len().saturating_sub(limit)));
    let offsets = [
        0,
        (pattern.len() - seed_length) / 2,
        pattern.len() - seed_length,
    ];
    let structural = operator_signature(pattern);
    let mut starts = BTreeSet::new();
    for offset in offsets {
        if !pattern.is_char_boundary(offset) || !pattern.is_char_boundary(offset + seed_length) {
            continue;
        }
        let seed = &pattern[offset..offset + seed_length];
        let mut from = 0;
        while from <= content.len().saturating_sub(seed.len()) {
            let Some(relative) = content[from..].find(seed) else {
                break;
            };
            let found = from + relative;
            for delta in -(limit as isize)..=limit as isize {
                let start = found as isize - offset as isize + delta;
                if start >= 0
                    && (start as usize) < content.len()
                    && content.is_char_boundary(start as usize)
                {
                    starts.insert(start as usize);
                }
            }
            from = found + seed.chars().next().map_or(1, char::len_utf8);
        }
    }
    // Same work-limit exception as the TypeScript implementation.
    if starts.is_empty() && content.len() <= 10_000 {
        starts.extend(content.char_indices().map(|(index, _)| index));
    }
    let pattern_chars: Vec<char> = pattern.chars().collect();
    let mut candidate_chars = Vec::new();
    let mut raw = Vec::new();
    for start in starts {
        let mut best: Option<Occurrence> = None;
        for length in pattern.len().saturating_sub(limit).max(1)..=pattern.len() + limit {
            // Only a distance within `limit` that beats the best so far counts.
            let max = match best {
                Some(current) if current.distance == 0 => break,
                Some(current) => limit.min(current.distance - 1),
                None => limit,
            };
            let end = start + length;
            if end > content.len() || !content.is_char_boundary(end) {
                continue;
            }
            let candidate = &content[start..end];
            let signature = operator_signature(candidate);
            let punctuation_edits = usize::from(signature != structural);
            if punctuation_edits != 0
                && (!allow_punctuation
                    || !differs_by_one_punctuation_insertion(&structural, &signature))
            {
                continue;
            }
            candidate_chars.clear();
            candidate_chars.extend(candidate.chars());
            let Some(distance) = levenshtein_within(&pattern_chars, &candidate_chars, max) else {
                continue;
            };
            best = Some(Occurrence {
                start,
                end,
                distance,
                punctuation_edits,
            });
        }
        if let Some(best) = best {
            raw.push(best);
        }
    }
    raw.sort_by_key(|entry| (entry.distance, entry.start));
    let mut distinct: Vec<Occurrence> = Vec::new();
    for candidate in raw {
        if distinct
            .iter()
            .any(|kept| candidate.start < kept.end && candidate.end > kept.start)
        {
            continue;
        }
        distinct.push(candidate);
    }
    distinct.sort_by_key(|entry| entry.start);
    distinct
}

pub(super) fn source_start(normalized: &NormalizedText, offset: usize, fallback: usize) -> usize {
    normalized.starts.get(offset).copied().unwrap_or(fallback)
}

pub(super) fn source_end(normalized: &NormalizedText, offset: usize, fallback: usize) -> usize {
    if offset == 0 {
        0
    } else {
        normalized.ends.get(offset - 1).copied().unwrap_or(fallback)
    }
}

/// Whether a candidate's selection spans can be applied without corrupting the
/// rewrite.
///
/// Consumers ([`super::rewrite::rewrite_selection_spans`], the ambiguity-outcome
/// projector, and [`super::rewrite::rewrite_proves_whole_span`]) assume the spans are ordered,
/// non-overlapping, distinct-start sub-ranges of `[start, end]`. Garbled `⟪…⟫`
/// marker glyphs can resolve pairs that overlap or fall outside that range;
/// splicing such a pair would index `content` (or the mutated
/// `content[start..end]` buffer) on an out-of-range or mid-char edge and panic
/// the worker instead of reporting a miss. Rejecting the candidate lets the
/// miss surface as an [`EditError`](crate::error::EditError).
pub(super) fn selection_spans_usable(spans: &[(usize, usize)], start: usize, end: usize) -> bool {
    let mut ordered: Vec<(usize, usize)> = spans.to_vec();
    ordered.sort_unstable();
    let mut cursor = start;
    let mut previous_start: Option<usize> = None;
    for (span_start, span_end) in ordered {
        if span_start < cursor
            || span_start > span_end
            || span_end > end
            || previous_start == Some(span_start)
        {
            return false;
        }
        previous_start = Some(span_start);
        cursor = span_end;
    }
    true
}

pub(super) fn preceding_literal(tokens: &[PatternToken], boundary: usize) -> Option<usize> {
    (0..boundary)
        .rev()
        .find(|index| matches!(tokens[*index], PatternToken::Literal { .. }))
}

pub(super) fn following_literal(tokens: &[PatternToken], boundary: usize) -> Option<usize> {
    (boundary..tokens.len()).find(|index| matches!(tokens[*index], PatternToken::Literal { .. }))
}
