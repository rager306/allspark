//! Whole-block fuzzy search: the exact pass, the indent-normalized window
//! scan, and [`find_match`], which chooses between them.

use crate::text::{count_leading_whitespace, is_non_empty_line, js_trim, normalize_for_fuzzy};

use super::distance::similarity;
use super::report::format_preview_window;
use super::{
    DEFAULT_FUZZY_THRESHOLD, DOMINANT_FUZZY_DELTA, DOMINANT_FUZZY_MIN_CONFIDENCE,
    FALLBACK_THRESHOLD, MAX_RECORDED_MATCHES,
};

/// A located block of text.
#[derive(Debug, Clone, PartialEq)]
pub struct FuzzyMatch {
    pub actual_text: String,
    /// Byte offset of the match start in the searched content.
    pub start_index: usize,
    /// 1-indexed line of the match start.
    pub start_line: u32,
    pub confidence: f64,
}

/// Outcome of [`find_match`].
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MatchOutcome {
    pub matched: Option<FuzzyMatch>,
    pub closest: Option<FuzzyMatch>,
    pub occurrences: Option<usize>,
    pub occurrence_lines: Option<Vec<u32>>,
    pub occurrence_previews: Option<Vec<String>>,
    pub fuzzy_matches: Option<usize>,
    pub dominant_fuzzy: Option<bool>,
}

/// A byte range excluded from matching.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExcludedRange {
    pub start_index: usize,
    pub end_index: usize,
}

/// Knobs for [`find_match`].
#[derive(Debug, Clone, Default)]
pub struct FindMatchOptions<'a> {
    pub allow_fuzzy: bool,
    /// Defaults to [`DEFAULT_FUZZY_THRESHOLD`].
    pub threshold: Option<f64>,
    pub excluded_ranges: &'a [ExcludedRange],
}

fn overlaps_excluded(start: usize, end: usize, ranges: &[ExcludedRange]) -> bool {
    ranges
        .iter()
        .any(|range| start < range.end_index && end > range.start_index)
}

fn find_exact_match_outcome(
    content: &str,
    target: &str,
    excluded_ranges: &[ExcludedRange],
) -> Option<MatchOutcome> {
    let mut first_index = None;
    let mut occurrences = 0;
    let mut recorded_indices = Vec::new();
    let mut search_start = 0;
    while search_start <= content.len().saturating_sub(target.len()) {
        let Some(relative) = content[search_start..].find(target) else {
            break;
        };
        let index = search_start + relative;
        let end_index = index + target.len();
        if !overlaps_excluded(index, end_index, excluded_ranges) {
            first_index.get_or_insert(index);
            occurrences += 1;
            if recorded_indices.len() < MAX_RECORDED_MATCHES {
                recorded_indices.push(index);
            }
        }
        search_start = end_index;
    }
    let first_index = first_index?;
    if occurrences > 1 {
        let content_lines: Vec<&str> = content.split('\n').collect();
        let mut occurrence_lines = Vec::with_capacity(recorded_indices.len());
        let mut occurrence_previews = Vec::with_capacity(recorded_indices.len());
        for index in recorded_indices {
            let line_number = content[..index]
                .bytes()
                .filter(|byte| *byte == b'\n')
                .count()
                + 1;
            occurrence_lines.push(line_number as u32);
            occurrence_previews.push(format_preview_window(&content_lines, line_number - 1));
        }
        return Some(MatchOutcome {
            occurrences: Some(occurrences),
            occurrence_lines: Some(occurrence_lines),
            occurrence_previews: Some(occurrence_previews),
            ..MatchOutcome::default()
        });
    }
    let start_line = content[..first_index]
        .bytes()
        .filter(|byte| *byte == b'\n')
        .count() as u32
        + 1;
    Some(MatchOutcome {
        matched: Some(FuzzyMatch {
            actual_text: target.to_owned(),
            start_index: first_index,
            start_line,
            confidence: 1.0,
        }),
        ..MatchOutcome::default()
    })
}

fn relative_indent_depths(lines: &[&str]) -> Vec<usize> {
    let indents: Vec<usize> = lines
        .iter()
        .map(|line| count_leading_whitespace(line))
        .collect();
    let non_empty_indents: Vec<usize> = lines
        .iter()
        .zip(&indents)
        .filter_map(|(line, indent)| is_non_empty_line(line).then_some(*indent))
        .collect();
    let min_indent = non_empty_indents.iter().copied().min().unwrap_or(0);
    let indent_unit = non_empty_indents
        .iter()
        .filter_map(|indent| indent.checked_sub(min_indent))
        .filter(|step| *step > 0)
        .min()
        .unwrap_or(1);
    lines
        .iter()
        .zip(indents)
        .map(|(line, indent)| {
            if !is_non_empty_line(line) || indent_unit == 0 {
                0
            } else {
                ((indent - min_indent) as f64 / indent_unit as f64).round() as usize
            }
        })
        .collect()
}

fn normalize_lines(lines: &[&str], include_depth: bool) -> Vec<String> {
    let depths = include_depth.then(|| relative_indent_depths(lines));
    lines
        .iter()
        .enumerate()
        .map(|(index, line)| {
            let trimmed = js_trim(line);
            let prefix = depths
                .as_ref()
                .map_or_else(|| "|".to_owned(), |values| format!("{}|", values[index]));
            if trimmed.is_empty() {
                prefix
            } else {
                prefix + &normalize_for_fuzzy(trimmed)
            }
        })
        .collect()
}

fn line_offsets(lines: &[&str]) -> Vec<usize> {
    let mut offsets = Vec::with_capacity(lines.len());
    let mut offset = 0;
    for (index, line) in lines.iter().enumerate() {
        offsets.push(offset);
        offset += line.len() + usize::from(index + 1 < lines.len());
    }
    offsets
}

#[derive(Debug)]
struct BestFuzzyMatch {
    best: Option<FuzzyMatch>,
    above_threshold_count: usize,
    second_best_score: f64,
}

fn best_fuzzy_match_core(
    content_lines: &[&str],
    target_lines: &[&str],
    offsets: &[usize],
    threshold: f64,
    include_depth: bool,
    excluded_ranges: &[ExcludedRange],
) -> BestFuzzyMatch {
    let target_normalized = normalize_lines(target_lines, include_depth);
    let mut best = None;
    let mut best_score = -1.0;
    let mut second_best_score = -1.0;
    let mut above_threshold_count = 0;
    for start in 0..=content_lines.len() - target_lines.len() {
        let start_index = offsets[start];
        let end_line = start + target_lines.len() - 1;
        let end_index = (offsets[end_line] + content_lines[end_line].len()).max(start_index + 1);
        if overlaps_excluded(start_index, end_index, excluded_ranges) {
            continue;
        }
        let window = &content_lines[start..start + target_lines.len()];
        let window_normalized = normalize_lines(window, include_depth);
        let score = target_normalized
            .iter()
            .zip(&window_normalized)
            .map(|(target, actual)| similarity(target, actual))
            .sum::<f64>()
            / target_lines.len() as f64;
        if score >= threshold {
            above_threshold_count += 1;
        }
        if score > best_score {
            second_best_score = best_score;
            best_score = score;
            best = Some(FuzzyMatch {
                actual_text: window.join("\n"),
                start_index,
                start_line: start as u32 + 1,
                confidence: score,
            });
        } else if score > second_best_score {
            second_best_score = score;
        }
    }
    BestFuzzyMatch {
        best,
        above_threshold_count,
        second_best_score,
    }
}

fn best_fuzzy_match(
    content: &str,
    target: &str,
    threshold: f64,
    excluded_ranges: &[ExcludedRange],
) -> BestFuzzyMatch {
    let content_lines: Vec<&str> = content.split('\n').collect();
    let target_lines: Vec<&str> = target.split('\n').collect();
    if target.is_empty() || target_lines.len() > content_lines.len() {
        return BestFuzzyMatch {
            best: None,
            above_threshold_count: 0,
            second_best_score: 0.0,
        };
    }
    let offsets = line_offsets(&content_lines);
    let mut result = best_fuzzy_match_core(
        &content_lines,
        &target_lines,
        &offsets,
        threshold,
        true,
        excluded_ranges,
    );
    if result
        .best
        .as_ref()
        .is_some_and(|best| best.confidence < threshold && best.confidence >= FALLBACK_THRESHOLD)
    {
        let without_depth = best_fuzzy_match_core(
            &content_lines,
            &target_lines,
            &offsets,
            threshold,
            false,
            excluded_ranges,
        );
        if without_depth.best.as_ref().is_some_and(|candidate| {
            result
                .best
                .as_ref()
                .is_none_or(|best| candidate.confidence > best.confidence)
        }) {
            result = without_depth;
        }
    }
    result
}

/// Locate `target` in `content`: exact first, then fuzzy when allowed.
/// Excluded ranges are invisible to both passes.
pub fn find_match(content: &str, target: &str, options: &FindMatchOptions<'_>) -> MatchOutcome {
    if target.is_empty() {
        return MatchOutcome::default();
    }
    if let Some(exact) = find_exact_match_outcome(content, target, options.excluded_ranges) {
        return exact;
    }
    let threshold = options.threshold.unwrap_or(DEFAULT_FUZZY_THRESHOLD);
    let result = best_fuzzy_match(content, target, threshold, options.excluded_ranges);
    let Some(best) = result.best else {
        return MatchOutcome::default();
    };
    if options.allow_fuzzy && best.confidence >= threshold {
        if result.above_threshold_count == 1 {
            return MatchOutcome {
                matched: Some(best.clone()),
                closest: Some(best),
                ..MatchOutcome::default()
            };
        }
        if result.above_threshold_count > 1
            && best.confidence >= DOMINANT_FUZZY_MIN_CONFIDENCE
            && best.confidence - result.second_best_score >= DOMINANT_FUZZY_DELTA
        {
            return MatchOutcome {
                matched: Some(best.clone()),
                closest: Some(best),
                fuzzy_matches: Some(result.above_threshold_count),
                dominant_fuzzy: Some(true),
                ..MatchOutcome::default()
            };
        }
    }
    MatchOutcome {
        closest: Some(best),
        fuzzy_matches: Some(result.above_threshold_count),
        ..MatchOutcome::default()
    }
}
