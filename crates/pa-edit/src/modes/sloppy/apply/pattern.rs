//! `*** Find` normalization and pattern parsing: one pattern text becomes the
//! token stream every later stage matches against.

use super::super::types::{
    EdgeGaps, LiteralFallback, NormalizedText, ParsedPattern, PatternToken, SelectionPair,
    markers::{GAP, SELECT_CLOSE, SELECT_OPEN},
};

use crate::error::EditError;
use crate::text::normalize_unicode;

/// Normalize matching text while retaining source byte boundaries.
pub fn normalize_text(source: &str) -> NormalizedText {
    let mut text = String::with_capacity(source.len());
    let mut starts = Vec::with_capacity(source.len());
    let mut ends = Vec::with_capacity(source.len());
    for (start, character) in source.char_indices() {
        let end = start + character.len_utf8();
        if character.is_ascii_whitespace() {
            continue;
        }
        let normalized = if character.is_ascii() {
            character.to_string()
        } else {
            normalize_unicode(&character.to_string())
        };
        text.push_str(&normalized);
        for _ in normalized.bytes() {
            starts.push(start);
            ends.push(end);
        }
    }
    NormalizedText { text, starts, ends }
}

fn visible_identifier(text: &str) -> bool {
    text.chars()
        .any(|character| character.is_alphanumeric() || matches!(character, '_' | '$'))
}

pub(crate) fn parse_pattern(
    pattern: &str,
    operation_number: usize,
) -> Result<ParsedPattern, EditError> {
    if pattern.trim().is_empty() {
        return Err(EditError::matched(format!(
            "Operation {operation_number} has an empty pattern."
        )));
    }
    let open_count = pattern.matches(SELECT_OPEN).count();
    let close_count = pattern.matches(SELECT_CLOSE).count();
    if open_count > close_count {
        return Err(EditError::matched(format!(
            "Operation {operation_number} has an unclosed selection marker {SELECT_OPEN}; add \
			 closing {SELECT_CLOSE}."
        )));
    }
    if close_count > open_count {
        return Err(EditError::matched(format!(
            "Operation {operation_number} has an unmatched closing selection marker {SELECT_CLOSE}; \
			 add opening {SELECT_OPEN}."
        )));
    }
    let has_gap = pattern.contains(GAP);
    let has_selection = pattern.contains(SELECT_OPEN);
    if !has_gap && !has_selection {
        let normalized = normalize_text(pattern).text;
        if normalized.is_empty() {
            return Err(EditError::matched(format!(
                "Operation {operation_number} has no visible current text."
            )));
        }
        return Ok(ParsedPattern {
            tokens: vec![PatternToken::Literal {
                text: pattern.to_owned(),
                normalized,
            }],
            edge_gaps: EdgeGaps::default(),
            selection_start: 0,
            selection_end: 1,
            insertion: false,
            line_insertion: false,
            selected_capture_indices: Vec::new(),
            selection_ranges: Vec::new(),
            selection_pairs: Vec::new(),
            literal_fallback: None,
        });
    }

    let mut tokens = Vec::new();
    let mut literal = String::new();
    let mut capture_count = 0;
    let mut selection_boundaries = Vec::new();
    let mut selection_line_starts = Vec::new();
    let mut selection_raw_offsets = Vec::new();
    let flush = |tokens: &mut Vec<PatternToken>, literal: &mut String| {
        if literal.is_empty() {
            return;
        }
        let normalized = normalize_text(literal).text;
        if normalized.is_empty() {
            literal.clear();
        } else {
            tokens.push(PatternToken::Literal {
                text: std::mem::take(literal),
                normalized,
            });
        }
    };
    let mut index = 0;
    while index < pattern.len() {
        let rest = &pattern[index..];
        if rest.starts_with(GAP) {
            flush(&mut tokens, &mut literal);
            if matches!(tokens.last(), Some(PatternToken::Gap { .. })) {
                return Err(EditError::matched(format!(
                    "Operation {operation_number} has adjacent {GAP}; use one ellipsis."
                )));
            }
            let line_start = pattern[..index].rfind('\n').map_or(0, |at| at + 1);
            let line_end = pattern[index + GAP.len()..]
                .find('\n')
                .map_or(pattern.len(), |at| index + GAP.len() + at);
            let before = pattern[line_start..index]
                .replace(SELECT_OPEN, "")
                .replace(SELECT_CLOSE, "");
            let after = pattern[index + GAP.len()..line_end]
                .replace(SELECT_OPEN, "")
                .replace(SELECT_CLOSE, "");
            tokens.push(PatternToken::Gap {
                capture_index: capture_count,
                line_bounded: !before.trim().is_empty() && !after.trim().is_empty(),
            });
            capture_count += 1;
            index += GAP.len();
            continue;
        }
        if rest.starts_with(SELECT_OPEN) || rest.starts_with(SELECT_CLOSE) {
            let opening = rest.starts_with(SELECT_OPEN);
            flush(&mut tokens, &mut literal);
            selection_boundaries.push(tokens.len());
            selection_raw_offsets.push(index);
            let line_start = pattern[..index].rfind('\n').map_or(0, |at| at + 1);
            selection_line_starts.push(opening && pattern[line_start..index].trim().is_empty());
            index += if opening {
                SELECT_OPEN.len()
            } else {
                SELECT_CLOSE.len()
            };
            continue;
        }
        let character = rest.chars().next().expect("non-empty suffix");
        literal.push(character);
        index += character.len_utf8();
    }
    flush(&mut tokens, &mut literal);

    let mut stripped_leading = 0;
    while matches!(tokens.first(), Some(PatternToken::Gap { .. })) {
        tokens.remove(0);
        stripped_leading += 1;
    }
    let mut stripped_trailing = 0;
    while matches!(tokens.last(), Some(PatternToken::Gap { .. })) {
        tokens.pop();
        stripped_trailing += 1;
    }
    let edge_gaps = EdgeGaps {
        leading: stripped_leading > 0,
        trailing: stripped_trailing > 0,
    };
    // An edge gap spans nothing inside the match: the newline joining a
    // whole-line `…` to its neighbour belongs to the gap, not the anchor, and
    // the surviving captures renumber from zero.
    if edge_gaps.leading
        && let Some(PatternToken::Literal { text, .. }) = tokens.first_mut()
        && let Some(rest) = text.strip_prefix('\n')
    {
        *text = rest.strip_prefix('\r').unwrap_or(rest).to_owned();
    }
    if edge_gaps.trailing
        && let Some(PatternToken::Literal { text, .. }) = tokens.last_mut()
        && let Some(rest) = text.strip_suffix('\n')
    {
        *text = rest.strip_suffix('\r').unwrap_or(rest).to_owned();
    }
    for token in &mut tokens {
        if let PatternToken::Gap { capture_index, .. } = token {
            *capture_index -= stripped_leading;
        }
    }
    for boundary in &mut selection_boundaries {
        *boundary = boundary.saturating_sub(stripped_leading).min(tokens.len());
    }
    let literals = tokens
        .iter()
        .filter_map(|token| match token {
            PatternToken::Literal { normalized, .. } => Some(normalized),
            PatternToken::Gap { .. } => None,
        })
        .collect::<Vec<_>>();
    if literals.is_empty() {
        return Err(EditError::matched(format!(
            "Operation {operation_number} needs visible current text."
        )));
    }
    if !literals.iter().any(|literal| visible_identifier(literal)) {
        return Err(EditError::matched(format!(
            "Operation {operation_number} pattern is too generic; include a distinctive name or \
			 statement."
        )));
    }
    let empty_double =
        selection_boundaries.len() == 2 && selection_boundaries[0] == selection_boundaries[1];
    let insertion = selection_boundaries.len() == 1 || empty_double;
    let explicit_single = selection_boundaries.len() == 2 && !empty_double;
    let selection_start = if insertion || explicit_single {
        selection_boundaries[0]
    } else {
        0
    };
    let selection_end = if insertion {
        selection_start
    } else if explicit_single {
        selection_boundaries[1]
    } else {
        tokens.len()
    };
    let mut selection_pairs = Vec::new();
    if !selection_boundaries.is_empty() && selection_boundaries.len() % 2 == 0 {
        for pair_index in 0..selection_boundaries.len() / 2 {
            let start = selection_boundaries[pair_index * 2];
            let end = selection_boundaries[pair_index * 2 + 1];
            let capture_indices = tokens[start..end]
                .iter()
                .filter_map(|token| match token {
                    PatternToken::Gap { capture_index, .. } => Some(*capture_index),
                    PatternToken::Literal { .. } => None,
                })
                .collect();
            selection_pairs.push(SelectionPair {
                start,
                end,
                capture_indices,
                line_insertion: start == end
                    && selection_line_starts
                        .get(pair_index * 2)
                        .copied()
                        .unwrap_or(false),
                gap_only: start < end
                    && tokens[start..end]
                        .iter()
                        .all(|token| matches!(token, PatternToken::Gap { .. })),
            });
        }
    }
    let selection_ranges = if selection_pairs.len() > 1 {
        selection_pairs
            .iter()
            .map(|pair| (pair.start, pair.end))
            .collect()
    } else {
        Vec::new()
    };
    let selected_capture_indices = tokens[selection_start..selection_end]
        .iter()
        .filter_map(|token| match token {
            PatternToken::Gap { capture_index, .. } => Some(*capture_index),
            PatternToken::Literal { .. } => None,
        })
        .collect();
    let literal_fallback = if selection_ranges.is_empty() && pattern.contains(GAP) {
        let fallback = pattern.replace(SELECT_OPEN, "").replace(SELECT_CLOSE, "");
        let normalized = normalize_text(&fallback).text;
        let normalized_offset = |raw: usize| {
            normalize_text(
                &pattern[..raw]
                    .replace(SELECT_OPEN, "")
                    .replace(SELECT_CLOSE, ""),
            )
            .text
            .len()
        };
        Some(LiteralFallback {
            selection_start: if insertion || explicit_single {
                normalized_offset(selection_raw_offsets[0])
            } else {
                0
            },
            selection_end: if insertion {
                normalized_offset(selection_raw_offsets[0])
            } else if explicit_single {
                normalized_offset(selection_raw_offsets[1])
            } else {
                normalized.len()
            },
            insertion,
            normalized,
        })
    } else {
        None
    };
    Ok(ParsedPattern {
        tokens,
        edge_gaps,
        selection_start,
        selection_end,
        insertion,
        line_insertion: insertion && selection_line_starts.first().copied().unwrap_or(false),
        selected_capture_indices,
        selection_ranges,
        selection_pairs,
        literal_fallback,
    })
}
