//! The atomic apply: plan every operation's edits against the original file,
//! reconcile overlaps, and splice once.

use std::collections::{BTreeMap, HashSet};

use super::super::{
    parse::{edit_header, operation_payload, parse_section},
    types::{
        Candidate, EdgeGaps, NormalizedText, OperationRewrite, PatternToken, Placement, PlannedEdit,
    },
};

use super::{
    ApplyContext,
    diagnostics::{line_number_at, numbered_preview},
    locate::locate_with_recovery,
    pattern::normalize_text,
    rewrite::{
        align_boundary_echoes, expand_full_line_deletion, frame_line_insertion,
        positional_rewrite_segments, prepare_inline, render_rewrite, rewrite_proves_whole_span,
        rewrite_selection_spans, selection_edges, snap_line_insertion_offset,
    },
};

use crate::error::EditError;
use crate::store::payload_hash;

fn normalized_index_at(normalized: &NormalizedText, raw_offset: usize) -> usize {
    normalized
        .starts
        .partition_point(|offset| *offset < raw_offset)
}

fn duplicate_collapse_span(
    content: &str,
    candidate: &Candidate,
    replacement: &str,
) -> Option<(usize, usize)> {
    const MIN_OVERLAP: usize = 8;
    let rewrite = normalize_text(replacement).text;
    if rewrite.len() < MIN_OVERLAP || rewrite.len() > 5000 {
        return None;
    }
    let normalized = normalize_text(content);
    let match_start = normalized_index_at(&normalized, candidate.start);
    let match_end = normalized_index_at(&normalized, candidate.end);
    for overlap in (MIN_OVERLAP..=rewrite.len().min(match_start)).rev() {
        if !normalized
            .text
            .get(match_start - overlap..match_start)
            .is_some_and(|prefix| rewrite.starts_with(prefix))
        {
            continue;
        }
        let mut start = normalized
            .starts
            .get(match_start - overlap)
            .copied()
            .unwrap_or(candidate.start);
        let line_start = content[..start].rfind('\n').map_or(0, |at| at + 1);
        if content[line_start..start]
            .bytes()
            .all(|byte| matches!(byte, b' ' | b'\t'))
        {
            start = line_start;
        }
        return Some((start, candidate.end));
    }
    for overlap in (MIN_OVERLAP
        ..=rewrite
            .len()
            .min(normalized.text.len().saturating_sub(match_end)))
        .rev()
    {
        if !normalized
            .text
            .get(match_end..match_end + overlap)
            .is_some_and(|suffix| rewrite.ends_with(suffix))
        {
            continue;
        }
        let mut end = normalized
            .ends
            .get(match_end + overlap - 1)
            .copied()
            .unwrap_or(candidate.end);
        let line_end = content[end..]
            .find('\n')
            .map_or(content.len(), |at| end + at);
        if content[end..line_end]
            .bytes()
            .all(|byte| matches!(byte, b' ' | b'\t'))
        {
            end = line_end;
        }
        return Some((candidate.start, end));
    }
    None
}

fn resolve_references(rewrite: &str, removed: &[Option<String>]) -> Result<String, EditError> {
    let mut lines = Vec::new();
    for line in rewrite.split('\n') {
        let trimmed = line.trim();
        if let Some(number) = trimmed
            .strip_prefix('»')
            .and_then(|value| value.parse::<usize>().ok())
        {
            let Some(Some(value)) = removed.get(number.saturating_sub(1)) else {
                return Err(EditError::matched(format!(
                    "»{number} must reference an earlier deletion operation."
                )));
            };
            lines.push(value.clone());
        } else {
            lines.push(line.to_owned());
        }
    }
    Ok(lines.join("\n"))
}

fn no_op_error(
    context: &ApplyContext<'_>,
    payload: u64,
    operation: Option<usize>,
    preview: Option<(&str, usize)>,
    match_count: Option<usize>,
    hint: Option<&str>,
) -> EditError {
    let (count, escalated) = context.store.record_noop(context.canonical, payload);
    let base = if escalated {
        format!(
            "STOP: identical no-op repeated {count} times for {}. Re-read current code and send a \
			 changed payload, or move on.",
            context.path
        )
    } else if let Some(operation) = operation {
        if let Some(matches) = match_count {
            format!(
                "Operation {operation} (all) matched {matches} occurrences but all make no change to \
				 {}.",
                context.path
            )
        } else {
            format!("Operation {operation} makes no change to {}.", context.path)
        }
    } else {
        format!("Edits to {} made no change.", context.path)
    };
    let grounding = preview.map_or(String::new(), |(content, offset)| {
        format!(
            "\nYour rewrite normalized to text identical to these lines. Indentation-only changes \
			 are applied verbatim; adjust the authored *** Replace if another whitespace change was \
			 intended.\nCurrent file content near the closest match (no re-read needed):\n{}",
            numbered_preview(content, offset)
        )
    });
    let hint = hint.map_or(String::new(), |hint| format!("\n{hint}"));
    EditError::matched(format!("{base}{grounding}{hint}"))
}

fn reconcile_overlap(
    content: &str,
    left: &PlannedEdit,
    right: &PlannedEdit,
) -> Option<PlannedEdit> {
    let start = left.start.min(right.start);
    let end = left.end.max(right.end);
    let container = |outer: &PlannedEdit, inner: &PlannedEdit| {
        (outer.replacement.is_empty()
            && !inner.replacement.is_empty()
            && inner.start >= outer.start
            && inner.end <= outer.end)
            .then(|| PlannedEdit {
                start: outer.start,
                end: outer.end,
                replacement: if content.as_bytes().get(outer.end.wrapping_sub(1)) == Some(&b'\n')
                    && !inner.replacement.ends_with('\n')
                {
                    format!("{}\n", inner.replacement)
                } else {
                    inner.replacement.clone()
                },
                operation_number: inner.operation_number,
            })
    };
    if let Some(result) = container(left, right).or_else(|| container(right, left)) {
        return Some(result);
    }
    let project = |edit: &PlannedEdit| {
        format!(
            "{}{}{}",
            &content[start..edit.start],
            edit.replacement,
            &content[edit.end..end]
        )
    };
    (project(left) == project(right)).then(|| PlannedEdit {
        start,
        end,
        replacement: project(left),
        operation_number: left.operation_number,
    })
}

#[allow(
    clippy::suspicious_operation_groupings,
    reason = "paired index bounds are intentional"
)]
pub(super) fn apply_operations(
    content: &str,
    input: &str,
    context: &mut ApplyContext<'_>,
) -> Result<String, EditError> {
    let payload = payload_hash(input);
    let operations = parse_section(input, content, context.path, context.streaming)?;
    let mut removed = vec![None; operations.len()];
    let mut planned = Vec::new();
    let mut recovery_notes = Vec::new();
    let mut deletion_notes = BTreeMap::new();
    let mut last_match = 0;
    let mut queue = (0..operations.len()).collect::<Vec<_>>();
    let mut deferred = HashSet::new();
    let mut cursor = 0;
    while cursor < queue.len() {
        let index = queue[cursor];
        cursor += 1;
        let number = index + 1;
        if let Some(note) = &operations[index].recovery_note {
            recovery_notes.push(note.clone());
        }
        let exclusions = if deferred.contains(&index) {
            planned
                .iter()
                .map(|edit: &PlannedEdit| (edit.start, edit.end))
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        };
        let located = locate_with_recovery(
            content,
            &operations[index],
            number,
            context.path,
            &exclusions,
            operations.len() == 1,
        );
        let (operation, pattern, mut candidates) = match located {
            Ok(value) => value,
            Err(error)
                if !deferred.contains(&index) && error.to_string().contains(" is ambiguous: ") =>
            {
                deferred.insert(index);
                queue.push(index);
                continue;
            }
            Err(error) => return Err(error),
        };
        if operation.whitespace_matched {
            recovery_notes.push(format!(
                "Note: operation {number}'s *** Find differed from the file in whitespace only and \
				 was matched leniently. Inserted lines are written exactly as authored — verify their \
				 indentation."
            ));
        }
        if operation.all {
            candidates.sort_by_key(|candidate| std::cmp::Reverse(candidate.start));
        }
        match &operation.rewrite {
            OperationRewrite::Insert { text, at } => {
                let body = text.strip_suffix('\n').unwrap_or(text);
                for candidate in &candidates {
                    let (offset, replacement) = match at {
                        // Insert at the first anchor line's start, ahead of its indentation.
                        Placement::Before => {
                            let start = candidate.match_start;
                            let offset = content[..start].rfind('\n').map_or(0, |at| at + 1);
                            (offset, format!("{body}\n"))
                        }
                        // Insert before the final anchor line's newline, retaining its EOF
                        // convention.
                        Placement::After => {
                            let end = candidate.match_end;
                            let offset = if end > 0 && content.as_bytes()[end - 1] == b'\n' {
                                end - 1
                            } else {
                                content[end..]
                                    .find('\n')
                                    .map_or(content.len(), |at| end + at)
                            };
                            (offset, format!("\n{body}"))
                        }
                    };
                    planned.push(PlannedEdit {
                        start: offset,
                        end: offset,
                        replacement,
                        operation_number: number,
                    });
                    last_match = candidate.match_start;
                }
            }
            OperationRewrite::Inline { replacements } => {
                let replacements = replacements
                    .iter()
                    .map(|rewrite| resolve_references(rewrite, &removed))
                    .collect::<Result<Vec<_>, _>>()?;
                if pattern.selection_pairs.len() != replacements.len() {
                    return Err(EditError::matched(format!(
                        "Operation {number} inline replacements do not align with its selections."
                    )));
                }
                let mut changes = 0;
                for candidate in &candidates {
                    let mut selections = candidate
                        .selection_spans
                        .iter()
                        .copied()
                        .enumerate()
                        .collect::<Vec<_>>();
                    selections.sort_by_key(|(_, (start, _))| std::cmp::Reverse(*start));
                    for (selection_index, span) in selections {
                        let pair = &pattern.selection_pairs[selection_index];
                        let (prepared, replacement, deleted) = prepare_inline(
                            content,
                            candidate,
                            span,
                            pair,
                            &replacements[selection_index],
                            selection_edges(&pattern, candidate, pair),
                            number,
                            operation.whitespace_matched,
                        )?;
                        if content[prepared.start..prepared.end] == replacement {
                            continue;
                        }
                        if candidates.len() == 1
                            && pattern.selection_pairs.len() == 1
                            && let Some(deleted) = deleted
                        {
                            removed[index] = Some(deleted);
                        }
                        planned.push(PlannedEdit {
                            start: prepared.start,
                            end: prepared.end,
                            replacement,
                            operation_number: number,
                        });
                        changes += 1;
                    }
                    last_match = candidate.match_start;
                }
                if changes == 0 {
                    return Err(no_op_error(
                        context,
                        payload,
                        Some(number),
                        Some((content, candidates[0].match_start)),
                        operation.all.then_some(candidates.len()),
                        Some(
                            "The stated text equals the current text and never changes the file. Restate \
							 the edit with the actual change; do not drop the operation.",
                        ),
                    ));
                }
            }
            OperationRewrite::Explicit { text } => {
                let resolved = resolve_references(text, &removed)?;
                let base = if resolved.trim().is_empty() {
                    if pattern.insertion {
                        "\n".to_owned()
                    } else {
                        String::new()
                    }
                } else {
                    resolved
                };
                if pattern.selection_ranges.len() > 1
                    && let Some(segments) = positional_rewrite_segments(
                        &base,
                        pattern.selection_ranges.len(),
                        pattern
                            .tokens
                            .iter()
                            .any(|token| matches!(token, PatternToken::Gap { .. })),
                    )
                {
                    let mut changes = 0;
                    for candidate in &candidates {
                        for ((start, end), replacement) in candidate
                            .selection_spans
                            .iter()
                            .copied()
                            .zip(&segments)
                            .rev()
                        {
                            if content[start..end] == *replacement {
                                continue;
                            }
                            planned.push(PlannedEdit {
                                start,
                                end,
                                replacement: replacement.clone(),
                                operation_number: number,
                            });
                            changes += 1;
                        }
                        last_match = candidate.match_start;
                    }
                    if changes == 0 {
                        return Err(no_op_error(
                            context,
                            payload,
                            Some(number),
                            Some((content, candidates[0].match_start)),
                            operation.all.then_some(candidates.len()),
                            None,
                        ));
                    }
                    continue;
                }
                if pattern.selection_ranges.len() > 1
                    && !candidates
                        .iter()
                        .all(|candidate| rewrite_proves_whole_span(content, candidate, &base))
                {
                    let one_line = base.split_whitespace().collect::<Vec<_>>().join(" ");
                    let repeated = vec![one_line; pattern.selection_ranges.len()];
                    let header = edit_header(context.path, operation.all);
                    let candidate = &candidates[0];
                    return Err(EditError::matched(
						[
							format!(
								"Operation {number} has {} selections, but *** Replace proves neither \
								 positional substitution nor whole-span replacement.",
								pattern.selection_ranges.len()
							),
							"Copy-ready per-selection interpretation:".to_owned(),
							format!(
								"{header}\n*** Find\n{}\n*** Replace\n{}",
								operation.pattern_text,
								repeated.join("\n")
							),
							"Copy-ready whole-span interpretation:".to_owned(),
							format!(
								"{header}\n*** Find\n{}\n*** Replace\n{}",
								operation.pattern_text,
								rewrite_selection_spans(content, candidate, &repeated)
							),
						]
						.join("\n"),
					));
                }
                let mut changes = 0;
                for located in &candidates {
                    let mut candidate = located.clone();
                    if operation.whitespace_matched
                        && pattern.line_insertion
                        && candidate.start == candidate.end
                    {
                        let snapped = snap_line_insertion_offset(
                            content,
                            candidate.start,
                            !base.starts_with('\n'),
                        );
                        candidate.start = snapped;
                        candidate.end = snapped;
                    }
                    let rewrite = if pattern.line_insertion {
                        frame_line_insertion(content, candidate.start, &base, false)
                    } else {
                        base.clone()
                    };
                    let rendered = render_rewrite(
                        &rewrite,
                        if candidate.captures.is_empty() {
                            &[]
                        } else {
                            &pattern.selected_capture_indices
                        },
                        &candidate.captures,
                        if candidate.literal_gaps {
                            EdgeGaps::default()
                        } else {
                            pattern.edge_gaps
                        },
                        number,
                    )?;
                    let replacement = align_boundary_echoes(content, &candidate, &rendered);
                    let deleted = replacement
                        .is_empty()
                        .then(|| content[candidate.start..candidate.end].to_owned());
                    if deleted.is_some() {
                        candidate = expand_full_line_deletion(content, &candidate);
                    }
                    if candidates.len() == 1
                        && let Some(deleted) = &deleted
                    {
                        removed[index] = Some(deleted.clone());
                    }
                    if let Some(deleted) = deleted {
                        let lines = deleted
                            .lines()
                            .filter(|line| !line.trim().is_empty())
                            .count();
                        deletion_notes.insert(
							number,
							if operation.assumed_deletion {
								format!(
									"Note: operation {number} had no *** Replace and was applied as a move \
									 deletion (a later operation re-emits its block)."
								)
							} else {
								format!(
									"Note: operation {number} deleted {lines} line(s); an empty *** \
									 Replace means deletion — resend with the final text if you meant to \
									 replace."
								)
							},
						);
                    }
                    last_match = candidate.match_start;
                    let same = content[candidate.start..candidate.end] == replacement
                        || operation.desired_state
                            && normalize_text(&replacement).text
                                == normalize_text(&content[candidate.start..candidate.end]).text;
                    if same {
                        if operation.all {
                            continue;
                        }
                        if let Some((start, end)) =
                            duplicate_collapse_span(content, &candidate, &replacement)
                        {
                            planned.push(PlannedEdit {
                                start,
                                end,
                                replacement,
                                operation_number: number,
                            });
                            changes += 1;
                            continue;
                        }
                        if operation.desired_state {
                            recovery_notes.push(format!(
								"Note: operation {number} already matches the file; no change was needed \
								 there."
							));
                            changes += 1;
                            continue;
                        }
                        return Err(no_op_error(
                            context,
                            payload,
                            Some(number),
                            Some((content, candidate.match_start)),
                            None,
                            None,
                        ));
                    }
                    planned.push(PlannedEdit {
                        start: candidate.start,
                        end: candidate.end,
                        replacement,
                        operation_number: number,
                    });
                    changes += 1;
                }
                if operation.all && changes == 0 {
                    return Err(no_op_error(
                        context,
                        payload,
                        Some(number),
                        Some((content, candidates[0].match_start)),
                        Some(candidates.len()),
                        None,
                    ));
                }
            }
        }
    }
    if planned.is_empty() {
        return Err(no_op_error(
            context,
            payload,
            None,
            Some((content, last_match)),
            None,
            None,
        ));
    }
    planned.sort_by_key(|edit| (edit.start, edit.end));
    let mut ordered: Vec<PlannedEdit> = Vec::new();
    for current in planned {
        let Some(previous) = ordered.last().cloned() else {
            ordered.push(current);
            continue;
        };
        let overlaps = current.start < previous.end
            || current.start == previous.start
                && current.end == current.start
                && previous.end == previous.start;
        if !overlaps {
            ordered.push(current);
            continue;
        }
        if let Some(merged) = reconcile_overlap(content, &previous, &current) {
            *ordered.last_mut().expect("present") = merged;
            continue;
        }
        if previous.operation_number == current.operation_number {
            continue;
        }
        let first_line = line_number_at(content, previous.start);
        let second_line = line_number_at(content, current.start);
        return Err(EditError::matched(format!(
            "Operations {} and {} target overlapping original spans near lines {first_line} and \
			 {second_line}.\n\nConflicting candidates:\n\nOperation {} near line \
			 {first_line}:\n{}\n\nOperation {} near line {second_line}:\n{}\n\nKeep whichever states \
			 the intended final text and drop the other.",
            previous.operation_number,
            current.operation_number,
            previous.operation_number,
            operation_payload(
                &operations[previous.operation_number - 1],
                context.path,
                false,
                None
            ),
            current.operation_number,
            operation_payload(
                &operations[current.operation_number - 1],
                context.path,
                false,
                None
            )
        )));
    }
    let mut result = content.to_owned();
    for edit in ordered.into_iter().rev() {
        result.replace_range(edit.start..edit.end, &edit.replacement);
    }
    if result == content {
        return Err(no_op_error(
            context,
            payload,
            None,
            Some((content, last_match)),
            None,
            None,
        ));
    }
    context.store.reset_noop(context.canonical);
    context.notes.extend(recovery_notes);
    context.notes.extend(deletion_notes.into_values());
    Ok(result)
}
