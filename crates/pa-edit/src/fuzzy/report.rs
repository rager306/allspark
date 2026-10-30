//! The two error formatters, byte-for-byte with `EditMatchError.formatMessage`
//! and `formatOccurrenceError`, plus the preview window they render.

use crate::text::utf16_len;

use super::block::{FuzzyMatch, MatchOutcome};
use super::{MAX_RECORDED_MATCHES, OCCURRENCE_PREVIEW_CONTEXT, OCCURRENCE_PREVIEW_MAX_LEN};

pub(super) fn format_preview_window(lines: &[&str], center_index: usize) -> String {
    let start = center_index.saturating_sub(OCCURRENCE_PREVIEW_CONTEXT);
    let end = lines
        .len()
        .min(center_index + OCCURRENCE_PREVIEW_CONTEXT + 1);
    lines[start..end]
        .iter()
        .enumerate()
        .map(|(offset, line)| {
            let truncated = if utf16_len(line) > OCCURRENCE_PREVIEW_MAX_LEN {
                let mut units = 0;
                let mut text = String::new();
                for ch in line.chars() {
                    let width = ch.len_utf16();
                    if units + width > OCCURRENCE_PREVIEW_MAX_LEN - 1 {
                        break;
                    }
                    text.push(ch);
                    units += width;
                }
                text.push('…');
                text
            } else {
                (*line).to_owned()
            };
            format!("  {} | {truncated}", start + offset + 1)
        })
        .collect::<Vec<_>>()
        .join("\n")
}
fn first_different_line<'a>(old_lines: &'a [&str], new_lines: &'a [&str]) -> (&'a str, &'a str) {
    for index in 0..old_lines.len().max(new_lines.len()) {
        let old = old_lines.get(index).copied().unwrap_or("");
        let new = new_lines.get(index).copied().unwrap_or("");
        if old != new {
            return (old, new);
        }
    }
    (
        old_lines.first().copied().unwrap_or(""),
        new_lines.first().copied().unwrap_or(""),
    )
}

/// Format `EditMatchError.formatMessage` byte-for-byte.
pub fn format_match_error(
    path: &str,
    search_text: &str,
    closest: Option<&FuzzyMatch>,
    allow_fuzzy: bool,
    threshold: f64,
    fuzzy_matches: Option<usize>,
) -> String {
    let Some(closest) = closest else {
        return if allow_fuzzy {
            format!("Could not find a close enough match in {path}.")
        } else {
            format!(
                "Could not find the exact text in {path}. The old text must match exactly including \
				 all whitespace and newlines."
            )
        };
    };
    let similarity_percent = (closest.confidence * 100.0).round() as i64;
    let threshold_percent = (threshold * 100.0).round() as i64;
    let search_lines: Vec<&str> = search_text.split('\n').collect();
    let actual_lines: Vec<&str> = closest.actual_text.split('\n').collect();
    let (old_line, new_line) = first_different_line(&search_lines, &actual_lines);
    let hint = if allow_fuzzy {
        if fuzzy_matches.is_some_and(|count| count > 1) {
            format!(
                "Found {} high-confidence matches. Provide more context to make it unique.",
                fuzzy_matches.unwrap_or(0)
            )
        } else {
            format!("Closest match was below the {threshold_percent}% similarity threshold.")
        }
    } else {
        "Fuzzy matching is disabled. Enable 'Edit fuzzy match' in settings to accept high-confidence \
		 matches."
			.to_owned()
    };
    let heading = if allow_fuzzy {
        format!("Could not find a close enough match in {path}.")
    } else {
        format!("Could not find the exact text in {path}.")
    };
    format!(
        "{heading}\n\nClosest match ({similarity_percent}% similar) at line {}:\n  - {old_line}\n  \
		 + {new_line}\n{hint}",
        closest.start_line
    )
}

/// Format `formatOccurrenceError` byte-for-byte.
pub fn format_occurrence_error(path: &str, outcome: &MatchOutcome) -> String {
    let occurrences = outcome.occurrences.unwrap_or(0);
    let previews = outcome
        .occurrence_previews
        .as_ref()
        .map_or_else(String::new, |items| items.join("\n\n"));
    let more = if occurrences > MAX_RECORDED_MATCHES {
        format!(" (showing first {MAX_RECORDED_MATCHES} of {occurrences})")
    } else {
        String::new()
    };
    format!(
        "Found {occurrences} occurrences in {path}{more}:\n\n{previews}\n\nAdd more context lines \
		 to disambiguate."
    )
}
