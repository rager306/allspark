//! The op stream's per-operation half: marker handling, the recovery ladder one
//! `*** Find`/`*** Replace` pair can need, and operation assembly.
//!
//! The walk itself lives in [`super::sections::parse_section`], which calls
//! [`create_operation`], [`finish_operation`] and [`finish_pattern`] here. Those
//! two finishing passes render copy-ready payloads back out through
//! [`super::sections::ir_to_payload`].

use std::collections::{HashMap, HashSet};

use super::super::apply::{
    closest_desired_block, diff_shaped_candidates, is_diff_shaped, locate, normalize_text,
    parse_pattern,
};
use super::super::types::{Operation, OperationRewrite};
use crate::error::EditError;

use super::sections::{ir_to_payload, normalize_block};
use super::{
    ADD_LINE, BARE_DESIRED_CAPTURE_RE, BARE_DESIRED_RE, DIRECTIVE_RE, EMPTY_SELECTION_TAIL_RE, GAP,
    LITERAL_CLOSE, LITERAL_DIVIDER, LITERAL_OPEN, REFERENCE_RE, REMOVE_LINE, REWRITE_HEADER,
    SELECT_CLOSE, SELECT_DIVIDER, SELECT_OPEN, SELECTION_ONLY_RE, STRAY_DIVIDER_RE,
    UNICODE_TOKEN_RE, js_len, parse_error,
};

fn recover_missing_separator(lines: &[String], content: &str) -> Option<(String, String)> {
    if is_diff_shaped(&lines.join("\n")) {
        return None;
    }
    let mut candidates = Vec::new();
    for split in 1..lines.len() {
        let mut remainder_start = split;
        while lines
            .get(remainder_start)
            .is_some_and(|line| line.trim().is_empty())
        {
            remainder_start += 1;
        }
        if remainder_start >= lines.len() {
            continue;
        }
        let pattern = normalize_block(&lines[..split], false);
        let rewrite = normalize_block(&lines[remainder_start..], true);
        if js_len(&pattern) < 4 || rewrite.replace(GAP, "").trim().is_empty() {
            continue;
        }
        if rewrite
            .split('\n')
            .any(|line| !line.trim().is_empty() && line.trim().replace(GAP, "").is_empty())
        {
            continue;
        }
        let occurrences: Vec<usize> = content.match_indices(&pattern).map(|(at, _)| at).collect();
        if occurrences.len() != 1 {
            continue;
        }
        let through_first_rewrite = normalize_block(&lines[..=remainder_start], false);
        if content[occurrences[0]..].starts_with(&through_first_rewrite) {
            continue;
        }
        if !candidates
            .iter()
            .any(|candidate: &(String, String)| candidate.0 == pattern && candidate.1 == rewrite)
        {
            candidates.push((pattern, rewrite));
        }
    }
    (candidates.len() == 1).then(|| candidates.remove(0))
}

fn has_inline_selection(pattern: &str) -> bool {
    let mut selected = false;
    for character in pattern.chars() {
        match character.to_string().as_str() {
            SELECT_OPEN => selected = true,
            SELECT_CLOSE => selected = false,
            SELECT_DIVIDER if selected => return true,
            _ => {}
        }
    }
    false
}

fn validate_selection_markers(pattern: &str, operation_number: usize) -> Result<(), EditError> {
    let opens = pattern.matches(SELECT_OPEN).count();
    let closes = pattern.matches(SELECT_CLOSE).count();
    if opens == closes {
        return Ok(());
    }
    Err(parse_error(if opens > closes {
        format!("Operation {operation_number} has an unclosed selection marker ⟪; add closing ⟫.")
    } else {
        format!(
            "Operation {operation_number} has an unmatched closing selection marker ⟫; add opening ⟪."
        )
    }))
}

fn resolve_literal_dividers(selected: &str, operation_number: usize) -> (String, String, String) {
    let dividers: Vec<usize> = selected
        .match_indices(SELECT_DIVIDER)
        .map(|(at, _)| at)
        .collect();
    let advice = format!(
        "Selections containing literal {SELECT_DIVIDER} are ambiguous; state those lines with a \
		 {REWRITE_HEADER} block rewrite instead."
    );
    let last = *dividers.last().expect("called with multiple dividers");
    if last + SELECT_DIVIDER.len() == selected.len() {
        return (
            selected[..last].to_owned(),
            String::new(),
            format!(
                "Note: operation {operation_number}'s selection contained {} {SELECT_DIVIDER} \
				 characters and ended with one; it was read as a deletion of the selected text with \
				 the inner {SELECT_DIVIDER}s literal. {advice}",
                dividers.len()
            ),
        );
    }
    if dividers.len() % 2 == 1 {
        let middle = dividers[(dividers.len() - 1) / 2];
        return (
            selected[..middle].to_owned(),
            selected[middle + SELECT_DIVIDER.len()..].to_owned(),
            format!(
                "Note: operation {operation_number}'s selection contained {} {SELECT_DIVIDER} \
				 characters; the middle one was read as the divider and the others as literal text. \
				 {advice}",
                dividers.len()
            ),
        );
    }
    (
        selected.to_owned(),
        String::new(),
        format!(
            "Note: operation {operation_number}'s selection contained {} {SELECT_DIVIDER} characters \
			 with no unambiguous divider; it was read as a deletion of the selected text. {advice}",
            dividers.len()
        ),
    )
}

fn parse_inline_pattern(
    pattern: &str,
    operation_number: usize,
) -> Result<(String, Vec<String>, Vec<String>), EditError> {
    validate_selection_markers(pattern, operation_number)?;
    let mut pattern_text = String::new();
    let mut replacements = Vec::new();
    let mut notes = Vec::new();
    let mut saw_bare = false;
    let mut saw_inline = false;
    let mut index = 0;
    while index < pattern.len() {
        if pattern[index..].starts_with(SELECT_CLOSE) {
            return Err(parse_error(format!(
                "Operation {operation_number} has an unmatched closing selection marker ⟫; add \
				 opening ⟪."
            )));
        }
        if !pattern[index..].starts_with(SELECT_OPEN) {
            let character = pattern[index..]
                .chars()
                .next()
                .expect("character at boundary");
            pattern_text.push(character);
            index += character.len_utf8();
            continue;
        }
        let selected_start = index + SELECT_OPEN.len();
        let close_relative = pattern[selected_start..]
            .find(SELECT_CLOSE)
            .expect("validated close marker");
        let close = selected_start + close_relative;
        let selected = &pattern[selected_start..close];
        if selected.contains(SELECT_OPEN) {
            return Err(parse_error(format!(
                "Operation {operation_number} has nested selection markers; use one selection per \
				 replacement."
            )));
        }
        if let Some(divider) = selected.find(SELECT_DIVIDER) {
            saw_inline = true;
            if selected[divider + SELECT_DIVIDER.len()..].contains(SELECT_DIVIDER) {
                let (old, desired, note) = resolve_literal_dividers(selected, operation_number);
                pattern_text.push_str(SELECT_OPEN);
                pattern_text.push_str(&old);
                pattern_text.push_str(SELECT_CLOSE);
                replacements.push(desired);
                notes.push(note);
            } else {
                pattern_text.push_str(SELECT_OPEN);
                pattern_text.push_str(&selected[..divider]);
                pattern_text.push_str(SELECT_CLOSE);
                replacements.push(selected[divider + SELECT_DIVIDER.len()..].to_owned());
            }
        } else {
            saw_bare = true;
            pattern_text.push_str(&pattern[index..close + SELECT_CLOSE.len()]);
        }
        index = close + SELECT_CLOSE.len();
    }
    if saw_bare && saw_inline {
        return Err(parse_error(format!(
            "Operation {operation_number} mixes inline and bare selections. Use \
			 {SELECT_OPEN}old{SELECT_DIVIDER}new{SELECT_CLOSE} for every selection, or use a \
			 {REWRITE_HEADER} rewrite for all bare selections."
        )));
    }
    Ok((pattern_text, replacements, notes))
}

fn relocate_selection_lines(pattern_text: &str) -> String {
    let mut lines: Vec<String> = pattern_text.split('\n').map(str::to_owned).collect();
    let mut index = 1;
    while index < lines.len() {
        let previous = lines[index - 1].clone();
        let Some(captures) = SELECTION_ONLY_RE.captures(lines[index].trim()) else {
            index += 1;
            continue;
        };
        if previous.contains(SELECT_OPEN) || previous.contains(GAP) || previous.trim().is_empty() {
            index += 1;
            continue;
        }
        let old = normalize_text(captures.get(1).map_or("", |value| value.as_str())).text;
        if old.is_empty() {
            index += 1;
            continue;
        }
        let normalized = normalize_text(&previous);
        let Some(first) = normalized.text.find(&old) else {
            index += 1;
            continue;
        };
        if normalized.text[first + old.len()..].contains(&old) || normalized.text == old {
            index += 1;
            continue;
        }
        let raw_start = normalized.starts.get(first).copied().unwrap_or(0);
        let raw_end = normalized
            .ends
            .get(first + old.len() - 1)
            .copied()
            .unwrap_or(previous.len());
        lines[index - 1] = format!(
            "{}{}{}",
            &previous[..raw_start],
            lines[index].trim(),
            &previous[raw_end..]
        );
        lines.remove(index);
        index = index.saturating_sub(1).max(1);
    }
    lines.join("\n")
}

fn pattern_reference(pattern: &str) -> Option<String> {
    pattern.split('\n').find_map(|line| {
        REFERENCE_RE
            .captures(line.trim())
            .map(|captures| captures[1].to_owned())
    })
}

fn has_bare_desired(pattern_text: &str) -> bool {
    BARE_DESIRED_RE.is_match(pattern_text)
}

fn embed_bare_desired(pattern_text: &str) -> String {
    BARE_DESIRED_CAPTURE_RE
        .replace_all(pattern_text, "⟪…│$1⟫")
        .into_owned()
}

fn expand_echoed_line_selection(pattern_text: &str, rewrite_text: &str) -> Option<String> {
    let rewrite_lines: Vec<&str> = rewrite_text
        .split('\n')
        .filter(|line| !line.trim().is_empty())
        .collect();
    if rewrite_lines.len() != 1 {
        return None;
    }
    let mut lines: Vec<String> = pattern_text.split('\n').map(str::to_owned).collect();
    let index = lines.iter().position(|line| line.contains(SELECT_OPEN))?;
    let line = lines[index].clone();
    if line.contains(GAP) {
        return None;
    }
    let open = line.find(SELECT_OPEN)?;
    let close = line[open + SELECT_OPEN.len()..].find(SELECT_CLOSE)? + open + SELECT_OPEN.len();
    if line[open + SELECT_OPEN.len()..]
        .matches(SELECT_OPEN)
        .count()
        > 0
        || lines
            .iter()
            .enumerate()
            .any(|(at, entry)| at != index && entry.contains(SELECT_OPEN))
    {
        return None;
    }
    let prefix = &line[..open];
    let suffix = &line[close + SELECT_CLOSE.len()..];
    if prefix.trim().is_empty() || suffix.contains(SELECT_CLOSE) {
        return None;
    }
    let selected = &line[open + SELECT_OPEN.len()..close];
    if selected.contains(SELECT_DIVIDER)
        || !normalize_text(rewrite_lines[0])
            .text
            .starts_with(&normalize_text(prefix).text)
    {
        return None;
    }
    lines[index] = format!("{SELECT_OPEN}{prefix}{selected}{suffix}{SELECT_CLOSE}");
    Some(lines.join("\n"))
}

/// Strip a full-line marker while retaining indentation.
pub fn marker_line_content(line: &str, marker: &str) -> Option<String> {
    let indent_len = line.len() - line.trim_start_matches([' ', '\t']).len();
    line[indent_len..].starts_with(marker).then(|| {
        format!(
            "{}{}",
            &line[..indent_len],
            &line[indent_len + marker.len()..]
        )
    })
}

/// Whether a pattern contains any full-line add/remove markers.
pub fn has_marker_lines(pattern_text: &str) -> bool {
    (pattern_text.contains(ADD_LINE) || pattern_text.contains(REMOVE_LINE))
        && pattern_text.split('\n').any(|line| {
            marker_line_content(line, ADD_LINE).is_some()
                || marker_line_content(line, REMOVE_LINE).is_some()
        })
}

pub(crate) fn missing_unmarked_lines(content: &str, source_pattern_text: &str) -> Vec<String> {
    let haystack = normalize_text(content).text;
    let mut missing = Vec::new();
    for line in source_pattern_text.split('\n') {
        if marker_line_content(line, ADD_LINE).is_some()
            || marker_line_content(line, REMOVE_LINE).is_some()
            || line.contains(SELECT_OPEN)
            || line.contains(SELECT_CLOSE)
        {
            continue;
        }
        let fragments: Vec<String> = line
            .split(GAP)
            .map(|fragment| normalize_text(fragment).text)
            .filter(|fragment| !fragment.is_empty())
            .collect();
        if !fragments.is_empty()
            && fragments
                .iter()
                .any(|fragment| !haystack.contains(fragment))
        {
            missing.push(line.trim().to_owned());
        }
    }
    missing
}

fn is_near_variant(anchor: &str, added: &str) -> bool {
    if anchor.contains(SELECT_OPEN) || anchor.trim().is_empty() {
        return false;
    }
    let left = anchor.trim();
    let right = added.trim();
    if left == right || js_len(left) < 4 || js_len(right) < 4 {
        return false;
    }

    let tokens = |line: &str| {
        let mut counts = HashMap::new();
        for token in UNICODE_TOKEN_RE.find_iter(line).map(|item| item.as_str()) {
            *counts.entry(token.to_owned()).or_insert(0usize) += 1;
        }
        counts
    };
    let left_tokens = tokens(left);
    let right_tokens = tokens(right);
    let left_total: usize = left_tokens.values().sum();
    let right_total: usize = right_tokens.values().sum();
    if left_total == 0 || right_total == 0 {
        return false;
    }
    let shared: usize = left_tokens
        .iter()
        .map(|(token, count)| (*count).min(right_tokens.get(token).copied().unwrap_or(0)))
        .sum();
    (2.0 * shared as f64) / (left_total + right_total) as f64 >= 0.8
}

fn encode_literal_markers(text: &str) -> String {
    text.replace(SELECT_OPEN, LITERAL_OPEN)
        .replace(SELECT_CLOSE, LITERAL_CLOSE)
        .replace(SELECT_DIVIDER, LITERAL_DIVIDER)
}

pub(in crate::modes::sloppy) fn decode_literal_markers(text: &str) -> String {
    text.replace(LITERAL_OPEN, SELECT_OPEN)
        .replace(LITERAL_CLOSE, SELECT_CLOSE)
        .replace(LITERAL_DIVIDER, SELECT_DIVIDER)
}

fn wrap_trailing_anchor(out: &mut [String], added: &[String]) -> bool {
    let Some(previous) = out.last().cloned() else {
        return false;
    };
    let close = previous.rfind(SELECT_CLOSE);
    let (head, tail) = close.map_or(("", previous.as_str()), |at| {
        previous.split_at(at + SELECT_CLOSE.len())
    });
    if tail.trim().is_empty()
        || tail.contains(GAP)
        || tail.contains(SELECT_DIVIDER)
        || tail.contains(SELECT_OPEN)
    {
        return false;
    }
    *out.last_mut().expect("non-empty output") = format!(
        "{head}{SELECT_OPEN}{tail}{SELECT_DIVIDER}{tail}\n{}{SELECT_CLOSE}",
        added.join("\n")
    );
    true
}

fn embed_marker_lines(pattern_text: &str) -> String {
    if !has_marker_lines(pattern_text) {
        return pattern_text.to_owned();
    }
    let lines: Vec<&str> = pattern_text.split('\n').collect();
    let mut out: Vec<String> = Vec::new();
    let mut index = 0;
    while index < lines.len() {
        let mut removed = Vec::new();
        while index < lines.len() {
            let Some(line) = marker_line_content(lines[index], REMOVE_LINE) else {
                break;
            };
            removed.push(line);
            index += 1;
        }
        let mut added = Vec::new();
        while index < lines.len() {
            let Some(line) = marker_line_content(lines[index], ADD_LINE) else {
                break;
            };
            added.push(encode_literal_markers(&line));
            index += 1;
        }
        if !removed.is_empty() {
            out.push(format!(
                "{SELECT_OPEN}{}{SELECT_DIVIDER}{}{SELECT_CLOSE}",
                removed.join("\n"),
                added.join("\n")
            ));
            continue;
        }
        if added.is_empty() {
            out.push(lines[index].to_owned());
            index += 1;
            continue;
        }
        if added.len() == 1
            && out
                .last()
                .is_some_and(|anchor| is_near_variant(anchor, &added[0]))
        {
            let anchor = out.pop().expect("checked anchor");
            out.push(format!(
                "{SELECT_OPEN}{anchor}{SELECT_DIVIDER}{}{SELECT_CLOSE}",
                added[0]
            ));
            continue;
        }
        let next = lines.get(index).copied();
        let mut lookahead = index;
        while lines
            .get(lookahead)
            .is_some_and(|line| line.trim().is_empty())
        {
            lookahead += 1;
        }
        let gap_below = lines.get(lookahead).is_some_and(|line| line.trim() == GAP);
        if let Some(next) = next {
            if gap_below && wrap_trailing_anchor(&mut out, &added) {
                out.push(next.to_owned());
            } else {
                out.push(format!(
                    "{SELECT_OPEN}{SELECT_DIVIDER}{}\n{SELECT_CLOSE}{next}",
                    added.join("\n")
                ));
            }
            index += 1;
        } else {
            out.push(format!(
                "{SELECT_OPEN}{SELECT_DIVIDER}{}\n{SELECT_CLOSE}",
                added.join("\n")
            ));
        }
    }
    out.join("\n")
}

fn recover_stray_close_divider(pattern_text: &str) -> Option<String> {
    let opens = pattern_text.matches(SELECT_OPEN).count();
    let closes = pattern_text.matches(SELECT_CLOSE).count();
    if closes <= opens {
        return None;
    }

    let repaired = STRAY_DIVIDER_RE
        .replace_all(pattern_text, "⟪$1│$2⟫")
        .into_owned();
    if repaired == pattern_text
        || repaired.matches(SELECT_OPEN).count() != repaired.matches(SELECT_CLOSE).count()
    {
        None
    } else {
        Some(repaired)
    }
}

fn recover_directive_rewrite(
    pattern_text: &str,
    rewrite_text: &str,
    all: bool,
    operation_number: usize,
) -> Result<Option<Operation>, EditError> {
    let lines: Vec<&str> = rewrite_text
        .split('\n')
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect();
    if lines.is_empty() {
        return Ok(None);
    }
    let mut unique = Vec::<(String, String)>::new();
    let mut seen = HashSet::new();
    for line in lines {
        let Some(directive) = DIRECTIVE_RE.captures(line) else {
            return Ok(None);
        };
        if seen.insert(directive[1].to_owned()) {
            unique.push((directive[1].to_owned(), directive[2].to_owned()));
        }
    }
    unique.sort_by_key(|entry| std::cmp::Reverse(js_len(&entry.0)));
    let mut segments = vec![(pattern_text.to_owned(), false)];
    let mut matched_any = false;
    for (old, next) in unique {
        let mut rewritten = Vec::new();
        for (segment, locked) in segments {
            if locked {
                rewritten.push((segment, true));
                continue;
            }
            let mut from = 0;
            while let Some(relative) = segment[from..].find(&old) {
                let at = from + relative;
                if at > from {
                    rewritten.push((segment[from..at].to_owned(), false));
                }
                rewritten.push((
                    format!("{SELECT_OPEN}{old}{SELECT_DIVIDER}{next}{SELECT_CLOSE}"),
                    true,
                ));
                matched_any = true;
                from = at + old.len();
            }
            if from < segment.len() {
                rewritten.push((segment[from..].to_owned(), false));
            }
        }
        segments = rewritten;
    }
    if !matched_any {
        return Ok(None);
    }
    let marked: String = segments.into_iter().map(|segment| segment.0).collect();
    let mut operation = create_operation_text(&marked, "", all, operation_number, false)?;
    operation.recovery_note = Some(format!(
        "Note: operation {operation_number}'s REWRITE listed ⟪old│new⟫ directives; they were \
		 applied to every occurrence inside the MATCH. Put selections in the MATCH itself, or state \
		 the final text in REWRITE."
    ));
    Ok(Some(operation))
}

fn recover_mixed_rewrite_forms(
    source_pattern_text: &str,
    rewrite_text: &str,
    all: bool,
    operation_number: usize,
) -> Result<Operation, EditError> {
    let (inline_pattern, replacements, inline_notes) =
        parse_inline_pattern(source_pattern_text, operation_number)?;
    let mut current = String::new();
    let mut desired = String::new();
    let mut replacement_index = 0;
    let mut index = 0;
    while let Some(relative) = inline_pattern[index..].find(SELECT_OPEN) {
        let open = index + relative;
        let close = inline_pattern[open + SELECT_OPEN.len()..]
            .find(SELECT_CLOSE)
            .expect("inline pattern was validated")
            + open
            + SELECT_OPEN.len();
        let between = &inline_pattern[index..open];
        let selected = &inline_pattern[open + SELECT_OPEN.len()..close];
        current.push_str(between);
        current.push_str(selected);
        desired.push_str(between);
        desired.push_str(
            replacements
                .get(replacement_index)
                .map_or(selected, String::as_str),
        );
        replacement_index += 1;
        index = close + SELECT_CLOSE.len();
    }
    current.push_str(&inline_pattern[index..]);
    desired.push_str(&inline_pattern[index..]);
    let normalized_rewrite = normalize_text(rewrite_text).text;
    let normalized_desired = normalize_text(&desired).text;
    let rewrite_lines: Vec<String> = rewrite_text
        .split('\n')
        .map(|line| normalize_text(line).text)
        .filter(|line| !line.is_empty())
        .collect();
    let echoes_desired = !rewrite_lines.is_empty()
        && rewrite_lines.len() == replacements.len()
        && rewrite_lines
            .iter()
            .zip(&replacements)
            .all(|(line, replacement)| *line == normalize_text(replacement).text);
    let redundant = normalized_rewrite.is_empty()
        || normalized_rewrite == normalized_desired
        || normalized_rewrite == normalize_text(&current).text
        || normalized_desired.contains(&normalized_rewrite)
        || echoes_desired;
    let mixed = format!(
        "Note: operation {operation_number} combined \
		 {SELECT_OPEN}current{SELECT_DIVIDER}desired{SELECT_CLOSE} replacements with a \
		 {REWRITE_HEADER} REWRITE"
    );
    if redundant {
        let mut operation =
            create_operation_text(source_pattern_text, "", all, operation_number, false)?;
        let mut notes = inline_notes;
        notes.push(format!(
			"{mixed}; the REWRITE only restated the inline result and was ignored. Use one form per \
			 operation."
		));
        operation.recovery_note = Some(notes.join("\n"));
        Ok(operation)
    } else {
        let mut operation =
            create_operation_text(&current, rewrite_text, all, operation_number, true)?;
        let mut notes = inline_notes;
        notes.push(format!(
			"{mixed}; the {REWRITE_HEADER} REWRITE was applied as the final text for the match. Use \
			 one form per operation."
		));
        operation.recovery_note = Some(notes.join("\n"));
        Ok(operation)
    }
}

fn create_operation_text(
    source_pattern_text: &str,
    rewrite_text: &str,
    all: bool,
    operation_number: usize,
    has_explicit_rewrite: bool,
) -> Result<Operation, EditError> {
    let mut embedded = embed_marker_lines(source_pattern_text);
    if let Some(repaired) = recover_stray_close_divider(&embedded) {
        let mut operation = create_operation_text(
            &repaired,
            rewrite_text,
            all,
            operation_number,
            has_explicit_rewrite,
        )?;
        source_pattern_text.clone_into(&mut operation.source_pattern_text);
        let note = format!(
            "Note: operation {operation_number} wrote {SELECT_CLOSE} where the {SELECT_DIVIDER} \
			 divider belongs; {SELECT_OPEN}old{SELECT_CLOSE}new{SELECT_CLOSE} was read as \
			 {SELECT_OPEN}old{SELECT_DIVIDER}new{SELECT_CLOSE}."
        );
        operation.recovery_note = Some(
            operation
                .recovery_note
                .map_or_else(|| note.clone(), |prior| format!("{note}\n{prior}")),
        );
        return Ok(operation);
    }
    if !has_explicit_rewrite && has_bare_desired(&embedded) {
        embedded = embed_bare_desired(&embedded);
    }
    if !has_inline_selection(&embedded)
        && has_explicit_rewrite
        && let Some(operation) =
            recover_directive_rewrite(&embedded, rewrite_text, all, operation_number)?
    {
        return Ok(operation);
    }
    if has_inline_selection(&embedded) {
        if has_explicit_rewrite {
            return recover_mixed_rewrite_forms(&embedded, rewrite_text, all, operation_number);
        }
        let (inline_pattern, replacements, notes) =
            parse_inline_pattern(&embedded, operation_number)?;
        if replacements.len() == 1
            && replacements[0].is_empty()
            && let Some(tail) = EMPTY_SELECTION_TAIL_RE.find(&inline_pattern)
        {
            let remainder = &inline_pattern[..tail.start()];
            if !normalize_text(remainder).text.is_empty() {
                return Ok(Operation {
                    pattern_text: remainder.to_owned(),
                    source_pattern_text: source_pattern_text.to_owned(),
                    rewrite: OperationRewrite::Explicit {
                        text: String::new(),
                    },
                    all,
                    assumed_deletion: false,
                    desired_state: false,
                    recovery_note: (!notes.is_empty()).then(|| notes.join("\n")),
                    whitespace_matched: false,
                });
            }
        }
        if let Some(reference) = pattern_reference(&inline_pattern) {
            return Err(parse_error(format!(
                "{REWRITE_HEADER}{reference} is valid only inside an inline replacement or REWRITE, \
				 never MATCH."
            )));
        }
        return Ok(Operation {
            pattern_text: relocate_selection_lines(&inline_pattern),
            source_pattern_text: source_pattern_text.to_owned(),
            rewrite: OperationRewrite::Inline { replacements },
            all,
            assumed_deletion: false,
            desired_state: false,
            recovery_note: (!notes.is_empty()).then(|| notes.join("\n")),
            whitespace_matched: false,
        });
    }
    if let Some(reference) = pattern_reference(source_pattern_text) {
        return Err(parse_error(format!(
            "{REWRITE_HEADER}{reference} is valid only in REWRITE, never MATCH."
        )));
    }
    if !rewrite_text.trim().is_empty()
        && let Some(echoed) = expand_echoed_line_selection(source_pattern_text, rewrite_text)
    {
        return Ok(Operation {
            pattern_text: echoed,
            source_pattern_text: source_pattern_text.to_owned(),
            rewrite: OperationRewrite::Explicit {
                text: rewrite_text.to_owned(),
            },
            all,
            assumed_deletion: false,
            desired_state: false,
            recovery_note: Some(format!(
                "Note: operation {operation_number}'s REWRITE restated the whole selection-bearing \
				 line, so the full line was replaced. A REWRITE after a bare selection replaces only \
				 the selected span; state just the span's new text, or select the whole line."
            )),
            whitespace_matched: false,
        });
    }
    Ok(Operation {
        pattern_text: source_pattern_text.to_owned(),
        source_pattern_text: source_pattern_text.to_owned(),
        rewrite: OperationRewrite::Explicit {
            text: rewrite_text.to_owned(),
        },
        all,
        assumed_deletion: false,
        desired_state: false,
        recovery_note: None,
        whitespace_matched: false,
    })
}

/// Build and validate one operation from its pattern and rewrite.
pub fn create_operation(
    source_pattern_text: &str,
    rewrite: OperationRewrite,
    all: bool,
    _content: &str,
    operation_number: usize,
) -> Result<Operation, EditError> {
    match rewrite {
        OperationRewrite::Explicit { text } => {
            create_operation_text(source_pattern_text, &text, all, operation_number, true)
        }
        OperationRewrite::Insert { ref text, at } if text.is_empty() => Err(parse_error(format!(
            "Operation {operation_number} has an empty {}; include the new lines to insert.",
            at.header()
        ))),
        OperationRewrite::Insert { .. } | OperationRewrite::Inline { .. } => Ok(Operation {
            pattern_text: source_pattern_text.to_owned(),
            source_pattern_text: source_pattern_text.to_owned(),
            rewrite,
            all,
            assumed_deletion: false,
            desired_state: false,
            recovery_note: None,
            whitespace_matched: false,
        }),
    }
}

pub(super) fn finish_operation(
    lines: &[String],
    end_index: usize,
    path: &str,
    pattern_lines: &[String],
    rewrite_lines: &[String],
    reference_separator: Option<&str>,
    all: bool,
    operations: &mut Vec<Operation>,
) -> Result<(), EditError> {
    let source = normalize_block(pattern_lines, false);
    let rewrite = normalize_block(rewrite_lines, true);
    if let Some(reference) = reference_separator.filter(|_| rewrite.trim().is_empty()) {
        if !has_inline_selection(&source) {
            let mut corrected = lines.to_vec();
            if let Some(separator_index) = corrected[..end_index]
                .iter()
                .rposition(|line| line.trim() == reference)
            {
                REWRITE_HEADER.clone_into(&mut corrected[separator_index]);
                corrected.insert(end_index, "{final text}".to_owned());
            }
            return Err(parse_error(format!(
                "{reference} after *** Find reads as the *** Replace separator, leaving *** Replace \
				 empty.\nCopy-ready corrected payload (fill in the final text):\n{}",
                ir_to_payload(
                    &corrected.iter().map(String::as_str).collect::<Vec<_>>(),
                    path
                )
            )));
        }
        operations.push(create_operation_text(
            &source,
            "",
            all,
            operations.len() + 1,
            false,
        )?);
        return Ok(());
    }
    if !has_inline_selection(&source) && !has_marker_lines(&source) {
        let mut body = rewrite_lines.to_vec();
        while body.first().is_some_and(|line| line.trim().is_empty()) {
            body.remove(0);
        }
        while body.last().is_some_and(|line| line.trim().is_empty()) {
            body.pop();
        }
        let add_only = !normalize_text(&source).text.is_empty()
            && body
                .iter()
                .any(|line| marker_line_content(line, ADD_LINE).is_some())
            && body.iter().all(|line| {
                line.trim().is_empty() || marker_line_content(line, ADD_LINE).is_some()
            });
        if add_only {
            let insertion: Vec<String> = body
                .iter()
                .map(|line| {
                    if line.trim().is_empty() {
                        ADD_LINE.to_owned()
                    } else {
                        line.clone()
                    }
                })
                .collect();
            let number = operations.len() + 1;
            let mut operation = create_operation_text(
                &format!("{source}\n{}", insertion.join("\n")),
                "",
                all,
                number,
                false,
            )?;
            let note = format!(
                "Note: operation {number}'s REWRITE contained only {ADD_LINE} add lines; they were \
				 inserted after the kept MATCH. Use *** Insert After with only the new lines to \
				 insert after *** Find; *** Replace replaces the match."
            );
            operation.recovery_note = Some(
                operation
                    .recovery_note
                    .map_or_else(|| note.clone(), |prior| format!("{note}\n{prior}")),
            );
            operations.push(operation);
            return Ok(());
        }
    }
    let number = operations.len() + 1;
    operations.push(create_operation_text(&source, &rewrite, all, number, true)?);
    Ok(())
}

pub(super) fn finish_pattern(
    lines: &[String],
    end_index: usize,
    content: &str,
    path: &str,
    pattern_lines: &[String],
    all: bool,
    streaming: bool,
    operations: &mut Vec<Operation>,
    pending: &mut Vec<(usize, String)>,
) -> Result<(), EditError> {
    let source = normalize_block(pattern_lines, false);
    let number = operations.len() + 1;
    if has_inline_selection(&source) || has_marker_lines(&source) || has_bare_desired(&source) {
        operations.push(create_operation_text(&source, "", all, number, false)?);
        return Ok(());
    }
    // A Find whose Replace has not streamed in yet: the recovery ladder below
    // scans the whole file, and its answer would be discarded anyway.
    if streaming {
        return Ok(());
    }
    if let Some((pattern, rewrite)) = recover_missing_separator(pattern_lines, content) {
        operations.push(create_operation_text(
            &pattern, &rewrite, all, number, true,
        )?);
        return Ok(());
    }
    let mut diff_fallback = None;
    for candidate in diff_shaped_candidates(&source) {
        let Ok(mut operation) = create_operation_text(&candidate, "", all, number, false) else {
            continue;
        };
        operation.recovery_note = Some(format!(
            "Note: operation {number} was written as a unified diff and was applied as inline \
			 changes; state edits as *** Find / *** Replace pairs instead."
        ));
        let located = parse_pattern(&operation.pattern_text, number)
            .and_then(|pattern| locate(content, &pattern, &operation, number, "", &[], true));
        if located.is_ok() {
            operations.push(operation);
            return Ok(());
        }
        diff_fallback.get_or_insert(operation);
    }
    if let Some(operation) = diff_fallback {
        operations.push(operation);
        return Ok(());
    }
    if !source.contains(GAP) {
        let mut desired = create_operation_text(&source, &source, all, number, true)?;
        desired.desired_state = true;
        if let Ok(pattern) = parse_pattern(&desired.pattern_text, number) {
            match locate(content, &pattern, &desired, number, "", &[], true) {
                Ok(located) => {
                    if let Some(matched) = located.first() {
                        let matched_normalized =
                            normalize_text(&content[matched.match_start..matched.match_end]).text;
                        let matched_units: Vec<u16> = matched_normalized.encode_utf16().collect();
                        let mut neighbors_duplicate = false;
                        if matched_units.len() >= 8 {
                            let before: Vec<u16> = normalize_text(&content[..matched.match_start])
                                .text
                                .encode_utf16()
                                .collect();
                            let after: Vec<u16> = normalize_text(&content[matched.match_end..])
                                .text
                                .encode_utf16()
                                .collect();
                            for overlap in (8..=matched_units.len()).rev() {
                                if before.ends_with(&matched_units[..overlap])
                                    || after.starts_with(
                                        &matched_units[matched_units.len() - overlap..],
                                    )
                                {
                                    neighbors_duplicate = true;
                                    break;
                                }
                            }
                        }
                        if neighbors_duplicate {
                            desired.recovery_note = Some(format!(
                                "Note: operation {number} stated desired text without markers; the \
								 closest matching block was replaced with it. State the current text in \
								 *** Find and the new text in *** Replace."
                            ));
                            operations.push(desired);
                            return Ok(());
                        }
                    }
                }
                Err(ambiguity) if ambiguity.to_string().contains(" is ambiguous: ") => {
                    let mut every = desired.clone();
                    every.all = true;
                    if let Ok(mut matches) =
                        locate(content, &pattern, &every, number, "", &[], true)
                        && matches.len() == 2
                    {
                        matches.sort_by_key(|candidate| candidate.match_start);
                        let first = &matches[0];
                        let second = &matches[1];
                        if first.match_end > second.match_start {
                            return Err(ambiguity);
                        }
                        if content[first.match_end..second.match_start]
                            .trim()
                            .is_empty()
                        {
                            let span_start = content[..first.match_start]
                                .rfind('\n')
                                .map_or(0, |at| at + 1);
                            let span = &content[span_start..second.match_end];
                            let replacement = &content[span_start..first.match_end];
                            let mut collapse =
                                create_operation_text(span, replacement, false, number, true)?;
                            collapse.recovery_note = Some(format!(
                                "Note: operation {number} stated desired text that currently appears \
								 twice back-to-back; the duplicate copy was collapsed."
                            ));
                            operations.push(collapse);
                            return Ok(());
                        }
                    }
                }
                Err(_) => {}
            }
        }
        if let Some(closest) = closest_desired_block(content, &source) {
            operations.push(Operation {
                pattern_text: closest,
                source_pattern_text: source.clone(),
                rewrite: OperationRewrite::Explicit {
                    text: source.clone(),
                },
                all: false,
                assumed_deletion: false,
                desired_state: false,
                recovery_note: Some(format!(
                    "Note: operation {number} stated desired text without markers; the closest \
					 matching block was replaced with it. State the current text in *** Find and the \
					 new text in *** Replace."
                )),
                whitespace_matched: false,
            });
            return Ok(());
        }
    }
    let normalized_pattern = normalize_text(&source).text;
    let context_echo = if !normalized_pattern.is_empty()
        && normalize_text(content).text.contains(&normalized_pattern)
    {
        "\nIts lines already exist in the file unchanged — if this operation only restated context, \
		 delete it; anchors belong inside the operation that edits them."
    } else {
        ""
    };
    let mut corrected = lines[..end_index].to_vec();
    corrected.push(REWRITE_HEADER.to_owned());
    corrected.push("{new text}".to_owned());
    corrected.extend_from_slice(&lines[end_index..]);
    let needs_separator = format!(
        "Operation {number} has *** Find but no *** Replace.{context_echo}\nCopy-ready corrected \
		 payload (fill in the new text):\n{}",
        ir_to_payload(
            &corrected.iter().map(String::as_str).collect::<Vec<_>>(),
            path
        )
    );
    if !source.contains('\n') || js_len(&normalized_pattern) < 24 {
        return Err(parse_error(needs_separator));
    }
    let mut operation = create_operation_text(&source, "", all, number, true)?;
    operation.assumed_deletion = true;
    pending.push((operations.len(), needs_separator));
    operations.push(operation);
    Ok(())
}
