//! Candidate location for one operation, and the recovery ladder that retries
//! a pattern whose markers garbled it.

use std::collections::HashSet;

use super::super::{
    parse::{has_marker_lines, operation_payload},
    types::{
        Candidate, Operation, OperationRewrite, ParsedPattern,
        markers::{GAP, SELECT_CLOSE, SELECT_DIVIDER, SELECT_OPEN},
    },
};

use super::{
    candidates::{MatchMode, collect_candidates},
    diagnostics::{line_number_at, no_match_error},
    occurrences::{exact_occurrences, source_end, source_start},
    pattern::{normalize_text, parse_pattern},
};

use crate::error::EditError;
use crate::fuzzy::PatternDistance;

fn same_rewrite_for_all(
    pattern: &ParsedPattern,
    operation: &Operation,
    candidates: &[Candidate],
) -> bool {
    match &operation.rewrite {
        OperationRewrite::Insert { .. } => true,
        OperationRewrite::Explicit { text } => {
            let gaps = text.matches(GAP).count();
            pattern
                .selected_capture_indices
                .iter()
                .take(gaps)
                .all(|index| {
                    candidates.iter().all(|candidate| {
                        candidate.captures[*index] == candidates[0].captures[*index]
                    })
                })
        }
        OperationRewrite::Inline { replacements } => {
            replacements
                .iter()
                .enumerate()
                .all(|(replacement_index, replacement)| {
                    pattern
                        .selection_pairs
                        .get(replacement_index)
                        .is_some_and(|pair| {
                            pair.capture_indices
                                .iter()
                                .take(replacement.matches(GAP).count())
                                .all(|index| {
                                    candidates.iter().all(|candidate| {
                                        candidate.captures[*index] == candidates[0].captures[*index]
                                    })
                                })
                        })
                })
        }
    }
}

pub(crate) fn locate(
    content: &str,
    pattern: &ParsedPattern,
    operation: &Operation,
    operation_number: usize,
    path: &str,
    exclusions: &[(usize, usize)],
    standalone_operation: bool,
) -> Result<Vec<Candidate>, EditError> {
    let normalized = normalize_text(content);
    let raw = collect_candidates(content, &normalized, pattern, MatchMode::Raw, false);
    if raw.overflow {
        return Err(EditError::matched(format!(
            "Operation {operation_number} pattern is too broad; add another distinctive {GAP} \
			 fragment."
        )));
    }
    let marker_op = has_marker_lines(&operation.source_pattern_text);
    if raw.candidates.is_empty()
        && let Some(fallback) = &pattern.literal_fallback
    {
        let exact = exact_occurrences(&normalized.text, &fallback.normalized);
        if !exact.is_empty() && (operation.all || exact.len() == 1) {
            let candidates = exact
                .into_iter()
                .map(|occurrence| {
                    let match_start = source_start(&normalized, occurrence.start, 0);
                    let match_end = source_end(&normalized, occurrence.end, content.len());
                    let fallback_start = occurrence.start + fallback.selection_start;
                    let fallback_end = occurrence.start + fallback.selection_end;
                    let start = if fallback.selection_start == fallback.normalized.len() {
                        match_end
                    } else {
                        source_start(&normalized, fallback_start, match_end)
                    };
                    let end = if fallback.selection_end == fallback.normalized.len() {
                        match_end
                    } else if fallback.insertion {
                        source_start(&normalized, fallback_end, match_end)
                    } else {
                        source_end(&normalized, fallback_end, match_end)
                    };
                    Candidate {
                        start,
                        end,
                        match_start,
                        match_end,
                        captures: Vec::new(),
                        selection_spans: (pattern.selection_pairs.len() == 1)
                            .then_some(vec![(start, end)])
                            .unwrap_or_default(),
                        tuple: vec![occurrence.start],
                        literal_gaps: true,
                    }
                })
                .collect::<Vec<_>>();
            return Ok(if operation.all {
                candidates
            } else {
                vec![candidates[0].clone()]
            });
        }
    }
    let mut result = if raw.candidates.is_empty() {
        collect_candidates(content, &normalized, pattern, MatchMode::Normalized, false)
    } else {
        raw
    };
    if result.candidates.is_empty() && !result.overflow && !marker_op {
        result = collect_candidates(content, &normalized, pattern, MatchMode::Fuzzy, false);
        if result.candidates.is_empty() && !result.overflow && !operation.all {
            let punctuation =
                collect_candidates(content, &normalized, pattern, MatchMode::Fuzzy, true);
            if !punctuation.overflow && punctuation.candidates.len() == 1 {
                result = punctuation;
            }
        }
    }
    if result.overflow {
        return Err(EditError::matched(format!(
            "Operation {operation_number} pattern is too broad; add another distinctive {GAP} \
			 fragment."
        )));
    }
    let mut candidates = result.candidates;
    if !exclusions.is_empty() && candidates.len() > 1 {
        let free = candidates
            .iter()
            .filter(|candidate| {
                !exclusions.iter().any(|(start, end)| {
                    candidate.match_start < *end && *start < candidate.match_end
                })
            })
            .cloned()
            .collect::<Vec<_>>();
        if !free.is_empty() {
            candidates = free;
        }
    }
    if operation.all && !candidates.is_empty() {
        return Ok(candidates);
    }
    if candidates.len() == 1 {
        return Ok(candidates);
    }
    if candidates.is_empty() {
        return Err(no_match_error(
            content,
            pattern,
            operation,
            operation_number,
            path,
            standalone_operation,
        ));
    }
    if candidates.len() <= 4
        && !operation.desired_state
        && !matches!(operation.rewrite, OperationRewrite::Insert { .. })
    {
        let outcomes = candidates
            .iter()
            .filter_map(|candidate| {
                Some(match &operation.rewrite {
                    OperationRewrite::Insert { .. } => return None,
                    OperationRewrite::Explicit { text } => {
                        format!(
                            "{}{}{}",
                            &content[..candidate.start],
                            text,
                            &content[candidate.end..]
                        )
                    }
                    OperationRewrite::Inline { replacements } => {
                        let mut result = content.to_owned();
                        let mut spans = candidate
                            .selection_spans
                            .iter()
                            .copied()
                            .zip(replacements)
                            .collect::<Vec<_>>();
                        spans.sort_by_key(|((start, _), _)| std::cmp::Reverse(*start));
                        for ((start, end), replacement) in spans {
                            result.replace_range(start..end, replacement);
                        }
                        result
                    }
                })
            })
            .map(|outcome| normalize_text(&outcome).text)
            .collect::<HashSet<_>>();
        if outcomes.len() == 1 {
            return Ok(vec![candidates[0].clone()]);
        }
    }
    let retries = candidates
        .iter()
        .take(2)
        .map(|candidate| {
            format!(
                "Near line {}:\n{}",
                line_number_at(content, candidate.start),
                operation_payload(operation, path, false, None)
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n");
    let all_retry = if same_rewrite_for_all(pattern, operation, &candidates) {
        format!(
            "\n\nAll candidates receive the same rewrite; retry every match:\n{}",
            operation_payload(operation, path, true, None)
        )
    } else {
        String::new()
    };
    Err(EditError::matched(format!(
        "Operation {operation_number} is ambiguous: {} ordered tuples match.\n\nAdd context that \
		 only the intended match has — one of these:\n\n{retries}{all_retry}",
        candidates.len()
    )))
}

pub(crate) fn closest_desired_block(content: &str, stated_text: &str) -> Option<String> {
    let stated = normalize_text(stated_text).text;
    if !(12..=1000).contains(&stated.len()) {
        return None;
    }
    let count = stated_text.split('\n').count();
    let lines = content.split('\n').collect::<Vec<_>>();
    if lines.len() < count {
        return None;
    }
    // normalize_text drops the joining newlines, so a window's normalized text
    // is its lines' normalized texts back to back: normalize every line once
    // and slice windows out of the result.
    let mut normalized = String::with_capacity(content.len());
    let mut line_starts = Vec::with_capacity(lines.len() + 1);
    for line in &lines {
        line_starts.push(normalized.len());
        normalized.push_str(&normalize_text(line).text);
    }
    line_starts.push(normalized.len());
    let stated_distance = PatternDistance::new(&stated.chars().collect::<Vec<_>>());
    let mut current_chars = Vec::new();
    let mut scores = Vec::new();
    for index in 0..=lines.len() - count {
        let current = &normalized[line_starts[index]..line_starts[index + count]];
        let max = stated.len().max(current.len()).max(1);
        let affix = stated.starts_with(current)
            || current.starts_with(&stated)
            || stated.ends_with(current)
            || current.ends_with(&stated);
        let score = if current.is_empty()
            || affix
            || current.len().abs_diff(stated.len()) as f64 / max as f64 > 0.35
        {
            1.0
        } else {
            current_chars.clear();
            current_chars.extend(current.chars());
            stated_distance.distance(&current_chars) as f64 / max as f64
        };
        scores.push((index, score));
    }
    let best = scores
        .iter()
        .min_by(|left, right| left.1.total_cmp(&right.1))?;
    if best.1 == 0.0 || best.1 > 0.35 {
        return None;
    }
    if scores
        .iter()
        .any(|other| other.0.abs_diff(best.0) >= count && other.1 - best.1 < 0.1)
    {
        return None;
    }
    Some(lines[best.0..best.0 + count].join("\n"))
}

pub(crate) fn is_diff_shaped(pattern_text: &str) -> bool {
    if pattern_text.contains(SELECT_OPEN)
        || pattern_text
            .lines()
            .any(|line| line.trim_start().starts_with(['＋', '－']))
    {
        return false;
    }
    let minus = pattern_text
        .lines()
        .any(|line| line.starts_with('-') && !line.starts_with("---"));
    minus
        && pattern_text.lines().any(|line| {
            (line.starts_with('+') && !line.starts_with("+++"))
                || line.trim().starts_with("@@")
                || line.starts_with(' ')
                    && line
                        .chars()
                        .nth(1)
                        .is_some_and(|character| !character.is_whitespace())
        })
}

pub(crate) fn diff_shaped_candidates(pattern_text: &str) -> Vec<String> {
    if !is_diff_shaped(pattern_text) {
        return Vec::new();
    }
    let lines = pattern_text.lines().collect::<Vec<_>>();
    let build = |strip_space: bool| {
        let mut out = Vec::new();
        let mut index = 0;
        while index < lines.len() {
            let line = lines[index];
            if line.starts_with("---") || line.starts_with("+++") {
                index += 1;
                continue;
            }
            if line.trim().starts_with("@@") {
                out.push(GAP.to_owned());
                index += 1;
                continue;
            }
            if line.starts_with('-') {
                let mut removed = Vec::new();
                while index < lines.len()
                    && lines[index].starts_with('-')
                    && !lines[index].starts_with("---")
                {
                    removed.push(&lines[index][1..]);
                    index += 1;
                }
                let mut added = Vec::new();
                while index < lines.len()
                    && lines[index].starts_with('+')
                    && !lines[index].starts_with("+++")
                {
                    added.push(&lines[index][1..]);
                    index += 1;
                }
                out.push(format!(
                    "{SELECT_OPEN}{}{SELECT_DIVIDER}{}{SELECT_CLOSE}",
                    removed.join("\n"),
                    added.join("\n")
                ));
                continue;
            }
            if line.starts_with('+') {
                let mut added = Vec::new();
                while index < lines.len()
                    && lines[index].starts_with('+')
                    && !lines[index].starts_with("+++")
                {
                    added.push(&lines[index][1..]);
                    index += 1;
                }
                if let Some(previous) = out.last_mut()
                    && previous.as_str() != GAP
                    && !previous.contains(SELECT_OPEN)
                    && !previous.trim().is_empty()
                {
                    let old = previous.clone();
                    *previous = format!(
                        "{SELECT_OPEN}{old}{SELECT_DIVIDER}{}{SELECT_CLOSE}",
                        std::iter::once(old.as_str())
                            .chain(added.iter().copied())
                            .collect::<Vec<_>>()
                            .join("\n")
                    );
                } else {
                    out.extend(added.into_iter().map(|entry| format!("＋{entry}")));
                }
                continue;
            }
            out.push(if strip_space {
                line.strip_prefix(' ').unwrap_or(line).to_owned()
            } else {
                line.to_owned()
            });
            index += 1;
        }
        out.join("\n")
    };
    let spaced = build(true);
    let plain = build(false);
    if spaced == plain {
        vec![spaced]
    } else {
        vec![spaced, plain]
    }
}

fn drop_selection_echoes(pattern: &str) -> Option<String> {
    let mut result = String::new();
    let mut run_start = 0;
    let mut changed = false;
    let mut index = 0;
    while index < pattern.len() {
        if pattern[index..].starts_with(GAP) {
            result.push_str(&pattern[run_start..index + GAP.len()]);
            index += GAP.len();
            run_start = index;
            continue;
        }
        if !pattern[index..].starts_with(SELECT_OPEN) {
            index += pattern[index..].chars().next().expect("suffix").len_utf8();
            continue;
        }
        let close =
            pattern[index + SELECT_OPEN.len()..].find(SELECT_CLOSE)? + index + SELECT_OPEN.len();
        let selected = &pattern[index + SELECT_OPEN.len()..close];
        let run = &pattern[run_start..index];
        let normalized_run = normalize_text(run);
        let old = normalize_text(selected).text;
        if !old.is_empty() && normalized_run.text.ends_with(&old) {
            let cut = normalized_run.starts[normalized_run.text.len() - old.len()];
            result.push_str(&pattern[run_start..run_start + cut]);
            changed = true;
        } else {
            result.push_str(run);
        }
        result.push_str(&pattern[index..close + SELECT_CLOSE.len()]);
        index = close + SELECT_CLOSE.len();
        run_start = index;
    }
    result.push_str(&pattern[run_start..]);
    changed.then_some(result)
}

fn echo_line_candidates(pattern: &str) -> Vec<String> {
    let lines = pattern.split('\n').collect::<Vec<_>>();
    let mut result = Vec::new();
    for index in 1..lines.len() {
        let previous = lines[index - 1];
        let line = lines[index];
        if !line.contains(SELECT_OPEN)
            || previous.contains(SELECT_OPEN)
            || previous.contains(GAP)
            || previous.trim().is_empty()
        {
            continue;
        }
        let without_markers = line.replace(SELECT_OPEN, "").replace(SELECT_CLOSE, "");
        if normalize_text(&without_markers).text == normalize_text(previous).text {
            let mut candidate = lines.clone();
            candidate.remove(index - 1);
            result.push(candidate.join("\n"));
        }
    }
    result
}

fn trailing_selection_candidate(pattern: &str) -> Option<String> {
    let mut changed = false;
    let lines = pattern
        .split('\n')
        .map(|line| {
            let Some(open) = line.rfind(SELECT_OPEN) else {
                return line.to_owned();
            };
            let Some(close_relative) = line[open + SELECT_OPEN.len()..].find(SELECT_CLOSE) else {
                return line.to_owned();
            };
            let close = open + SELECT_OPEN.len() + close_relative;
            if !line[close + SELECT_CLOSE.len()..].trim().is_empty()
                || line[..open].trim().is_empty()
                || line[..open].contains(SELECT_OPEN)
                || line[..open].contains(GAP)
            {
                return line.to_owned();
            }
            let old = &line[open + SELECT_OPEN.len()..close];
            let normalized = normalize_text(&line[..open]);
            let old_normalized = normalize_text(old).text;
            let Some(found) = normalized.text.find(&old_normalized) else {
                return line.to_owned();
            };
            if normalized.text[found + old_normalized.len()..].contains(&old_normalized) {
                return line.to_owned();
            }
            let raw_start = normalized.starts[found];
            let raw_end = normalized.ends[found + old_normalized.len() - 1];
            changed = true;
            format!(
                "{}{}{}{}{}{}",
                &line[..raw_start],
                SELECT_OPEN,
                &line[raw_start..raw_end],
                SELECT_CLOSE,
                &line[raw_end..open],
                &line[close + SELECT_CLOSE.len()..]
            )
        })
        .collect::<Vec<_>>();
    changed.then(|| lines.join("\n"))
}

fn recover_pattern_candidates(pattern: &str, inline: bool) -> Vec<String> {
    let mut result = Vec::new();
    let mut push = |candidate: Option<String>| {
        if let Some(candidate) = candidate
            && candidate != pattern
            && !result.contains(&candidate)
        {
            result.push(candidate);
        }
    };
    push(drop_selection_echoes(pattern));
    for candidate in echo_line_candidates(pattern) {
        push(Some(candidate));
    }
    let trailing = trailing_selection_candidate(pattern);
    push(trailing.clone());
    if !pattern.contains(GAP) {
        let lines = pattern
            .lines()
            .filter(|line| !line.trim().is_empty())
            .collect::<Vec<_>>();
        if lines.len() >= 2 && (inline && lines.iter().any(|line| line.contains(SELECT_OPEN))) {
            push(Some(lines.join(&format!("\n{GAP}\n"))));
        }
    }
    if let Some(trailing) = trailing
        && !trailing.contains(GAP)
    {
        let lines = trailing
            .lines()
            .filter(|line| !line.trim().is_empty())
            .collect::<Vec<_>>();
        if lines.len() >= 2 && inline && lines.iter().any(|line| line.contains(SELECT_OPEN)) {
            push(Some(lines.join(&format!("\n{GAP}\n"))));
        }
    }
    result
}

fn punctuation_pair_variants(operation: &Operation) -> Vec<Operation> {
    let OperationRewrite::Inline { replacements } = &operation.rewrite else {
        return Vec::new();
    };
    let mut variants = Vec::new();
    let mut from = 0;
    let mut selection_index = 0;
    while let Some(open_relative) = operation.pattern_text[from..].find(SELECT_OPEN) {
        let open = from + open_relative;
        let selected_start = open + SELECT_OPEN.len();
        let Some(close_relative) = operation.pattern_text[selected_start..].find(SELECT_CLOSE)
        else {
            break;
        };
        let close = selected_start + close_relative;
        let old = &operation.pattern_text[selected_start..close];
        let next = replacements.get(selection_index).map_or("", String::as_str);
        let punctuation = |text: &str| {
            !text.is_empty()
                && text.chars().all(|character| {
                    !(character.is_alphanumeric()
                        || character.is_whitespace()
                        || matches!(character, '_' | '$'))
                })
        };
        if punctuation(old) && punctuation(next) {
            let mut characters = old.chars().chain(next.chars()).collect::<Vec<_>>();
            characters.sort_unstable();
            characters.dedup();
            if characters.len() == 2 {
                for length in [2, 1, 3] {
                    for (left, right) in [
                        (characters[0], characters[1]),
                        (characters[1], characters[0]),
                    ] {
                        let old_run = left.to_string().repeat(length);
                        let next_run = right.to_string().repeat(length);
                        if old_run == old && next_run == next {
                            continue;
                        }
                        let mut pattern_text = operation.pattern_text.clone();
                        pattern_text.replace_range(selected_start..close, &old_run);
                        let mut next_replacements = replacements.clone();
                        next_replacements[selection_index] = next_run;
                        let mut variant = operation.clone();
                        variant.pattern_text = pattern_text;
                        variant.rewrite = OperationRewrite::Inline {
                            replacements: next_replacements,
                        };
                        variants.push(variant);
                    }
                }
            }
        }
        selection_index += 1;
        from = close + SELECT_CLOSE.len();
    }
    variants
}

pub(super) fn locate_with_recovery(
    content: &str,
    operation: &Operation,
    number: usize,
    path: &str,
    exclusions: &[(usize, usize)],
    standalone: bool,
) -> Result<(Operation, ParsedPattern, Vec<Candidate>), EditError> {
    let pattern = parse_pattern(&operation.pattern_text, number)?;
    match locate(
        content, &pattern, operation, number, path, exclusions, standalone,
    ) {
        Ok(candidates) => {
            let mut resolved = operation.clone();
            if has_marker_lines(&operation.source_pattern_text) {
                let normalized = normalize_text(content);
                let raw = collect_candidates(content, &normalized, &pattern, MatchMode::Raw, false);
                let lenient = raw.candidates.is_empty()
                    && !collect_candidates(
                        content,
                        &normalized,
                        &pattern,
                        MatchMode::Normalized,
                        false,
                    )
                    .candidates
                    .is_empty();
                resolved.whitespace_matched = lenient;
            }
            Ok((resolved, pattern, candidates))
        }
        Err(original) => {
            for candidate in recover_pattern_candidates(
                &operation.pattern_text,
                matches!(operation.rewrite, OperationRewrite::Inline { .. }),
            ) {
                let Ok(pattern) = parse_pattern(&candidate, number) else {
                    continue;
                };
                let mut recovered = operation.clone();
                recovered.pattern_text = candidate;
                if let Ok(candidates) = locate(
                    content, &pattern, &recovered, number, path, exclusions, standalone,
                ) {
                    return Ok((recovered, pattern, candidates));
                }
            }
            for mut variant in punctuation_pair_variants(operation) {
                let Ok(pattern) = parse_pattern(&variant.pattern_text, number) else {
                    continue;
                };
                if let Ok(candidates) = locate(
                    content, &pattern, &variant, number, path, exclusions, standalone,
                ) {
                    let partial_run = candidates.iter().any(|candidate| {
                        candidate.selection_spans.iter().any(|(start, end)| {
                            if start >= end {
                                return false;
                            }
                            let character = content[*start..]
                                .chars()
                                .next()
                                .expect("selected character");
                            content[..*start].ends_with(character)
                                || content[*end..].starts_with(character)
                        })
                    });
                    if partial_run {
                        continue;
                    }
                    variant.recovery_note = Some(format!(
                        "Note: operation {number}'s punctuation selection was garbled by its own marker \
						 glyphs and was resolved against the file. Include a neighboring character next \
						 time (e.g. i{SELECT_OPEN}++){SELECT_DIVIDER}--){SELECT_CLOSE})."
                    ));
                    return Ok((variant, pattern, candidates));
                }
            }
            Err(original)
        }
    }
}
