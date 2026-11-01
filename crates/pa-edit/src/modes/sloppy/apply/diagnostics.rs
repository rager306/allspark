//! Failure diagnostics: the line-numbered previews, closest-fragment search,
//! and the no-match error that carries a copy-ready correction.

use super::super::{
    parse::{has_marker_lines, missing_unmarked_lines, operation_payload},
    types::{Operation, ParsedPattern, PatternToken},
};

use super::{
    occurrences::{exact_occurrences, source_end, source_start},
    pattern::normalize_text,
};

use crate::error::EditError;
use crate::fuzzy::PatternDistance;

pub(super) fn line_number_at(content: &str, offset: usize) -> usize {
    content[..offset.min(content.len())]
        .bytes()
        .filter(|byte| *byte == b'\n')
        .count()
        + 1
}

pub(super) fn numbered_preview(content: &str, offset: usize) -> String {
    let lines = content.split('\n').collect::<Vec<_>>();
    let anchor = line_number_at(content, offset.min(content.len())) - 1;
    let mut start = anchor.saturating_sub(4);
    if lines.len().saturating_sub(start) < 10 {
        start = lines.len().saturating_sub(10);
    }
    lines
        .iter()
        .skip(start)
        .take(10)
        .enumerate()
        .map(|(index, line)| format!("{}: {line}", start + index + 1))
        .collect::<Vec<_>>()
        .join("\n")
}

fn display_fragment(text: &str) -> String {
    if text.contains('\n') && text.split('\n').count() <= 8 {
        return format!("\n{text}");
    }
    let compact = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let compact = if compact.chars().count() > 80 {
        format!("{}…", compact.chars().take(77).collect::<String>())
    } else {
        compact
    };
    serde_json::to_string(&compact).expect("string serializes")
}

fn first_literal(pattern: &ParsedPattern) -> Option<(&str, &str)> {
    pattern.tokens.iter().find_map(|token| match token {
        PatternToken::Literal { text, normalized } => Some((text.as_str(), normalized.as_str())),
        PatternToken::Gap { .. } => None,
    })
}

pub(super) fn no_match_error(
    content: &str,
    pattern: &ParsedPattern,
    operation: &Operation,
    operation_number: usize,
    path: &str,
    standalone: bool,
) -> EditError {
    let normalized = normalize_text(content);
    let (literal, needle) =
        first_literal(pattern).unwrap_or((&operation.pattern_text, &operation.pattern_text));
    let occurrences = exact_occurrences(&normalized.text, needle);
    let closest = closest_fragment(content, needle);
    let (reason, offset) = if occurrences.is_empty() {
        (
            format!(
                "Failed fragment: {} has 0 occurrences.",
                display_fragment(literal)
            ),
            closest.1,
        )
    } else {
        (
            format!(
                "Failed fragment: {} could not align.",
                display_fragment(literal)
            ),
            source_start(&normalized, occurrences[0].start, 0),
        )
    };
    let first = if operation.all {
        format!("Operation {operation_number} (all) found 0 matches in {path}. {reason}")
    } else {
        format!("Operation {operation_number} did not match {path}. {reason}")
    };
    let missing = if has_marker_lines(&operation.source_pattern_text) {
        let lines = missing_unmarked_lines(content, &operation.source_pattern_text);
        if lines.is_empty() {
            String::new()
        } else {
            let listed = lines
                .iter()
                .take(3)
                .map(|line| display_fragment(line))
                .collect::<Vec<_>>()
                .join(", ");
            let more = if lines.len() > 3 { ", …" } else { "" };
            let verb = if lines.len() == 1 { "does" } else { "do" };
            format!(
                "\nUnmarked MATCH lines must already exist in the file; {listed}{more} {verb} not. \
				 Copy real lines from the file, and mark new lines to insert with ＋."
            )
        }
    } else {
        String::new()
    };
    let correction = if occurrences.is_empty()
        && closest.2 < 0.35
        && !closest.0.is_empty()
        && standalone
    {
        let corrected = operation.pattern_text.replacen(literal, &closest.0, 1);
        format!(
            "Copy-ready corrected operation:\n{}",
            operation_payload(operation, path, operation.all, Some(&corrected))
        )
    } else if standalone {
        "No copy-ready correction — the closest current text is only a fuzzy match. Re-read the \
			 region above and rebuild *** Find from the exact current text."
            .to_owned()
    } else {
        "No copy-ready correction — retrying this operation alone would drop sibling operations. \
			 Rebuild it inside the full payload."
            .to_owned()
    };
    EditError::matched(format!(
        "{first}{missing}\nCurrent file content near the closest match (no re-read \
		 needed):\n{}\n{correction}",
        numbered_preview(content, offset)
    ))
}

fn closest_fragment(content: &str, pattern: &str) -> (String, usize, f64) {
    let pattern_chars: Vec<char> = pattern.chars().collect();
    let pattern_distance = PatternDistance::new(&pattern_chars);
    let mut candidate_chars = Vec::new();
    let mut ranked = Vec::new();
    let mut offset = 0;
    for line in content.split('\n') {
        let normalized = normalize_text(line);
        if !normalized.text.is_empty() {
            let denominator = pattern.len().max(normalized.text.len()).max(1);
            candidate_chars.clear();
            candidate_chars.extend(normalized.text.chars());
            let score = pattern_distance.distance(&candidate_chars) as f64 / denominator as f64;
            ranked.push((line, offset, normalized, score));
            ranked.sort_by(|left, right| left.3.total_cmp(&right.3));
            ranked.truncate(3);
        }
        offset += line.len() + 1;
    }
    let Some(first) = ranked.first() else {
        return (pattern.to_owned(), 0, 1.0);
    };
    let mut best = (first.0.to_owned(), first.1, first.3);
    if pattern.len() <= 160 {
        for (line, line_offset, normalized, _) in ranked {
            let width = pattern.len().min(normalized.text.len());
            // The tail window only adds coverage when `len - width` lands on a
            // char boundary (e.g. width 0 appends `len`); `char_indices` already
            // yields every other boundary, and an unaligned fallback slices
            // inside multibyte chars (e.g. CJK) and panics.
            let tail = normalized.text.len().saturating_sub(width);
            let tail = normalized.text.is_char_boundary(tail).then_some(tail);
            for start in normalized
                .text
                .char_indices()
                .map(|(index, _)| index)
                .chain(tail)
            {
                let end = start + width;
                if end > normalized.text.len() || !normalized.text.is_char_boundary(end) {
                    continue;
                }
                let candidate = &normalized.text[start..end];
                let denominator = pattern.len().max(candidate.len()).max(1);
                candidate_chars.clear();
                candidate_chars.extend(candidate.chars());
                let score = pattern_distance.distance(&candidate_chars) as f64 / denominator as f64;
                if score >= best.2 {
                    continue;
                }
                let raw_start = source_start(&normalized, start, 0);
                let raw_end = source_end(&normalized, end, line.len());
                best = (
                    line[raw_start..raw_end].to_owned(),
                    line_offset + raw_start,
                    score,
                );
            }
        }
    }
    best
}
