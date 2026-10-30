//! The occurrence tally shared by the sequence and context ladders: how many
//! indices matched, the first one, and the first few recorded ones.

use super::MAX_RECORDED_MATCHES;

#[derive(Debug, Default)]
pub(super) struct IndexedMatches {
    pub(super) first_match: Option<usize>,
    pub(super) match_count: usize,
    pub(super) match_indices: Vec<usize>,
}

pub(super) fn collect_indexed_matches(
    start: usize,
    end_inclusive: usize,
    mut predicate: impl FnMut(usize) -> bool,
) -> IndexedMatches {
    let mut matches = IndexedMatches::default();
    if start > end_inclusive {
        return matches;
    }
    for index in start..=end_inclusive {
        if !predicate(index) {
            continue;
        }
        matches.first_match.get_or_insert(index);
        matches.match_count += 1;
        if matches.match_indices.len() < MAX_RECORDED_MATCHES {
            matches.match_indices.push(index);
        }
    }
    matches
}
