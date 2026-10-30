//! The bit-parallel distance and the bounded reference agree on every input,
//! including patterns that straddle the 64-row word boundaries.

use super::super::levenshtein::levenshtein_chars;
use super::super::{PatternDistance, levenshtein_within};

#[test]
fn fast_distances_match_reference_across_block_boundaries() {
    // Patterns straddling the 64-row word boundaries of the bit-parallel
    // distance, with non-ASCII rows and texts shorter, equal and longer.
    let mut state = 0x5eed_u64;
    let mut next = |bound: usize| {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1);
        ((state >> 33) as usize) % bound
    };
    let alphabet = ['a', 'b', 'é', '界'];
    for pattern_len in [0, 1, 63, 64, 65, 127, 128, 129] {
        for _ in 0..40 {
            let pattern: Vec<char> = (0..pattern_len).map(|_| alphabet[next(4)]).collect();
            let mut text = pattern.clone();
            for _ in 0..next(12) {
                let at = next(text.len() + 1);
                match next(3) {
                    0 => text.insert(at, alphabet[next(4)]),
                    1 if at < text.len() => drop(text.remove(at)),
                    _ if at < text.len() => text[at] = alphabet[next(4)],
                    _ => {}
                }
            }
            let expected = levenshtein_chars(&pattern, &text);
            assert_eq!(PatternDistance::new(&pattern).distance(&text), expected);
            for max in [0, expected.saturating_sub(1), expected, expected + 3] {
                assert_eq!(
                    levenshtein_within(&pattern, &text, max),
                    (expected <= max).then_some(expected),
                    "len {pattern_len} max {max}"
                );
            }
        }
    }
}
