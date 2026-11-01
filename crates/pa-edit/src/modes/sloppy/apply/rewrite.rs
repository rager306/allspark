//! Rewrite rendering: a `*** Replace` body becomes final text, and an inline
//! selection is framed into the span it replaces.

use std::borrow::Cow;

use super::super::{
    parse::decode_literal_markers,
    types::{
        Candidate, EdgeGaps, ParsedPattern, SelectionPair,
        markers::{GAP, SELECT_CLOSE, SELECT_OPEN},
    },
};

use super::{
    occurrences::{source_end, source_start},
    pattern::normalize_text,
};

use crate::error::EditError;

/// Drop the `*** Replace` ellipses that re-emit `*** Find`'s open edges. An
/// edge gap captured nothing, so re-emitting it writes nothing; a whole-line
/// edge `…` takes the newline joining it to the rest of the rewrite with it.
/// The leading edge is positional; the trailing one is only claimed by an
/// ellipsis left over after the inner captures, since `…` re-emits captures
/// in order.
fn strip_edge_gaps(rewrite: &str, edges: EdgeGaps, inner: usize) -> Cow<'_, str> {
    // A rewrite that is nothing but edge ellipses is context elision, not a
    // deletion; leave it for `render_rewrite` to reject.
    let keep = |stripped: String, current: &str| {
        if stripped.trim().is_empty() {
            current.to_owned()
        } else {
            stripped
        }
    };
    let mut text = Cow::Borrowed(rewrite);
    if edges.leading {
        let at = text.len() - text.trim_start().len();
        if text[at..].starts_with(GAP) {
            let after = at + GAP.len();
            let line_end = text[after..]
                .find('\n')
                .map_or(text.len(), |offset| after + offset);
            let stripped = if text[after..line_end].trim().is_empty() {
                let line_start = text[..at].rfind('\n').map_or(0, |offset| offset + 1);
                format!(
                    "{}{}",
                    &text[..line_start],
                    &text[(line_end + 1).min(text.len())..]
                )
            } else {
                format!("{}{}", &text[..at], &text[after..])
            };
            text = Cow::Owned(keep(stripped, &text));
        }
    }
    if edges.trailing && text.matches(GAP).count() > inner {
        let end = text.trim_end().len();
        if text[..end].ends_with(GAP) {
            let at = end - GAP.len();
            let line_start = text[..at].rfind('\n').map_or(0, |offset| offset + 1);
            let cut = if text[line_start..at].trim().is_empty() {
                text[..line_start]
                    .strip_suffix('\n')
                    .map_or(line_start, |rest| {
                        rest.strip_suffix('\r').map_or(rest.len(), str::len)
                    })
            } else {
                at
            };
            let stripped = text[..cut].to_owned();
            text = Cow::Owned(keep(stripped, &text));
        }
    }
    text
}

/// Edges of `*** Find` that an inline selection borders; its replacement may
/// re-emit them.
pub(super) fn selection_edges(
    pattern: &ParsedPattern,
    candidate: &Candidate,
    pair: &SelectionPair,
) -> EdgeGaps {
    if candidate.literal_gaps {
        return EdgeGaps::default();
    }
    EdgeGaps {
        leading: pattern.edge_gaps.leading && pair.start == 0,
        trailing: pattern.edge_gaps.trailing && pair.end == pattern.tokens.len(),
    }
}

pub(super) fn render_rewrite(
    rewrite: &str,
    indices: &[usize],
    captures: &[String],
    edges: EdgeGaps,
    operation_number: usize,
) -> Result<String, EditError> {
    if rewrite.contains(SELECT_OPEN) || rewrite.contains(SELECT_CLOSE) {
        return Err(EditError::matched(format!(
            "Operation {operation_number} has selection markers in *** Replace; *** Find is current \
			 text, *** Replace is final text."
        )));
    }
    let stripped = strip_edge_gaps(rewrite, edges, indices.len());
    let rewrite = stripped.as_ref();
    let mut rendered = String::new();
    let mut marker = 0;
    let mut index = 0;
    while index < rewrite.len() {
        if rewrite[index..].starts_with(GAP) {
            let line_start = rewrite[..index].rfind('\n').map_or(0, |at| at + 1);
            let line_end = rewrite[index + GAP.len()..]
                .find('\n')
                .map_or(rewrite.len(), |at| index + GAP.len() + at);
            let line = &rewrite[line_start..line_end];
            if marker >= indices.len() {
                if line.trim() == GAP {
                    return Err(EditError::matched(format!(
                        "Operation {operation_number} *** Replace has a whole-line {GAP} with no *** \
						 Find gap to re-emit. *** Replace is final text written verbatim: type the \
						 elided lines out, or add a matching {GAP} gap to *** Find. To write a literal \
						 {GAP} line, use the write tool."
                    )));
                }
                rendered.push_str(GAP);
            } else {
                let capture = captures.get(indices[marker]).map_or("", String::as_str);
                let open_ended =
                    line.trim() == GAP || rewrite[index + GAP.len()..line_end].trim().is_empty();
                if capture.contains('\n') && !open_ended {
                    rendered.push_str(GAP);
                } else {
                    rendered.push_str(capture);
                    marker += 1;
                }
            }
            index += GAP.len();
            continue;
        }
        let character = rewrite[index..].chars().next().expect("non-empty suffix");
        rendered.push(character);
        index += character.len_utf8();
    }
    Ok(decode_literal_markers(&rendered))
}

pub(super) fn align_boundary_echoes(
    content: &str,
    candidate: &Candidate,
    replacement: &str,
) -> String {
    if replacement.is_empty()
        || candidate.start == candidate.match_start && candidate.end == candidate.match_end
    {
        return replacement.to_owned();
    }
    let prefix = &content[candidate.match_start..candidate.start];
    let suffix = &content[candidate.end..candidate.match_end];
    let normalized_replacement = normalize_text(replacement);
    let normalized_prefix = normalize_text(prefix).text;
    let normalized_suffix = normalize_text(suffix).text;
    let prefix_echo =
        normalized_prefix.len() >= 3 && normalized_replacement.text.starts_with(&normalized_prefix);
    let suffix_echo = !normalized_suffix.is_empty()
        && normalized_replacement.text.ends_with(&normalized_suffix)
        && (normalized_suffix.len() >= 3 || prefix_echo);
    if !prefix_echo && !suffix_echo {
        return replacement.to_owned();
    }
    let mut from = 0;
    let mut to = replacement.len();
    if prefix_echo {
        from = replacement.strip_prefix(prefix).map_or_else(
            || {
                source_end(
                    &normalized_replacement,
                    normalized_prefix.len(),
                    replacement.len(),
                )
            },
            |rest| replacement.len() - rest.len(),
        );
    }
    if suffix_echo {
        to = replacement.strip_suffix(suffix).map_or_else(
            || {
                source_start(
                    &normalized_replacement,
                    normalized_replacement.text.len() - normalized_suffix.len(),
                    replacement.len(),
                )
            },
            str::len,
        );
    }
    if from > to {
        return replacement.to_owned();
    }
    let mut aligned = replacement[from..to].to_owned();
    if prefix.chars().last().is_some_and(char::is_whitespace)
        && aligned.chars().next().is_some_and(char::is_whitespace)
    {
        aligned = aligned.trim_start_matches(char::is_whitespace).to_owned();
    }
    if suffix.chars().next().is_some_and(char::is_whitespace)
        && aligned.chars().last().is_some_and(char::is_whitespace)
    {
        aligned = aligned.trim_end_matches(char::is_whitespace).to_owned();
    }
    aligned
}

pub(super) fn expand_full_line_deletion(content: &str, candidate: &Candidate) -> Candidate {
    if candidate.start == candidate.end {
        return candidate.clone();
    }
    let line_start = content[..candidate.start]
        .rfind('\n')
        .map_or(0, |at| at + 1);
    let newline = content[candidate.end..]
        .find('\n')
        .map(|at| candidate.end + at);
    let line_end = newline.unwrap_or(content.len());
    if !content[line_start..candidate.start]
        .bytes()
        .all(|byte| matches!(byte, b' ' | b'\t'))
        || !content[candidate.end..line_end]
            .bytes()
            .all(|byte| matches!(byte, b' ' | b'\t'))
    {
        return candidate.clone();
    }
    let mut result = candidate.clone();
    result.start = line_start;
    result.end = newline.map_or(line_end, |at| at + 1);
    if line_start > 0 && result.end < content.len() {
        let previous_end = line_start - 1;
        let previous_start = content[..previous_end].rfind('\n').map_or(0, |at| at + 1);
        let next_end = content[result.end..]
            .find('\n')
            .map_or(content.len(), |at| result.end + at);
        let previous_blank = content[previous_start..previous_end].trim().is_empty();
        let next_blank = content[result.end..next_end].trim().is_empty();
        let previous_text = &content[previous_start..previous_end];
        if previous_blank && next_blank {
            result.end = if next_end == content.len() {
                next_end
            } else {
                next_end + 1
            };
        } else if previous_blank
            && content[result.end..next_end]
                .trim_start()
                .starts_with([')', ']', '}'])
        {
            result.start = previous_start;
        } else if next_blank && previous_text.trim_end().ends_with(['(', '{', '[']) {
            result.end = if next_end == content.len() {
                next_end
            } else {
                next_end + 1
            };
        }
    } else if line_start > 0 && result.end >= content.len() {
        let previous_end = line_start - 1;
        let previous_start = content[..previous_end].rfind('\n').map_or(0, |at| at + 1);
        if content[previous_start..previous_end].trim().is_empty() {
            result.start = previous_start;
        }
    }
    result
}

pub(super) fn snap_line_insertion_offset(
    content: &str,
    offset: usize,
    hop_blank_lines: bool,
) -> usize {
    let mut line_start = content[..offset].rfind('\n').map_or(0, |at| at + 1);
    if !content[line_start..offset]
        .bytes()
        .all(|byte| matches!(byte, b' ' | b'\t'))
    {
        return offset;
    }
    while hop_blank_lines && line_start > 0 {
        let previous_end = line_start - 1;
        let previous_start = content[..previous_end].rfind('\n').map_or(0, |at| at + 1);
        if !content[previous_start..previous_end]
            .bytes()
            .all(|byte| matches!(byte, b' ' | b'\t'))
        {
            break;
        }
        line_start = previous_start;
    }
    line_start
}

pub(super) fn frame_line_insertion(
    content: &str,
    offset: usize,
    desired: &str,
    blank_separated: bool,
) -> String {
    if desired.is_empty() {
        return String::new();
    }
    if offset > 0 && offset == content.len() && content.as_bytes()[offset - 1] != b'\n' {
        return if desired.starts_with('\n') {
            desired.to_owned()
        } else {
            format!("\n{desired}")
        };
    }
    let mut framed = if desired.ends_with('\n') {
        desired.to_owned()
    } else {
        format!("{desired}\n")
    };
    if blank_separated
        && content.as_bytes().get(offset) != Some(&b'\n')
        && !framed.ends_with("\n\n")
    {
        framed.push('\n');
    }
    framed
}

pub(super) fn rewrite_proves_whole_span(
    content: &str,
    candidate: &Candidate,
    rewrite: &str,
) -> bool {
    let normalized_rewrite = normalize_text(rewrite).text;
    let contexts = candidate
        .selection_spans
        .windows(2)
        .filter_map(|spans| {
            let context = normalize_text(&content[spans[0].1..spans[1].0]).text;
            (!context.is_empty()).then_some(context)
        })
        .collect::<Vec<_>>();
    if contexts.is_empty() {
        return false;
    }
    let mut from = 0;
    for context in contexts {
        let Some(found) = normalized_rewrite[from..].find(&context) else {
            return false;
        };
        from += found + context.len();
    }
    true
}

pub(super) fn rewrite_selection_spans(
    content: &str,
    candidate: &Candidate,
    replacements: &[String],
) -> String {
    let mut rewritten = content[candidate.start..candidate.end].to_owned();
    let mut indexed = candidate
        .selection_spans
        .iter()
        .copied()
        .zip(replacements)
        .collect::<Vec<_>>();
    indexed.sort_by_key(|((start, _), _)| std::cmp::Reverse(*start));
    for ((start, end), replacement) in indexed {
        rewritten.replace_range(start - candidate.start..end - candidate.start, replacement);
    }
    rewritten
}

pub(super) fn positional_rewrite_segments(
    rewrite: &str,
    count: usize,
    pattern_has_gaps: bool,
) -> Option<Vec<String>> {
    let lines = rewrite.split('\n').collect::<Vec<_>>();
    if lines.iter().any(|line| line.trim() == GAP) {
        if pattern_has_gaps {
            return None;
        }
        let mut groups = vec![Vec::new()];
        for line in lines {
            if line.trim() == GAP {
                groups.push(Vec::new());
            } else {
                groups.last_mut().expect("one group").push(line);
            }
        }
        return (groups.len() == count)
            .then(|| groups.into_iter().map(|group| group.join("\n")).collect());
    }
    if lines.len() == count {
        return Some(lines.into_iter().map(str::to_owned).collect());
    }
    if !rewrite.contains('\n') {
        let segments = rewrite.split(GAP).map(str::to_owned).collect::<Vec<_>>();
        if segments.len() == count {
            return Some(segments);
        }
    }
    None
}

pub(super) fn prepare_inline(
    content: &str,
    located: &Candidate,
    span: (usize, usize),
    selection: &SelectionPair,
    rewrite: &str,
    edges: EdgeGaps,
    operation_number: usize,
    lenient: bool,
) -> Result<(Candidate, String, Option<String>), EditError> {
    let (mut start, mut end) = span;
    if selection.gap_only {
        if let Some(newline) = content[start..].find('\n').map(|at| start + at)
            && newline < end
        {
            end = newline;
        }
        while start < end && matches!(content.as_bytes()[start], b' ' | b'\t') {
            start += 1;
        }
        while end > start && matches!(content.as_bytes()[end - 1], b' ' | b'\t') {
            end -= 1;
        }
    }
    if selection.line_insertion && start == end && lenient {
        start = snap_line_insertion_offset(content, start, !rewrite.starts_with('\n'));
        end = start;
    }
    let mut candidate = located.clone();
    candidate.start = start;
    candidate.end = end;
    candidate.match_start = start;
    candidate.match_end = end;
    let stripped_blank = start == end
        && rewrite.starts_with('\n')
        && content.as_bytes().get(start.wrapping_sub(1)) == Some(&b'\n');
    let desired = if stripped_blank {
        &rewrite[1..]
    } else {
        rewrite
    };
    let blank_separated = stripped_blank && start >= 2 && content.as_bytes()[start - 2] == b'\n';
    let framed = if selection.line_insertion {
        frame_line_insertion(content, start, desired, blank_separated)
    } else {
        desired.to_owned()
    };
    let replacement = render_rewrite(
        &framed,
        &selection.capture_indices,
        &candidate.captures,
        edges,
        operation_number,
    )?;
    if replacement.is_empty() && start != end {
        let deleted = content[start..end].to_owned();
        candidate = expand_full_line_deletion(content, &candidate);
        return Ok((candidate, replacement, Some(deleted)));
    }
    Ok((candidate, replacement, None))
}
