//! Pattern-to-many-texts distance: Myers' bit-parallel algorithm, plus the
//! `[0, 1]` similarity two callers compare against a threshold.

use std::collections::HashMap;

use super::levenshtein::levenshtein_chars;

/// `levenshtein_distance` from one fixed pattern to many texts.
///
/// Myers' bit-parallel algorithm with Hyyrö's 64-row blocks costs
/// O(text · ⌈pattern / 64⌉) word operations once the pattern's character
/// masks are built.
pub struct PatternDistance {
    len: usize,
    words: usize,
    /// Masks for ASCII characters, `words` per code point.
    ascii: Vec<u64>,
    other: HashMap<char, Vec<u64>>,
    zeros: Vec<u64>,
    /// The last pattern row's bit in the last word.
    last_bit: u64,
}

impl PatternDistance {
    pub fn new(pattern: &[char]) -> Self {
        let len = pattern.len();
        let words = len.div_ceil(64).max(1);
        let mut ascii = vec![0; 128 * words];
        let mut other: HashMap<char, Vec<u64>> = HashMap::new();
        for (row, &character) in pattern.iter().enumerate() {
            let (word, bit) = (row / 64, 1u64 << (row % 64));
            if character.is_ascii() {
                ascii[character as usize * words + word] |= bit;
            } else {
                other.entry(character).or_insert_with(|| vec![0; words])[word] |= bit;
            }
        }
        let last_bit = 1u64 << (len.saturating_sub(1) % 64);
        Self {
            len,
            words,
            ascii,
            other,
            zeros: vec![0; words],
            last_bit,
        }
    }

    /// Edit distance between the pattern and `text`.
    pub fn distance(&self, text: &[char]) -> usize {
        if self.len == 0 {
            return text.len();
        }
        let words = self.words;
        // Vertical deltas between adjacent rows: all +1 in column 0.
        let mut positive = vec![!0u64; words];
        let mut negative = vec![0u64; words];
        let mut score = self.len;
        for &character in text {
            let masks = if character.is_ascii() {
                &self.ascii[character as usize * words..][..words]
            } else {
                self.other
                    .get(&character)
                    .map_or(&self.zeros[..], Vec::as_slice)
            };
            // Global distance: the top row grows by one per text character.
            let (mut carry_positive, mut carry_negative) = (true, false);
            for word in 0..words {
                let (vertical_positive, vertical_negative) = (positive[word], negative[word]);
                let mut equal = masks[word];
                let vertical = equal | vertical_negative;
                if carry_negative {
                    equal |= 1;
                }
                let horizontal = ((equal & vertical_positive).wrapping_add(vertical_positive)
                    ^ vertical_positive)
                    | equal;
                let mut horizontal_positive = vertical_negative | !(horizontal | vertical_positive);
                let mut horizontal_negative = vertical_positive & horizontal;
                let high = if word + 1 == words {
                    self.last_bit
                } else {
                    1 << 63
                };
                let out_positive = horizontal_positive & high != 0;
                let out_negative = horizontal_negative & high != 0;
                horizontal_positive = (horizontal_positive << 1) | u64::from(carry_positive);
                horizontal_negative = (horizontal_negative << 1) | u64::from(carry_negative);
                positive[word] = horizontal_negative | !(vertical | horizontal_positive);
                negative[word] = horizontal_positive & vertical;
                (carry_positive, carry_negative) = (out_positive, out_negative);
            }
            if carry_positive {
                score += 1;
            } else if carry_negative {
                score -= 1;
            }
        }
        score
    }
}

/// Similarity in `[0, 1]`: `1 - distance / max_len`.
pub fn similarity(a: &str, b: &str) -> f64 {
    let a_chars: Vec<char> = a.chars().collect();
    let b_chars: Vec<char> = b.chars().collect();
    let max_len = a_chars.len().max(b_chars.len());
    if max_len == 0 {
        return 1.0;
    }
    1.0 - levenshtein_chars(&a_chars, &b_chars) as f64 / max_len as f64
}
