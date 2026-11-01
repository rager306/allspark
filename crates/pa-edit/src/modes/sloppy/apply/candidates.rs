//! The match-mode ladder and candidate assembly: boundary resolution turns
//! per-literal occurrences into whole-pattern
//! [`Candidate`](crate::modes::sloppy::types::Candidate)s.

use std::collections::HashMap;

use super::super::types::{
    Candidate, CandidateResult, MAX_CANDIDATES, MAX_COMBINATIONS, NormalizedText, Occurrence,
    ParsedPattern, PatternToken,
};

use super::occurrences::{
    exact_occurrences, following_literal, fuzzy_occurrences, preceding_literal,
    selection_spans_usable, source_end, source_start,
};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum MatchMode {
    Raw,
    Normalized,
    Fuzzy,
}

fn resolve_boundary(
    boundary: usize,
    kind: u8,
    pattern: &ParsedPattern,
    matches: &HashMap<usize, Occurrence>,
    normalized: &NormalizedText,
    mode: MatchMode,
    content: Option<&str>,
) -> usize {
    let previous_index = preceding_literal(&pattern.tokens, boundary);
    let next_index = following_literal(&pattern.tokens, boundary);
    let previous = previous_index.and_then(|index| matches.get(&index));
    let next = next_index.and_then(|index| matches.get(&index));
    let immediate_previous =
        boundary > 0 && matches!(pattern.tokens[boundary - 1], PatternToken::Literal { .. });
    let immediate_next = boundary < pattern.tokens.len()
        && matches!(pattern.tokens[boundary], PatternToken::Literal { .. });
    let raw = mode == MatchMode::Raw;
    let start_at = |offset| {
        if raw {
            offset
        } else {
            source_start(
                normalized,
                offset,
                content.map_or(normalized.text.len(), str::len),
            )
        }
    };
    let end_at = |offset| {
        if raw {
            offset
        } else {
            source_end(
                normalized,
                offset,
                content.map_or(normalized.text.len(), str::len),
            )
        }
    };
    if kind == 2 {
        if let Some(next) = next {
            return start_at(next.start);
        }
        if let Some(previous) = previous {
            let offset = end_at(previous.end);
            if let Some(content) = content
                && offset > 0
                && content.as_bytes().get(offset - 1) != Some(&b'\n')
                && let Some(newline) = content[offset..].find('\n')
            {
                return offset + newline + 1;
            }
            return offset;
        }
    }
    if kind == 0 {
        if immediate_next && let Some(next) = next {
            return start_at(next.start);
        }
        if let Some(previous) = previous {
            return end_at(previous.end);
        }
        if let Some(next) = next {
            return start_at(next.start);
        }
    }
    if immediate_previous && let Some(previous) = previous {
        return end_at(previous.end);
    }
    if let Some(next) = next {
        return start_at(next.start);
    }
    previous.map_or(0, |entry| end_at(entry.end))
}

pub(super) fn collect_candidates(
    content: &str,
    normalized: &NormalizedText,
    pattern: &ParsedPattern,
    mode: MatchMode,
    allow_punctuation: bool,
) -> CandidateResult {
    let literal_indices = pattern
        .tokens
        .iter()
        .enumerate()
        .filter_map(|(index, token)| matches!(token, PatternToken::Literal { .. }).then_some(index))
        .collect::<Vec<_>>();
    let mut occurrences = HashMap::new();
    for index in &literal_indices {
        let PatternToken::Literal {
            text,
            normalized: needle,
        } = &pattern.tokens[*index]
        else {
            unreachable!()
        };
        let values = match mode {
            MatchMode::Raw => exact_occurrences(content, text),
            MatchMode::Normalized => exact_occurrences(&normalized.text, needle),
            MatchMode::Fuzzy => fuzzy_occurrences(&normalized.text, needle, allow_punctuation),
        };
        if values.is_empty() {
            return CandidateResult {
                candidates: Vec::new(),
                overflow: false,
            };
        }
        occurrences.insert(*index, values);
    }
    struct Search<'a> {
        content: &'a str,
        normalized: &'a NormalizedText,
        pattern: &'a ParsedPattern,
        mode: MatchMode,
        literal_indices: &'a [usize],
        occurrences: &'a HashMap<usize, Vec<Occurrence>>,
        allow_punctuation: bool,
        chosen: HashMap<usize, Occurrence>,
        candidates: Vec<Candidate>,
        combinations: usize,
        overflow: bool,
    }
    impl Search<'_> {
        fn source_start(&self, offset: usize) -> usize {
            if self.mode == MatchMode::Raw {
                offset
            } else {
                source_start(self.normalized, offset, self.content.len())
            }
        }

        fn source_end(&self, offset: usize) -> usize {
            if self.mode == MatchMode::Raw {
                offset
            } else {
                source_end(self.normalized, offset, self.content.len())
            }
        }

        fn visit(&mut self, position: usize) {
            if self.overflow {
                return;
            }
            if self.candidates.len() >= MAX_CANDIDATES || self.combinations >= MAX_COMBINATIONS {
                self.overflow = true;
                return;
            }
            if position == self.literal_indices.len() {
                if self.allow_punctuation
                    && self
                        .literal_indices
                        .iter()
                        .map(|index| self.chosen[index].punctuation_edits)
                        .sum::<usize>()
                        > 1
                {
                    return;
                }
                self.combinations += 1;
                let content = self.pattern.line_insertion.then_some(self.content);
                let start = resolve_boundary(
                    self.pattern.selection_start,
                    if self.pattern.insertion { 2 } else { 0 },
                    self.pattern,
                    &self.chosen,
                    self.normalized,
                    self.mode,
                    content,
                );
                let end = resolve_boundary(
                    self.pattern.selection_end,
                    if self.pattern.insertion { 2 } else { 1 },
                    self.pattern,
                    &self.chosen,
                    self.normalized,
                    self.mode,
                    content,
                );
                let first = self.chosen[&self.literal_indices[0]];
                let last = self.chosen[&self.literal_indices[self.literal_indices.len() - 1]];
                if start > end {
                    return;
                }
                let capture_count = self
                    .pattern
                    .tokens
                    .iter()
                    .filter(|token| matches!(token, PatternToken::Gap { .. }))
                    .count();
                let mut captures = vec![String::new(); capture_count];
                for (token_index, token) in self.pattern.tokens.iter().enumerate() {
                    let PatternToken::Gap { capture_index, .. } = token else {
                        continue;
                    };
                    let (Some(before_index), Some(after_index)) = (
                        preceding_literal(&self.pattern.tokens, token_index),
                        following_literal(&self.pattern.tokens, token_index + 1),
                    ) else {
                        return;
                    };
                    let capture_start = self.source_end(self.chosen[&before_index].end);
                    let capture_end = self.source_start(self.chosen[&after_index].start);
                    self.content[capture_start..capture_end]
                        .clone_into(&mut captures[*capture_index]);
                }
                let selection_spans = self
                    .pattern
                    .selection_pairs
                    .iter()
                    .map(|pair| {
                        let insertion = pair.start == pair.end;
                        let content = pair.line_insertion.then_some(self.content);
                        (
                            resolve_boundary(
                                pair.start,
                                if insertion { 2 } else { 0 },
                                self.pattern,
                                &self.chosen,
                                self.normalized,
                                self.mode,
                                content,
                            ),
                            resolve_boundary(
                                pair.end,
                                if insertion { 2 } else { 1 },
                                self.pattern,
                                &self.chosen,
                                self.normalized,
                                self.mode,
                                content,
                            ),
                        )
                    })
                    .collect::<Vec<_>>();
                // Every selection span must be an ordered, non-overlapping,
                // char-boundary sub-range of the candidate's own [start, end].
                // Garbled marker glyphs can otherwise resolve overlapping or
                // out-of-range pairs that later slice `content` (or the
                // `content[start..end]` rewrite buffer) on a mid-char or
                // out-of-range edge and panic the worker instead of reporting a
                // miss.
                if !selection_spans_usable(&selection_spans, start, end) {
                    return;
                }
                let candidate = Candidate {
                    start,
                    end,
                    match_start: self.source_start(first.start),
                    match_end: self.source_end(last.end),
                    captures,
                    selection_spans,
                    tuple: self
                        .literal_indices
                        .iter()
                        .map(|index| self.chosen[index].start)
                        .collect(),
                    literal_gaps: false,
                };
                if let Some(existing) =
                    self.candidates.iter_mut().find(|existing| {
                        existing.start == candidate.start
                            && existing.end == candidate.end
                            && self.pattern.selected_capture_indices.iter().all(|index| {
                                existing.captures[*index] == candidate.captures[*index]
                            })
                    })
                {
                    if candidate.match_end - candidate.match_start
                        < existing.match_end - existing.match_start
                    {
                        *existing = candidate;
                    }
                } else {
                    self.candidates.push(candidate);
                }
                return;
            }
            let token_index = self.literal_indices[position];
            let previous_index = position.checked_sub(1).map(|at| self.literal_indices[at]);
            let previous = previous_index.and_then(|index| self.chosen.get(&index).copied());
            let gap_tokens = previous_index.map_or(&[][..], |index| {
                &self.pattern.tokens[index + 1..token_index]
            });
            let has_gap = gap_tokens
                .iter()
                .any(|token| matches!(token, PatternToken::Gap { .. }));
            for occurrence in self.occurrences[&token_index].iter().copied() {
                if let Some(previous) = previous
                    && (if has_gap {
                        occurrence.start < previous.end
                    } else {
                        occurrence.start != previous.end
                    })
                {
                    continue;
                }
                if let Some(previous) = previous
                    && gap_tokens.iter().any(|token| {
                        matches!(
                            token,
                            PatternToken::Gap {
                                line_bounded: true,
                                ..
                            }
                        )
                    })
                    && self.content
                        [self.source_end(previous.end)..self.source_start(occurrence.start)]
                        .contains('\n')
                {
                    continue;
                }
                self.chosen.insert(token_index, occurrence);
                self.visit(position + 1);
                self.chosen.remove(&token_index);
            }
        }
    }
    let mut search = Search {
        content,
        normalized,
        pattern,
        mode,
        literal_indices: &literal_indices,
        occurrences: &occurrences,
        allow_punctuation,
        chosen: HashMap::new(),
        candidates: Vec::new(),
        combinations: 0,
        overflow: false,
    };
    search.visit(0);
    let all = search.candidates.clone();
    search.candidates.retain(|candidate| {
        !all.iter().any(|other| {
            other.match_start == candidate.match_start && other.match_end < candidate.match_end
        })
    });
    search.candidates.sort_by(|left, right| {
        (left.start, left.match_start, left.match_end, &left.tuple).cmp(&(
            right.start,
            right.match_start,
            right.match_end,
            &right.tuple,
        ))
    });
    CandidateResult {
        candidates: search.candidates,
        overflow: search.overflow,
    }
}
