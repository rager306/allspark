//! Whole-text replacement: exact when possible, fuzzy by exclusion, and one
//! replacement when the match is unique.

use crate::{
    error::EditError,
    text::{adjust_indentation, normalize_to_lf},
};

use super::DEFAULT_FUZZY_THRESHOLD;
use super::block::{ExcludedRange, FindMatchOptions, MatchOutcome, find_match};

/// Result of [`replace_text`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplaceResult {
    pub content: String,
    pub count: usize,
}

/// What [`replace_text`] did.
#[derive(Debug, Clone, PartialEq)]
pub enum ReplaceOutcome {
    /// At least one replacement.
    Replaced(ReplaceResult),
    /// Nothing replaced: the search found several exact occurrences, no
    /// match, or only a match the replacement leaves unchanged. Callers
    /// format their error from it instead of searching again.
    Missed(MatchOutcome),
}

#[derive(Debug)]
struct Replacement {
    start: usize,
    end: usize,
    text: String,
}

/// Find and replace text using the same exact/fuzzy behavior as `replaceText`.
pub fn replace_text(
    content: &str,
    old_text: &str,
    new_text: &str,
    fuzzy: bool,
    all: bool,
    threshold: Option<f64>,
) -> Result<ReplaceOutcome, EditError> {
    if old_text.is_empty() {
        return Err(EditError::apply("oldText must not be empty."));
    }
    let threshold = threshold.unwrap_or(DEFAULT_FUZZY_THRESHOLD);
    let normalized_content = normalize_to_lf(content);
    let normalized_old = normalize_to_lf(old_text);
    let normalized_new = normalize_to_lf(new_text);
    if all {
        let exact_count = normalized_content
            .match_indices(normalized_old.as_ref())
            .count();
        if exact_count > 0 {
            return Ok(ReplaceOutcome::Replaced(ReplaceResult {
                content: normalized_content
                    .replace(normalized_old.as_ref(), normalized_new.as_ref()),
                count: exact_count,
            }));
        }
        let mut replacements: Vec<Replacement> = Vec::new();
        loop {
            let excluded: Vec<ExcludedRange> = replacements
                .iter()
                .map(|replacement| ExcludedRange {
                    start_index: replacement.start,
                    end_index: replacement.end,
                })
                .collect();
            let outcome = find_match(
                &normalized_content,
                normalized_old.as_ref(),
                &FindMatchOptions {
                    allow_fuzzy: fuzzy,
                    threshold: Some(threshold),
                    excluded_ranges: &excluded,
                },
            );
            let should_use_closest = fuzzy
                && outcome
                    .closest
                    .as_ref()
                    .is_some_and(|matched| matched.confidence >= threshold)
                && outcome.fuzzy_matches.is_none_or(|count| count <= 1);
            let matched = outcome
                .matched
                .as_ref()
                .or_else(|| outcome.closest.as_ref().filter(|_| should_use_closest));
            let adjusted = matched.map(|matched| {
                adjust_indentation(
                    normalized_old.as_ref(),
                    &matched.actual_text,
                    normalized_new.as_ref(),
                )
            });
            let (Some(matched), Some(adjusted)) = (matched, adjusted) else {
                if replacements.is_empty() {
                    return Ok(ReplaceOutcome::Missed(outcome));
                }
                break;
            };
            if adjusted == matched.actual_text {
                if replacements.is_empty() {
                    return Ok(ReplaceOutcome::Missed(outcome));
                }
                break;
            }
            replacements.push(Replacement {
                start: matched.start_index,
                // JavaScript's `substring` clamps this synthetic one-byte span
                // when an empty fuzzy window lands at EOF.
                end: (matched.start_index + matched.actual_text.len().max(1))
                    .min(normalized_content.len()),
                text: adjusted,
            });
        }
        replacements.sort_by_key(|replacement| replacement.start);
        let mut output = String::with_capacity(normalized_content.len());
        let mut source_index = 0;
        for replacement in &replacements {
            output.push_str(&normalized_content[source_index..replacement.start]);
            output.push_str(&replacement.text);
            source_index = replacement.end;
        }
        output.push_str(&normalized_content[source_index..]);
        return Ok(ReplaceOutcome::Replaced(ReplaceResult {
            content: output,
            count: replacements.len(),
        }));
    }

    let outcome = find_match(
        &normalized_content,
        normalized_old.as_ref(),
        &FindMatchOptions {
            allow_fuzzy: fuzzy,
            threshold: Some(threshold),
            excluded_ranges: &[],
        },
    );
    let matched = match &outcome.matched {
        Some(matched) if outcome.occurrences.is_none_or(|count| count <= 1) => matched,
        _ => return Ok(ReplaceOutcome::Missed(outcome)),
    };
    let adjusted = adjust_indentation(
        normalized_old.as_ref(),
        &matched.actual_text,
        normalized_new.as_ref(),
    );
    let mut output = String::with_capacity(
        normalized_content.len() - matched.actual_text.len() + adjusted.len(),
    );
    output.push_str(&normalized_content[..matched.start_index]);
    output.push_str(&adjusted);
    output.push_str(&normalized_content[matched.start_index + matched.actual_text.len()..]);
    Ok(ReplaceOutcome::Replaced(ReplaceResult {
        content: output,
        count: 1,
    }))
}
