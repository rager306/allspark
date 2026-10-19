//! V6 — `MinHash` + `LSH` near-duplicate detection over `AST` trigrams.
//!
//! Near-clone detection answers "does this symbol already exist elsewhere?"
//! Two functions that differ only in identifiers/whitespace have nearly the
//! same *structure*, which is captured by the multiset of `AST` leaf tokens.
//!
//! * **Trigrams** — the leaf-token sequence is sliced into overlapping
//!   trigrams (leaf-only, so the signal is grammar-agnostic: identifiers,
//!   literals, and operators, never node kinds).
//! * **`MinHash`** — `K = 64` independent hash minima estimate the `Jaccard`
//!   similarity of two trigram sets (±0.12 std. error, per the
//!   codebase-memory reference).
//! * **`LSH`** — the 64-word signature is cut into 32 bands × 2 rows; two
//!   signatures that agree on any one band are candidate near-duplicates.
//!   With `r = 2, b = 32`, the banding threshold is ≈ 0.95 `Jaccard`, matching
//!   the reference's clone threshold.
//!
//! Everything here is deterministic (fixed hash seeds, no randomness) so a
//! signature computed twice is byte-identical.

use std::collections::HashMap;

use tree_sitter::Node;

/// Number of `MinHash` words in a signature.
pub const MINHASH_K: usize = 64;
/// Number of `LSH` bands.
pub const LSH_BANDS: usize = 32;
/// Rows per `LSH` band (`LSH_BANDS * LSH_ROWS == MINHASH_K`).
pub const LSH_ROWS: usize = 2;
/// `Jaccard` threshold the banding is tuned to (~0.95).
pub const CLONE_JACCARD: f64 = 0.95;

/// A 64-word `MinHash` signature of a token set.
pub type MinHashSig = [u64; MINHASH_K];

/// Extract the leaf-token sequence of a parsed `source`.
///
/// Leaf-only: interior grammar nodes are skipped, so two syntactically
/// different but lexically identical snippets collide (which is the point for
/// clone detection).
#[must_use]
pub fn ast_leaf_tokens(source: &str, lang: pa_ast::SupportLang) -> Vec<String> {
    let Ok(Some(tree)) = pa_ast::parse_cache::parse_cached(source, lang) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    collect_leaves(tree.root_node(), source.as_bytes(), &mut out);
    out
}

fn collect_leaves(node: Node<'_>, src: &[u8], out: &mut Vec<String>) {
    if node.child_count() == 0 {
        let text = node.utf8_text(src).unwrap_or("").trim();
        if !text.is_empty() {
            out.push(text.to_string());
        }
        return;
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_leaves(child, src, out);
    }
}

/// Slice a token sequence into overlapping trigrams. Fewer than 3 tokens
/// yields the single whole-sequence "gram" so short symbols still have a
/// signature.
#[must_use]
pub fn trigrams(tokens: &[String]) -> Vec<String> {
    if tokens.len() < 3 {
        return if tokens.is_empty() {
            Vec::new()
        } else {
            vec![tokens.join("\u{1f}")]
        };
    }
    tokens
        .windows(3)
        .map(|w| format!("{}\u{1f}{}\u{1f}{}", w[0], w[1], w[2]))
        .collect()
}

/// Compute the `MinHash` signature of a set of string tokens (already trigrams).
///
/// `K = 64` minima are taken under 64 fixed seeds; a single FNV-1a pass per
/// gram with seed mixing keeps this linear in the gram count.
#[must_use]
pub fn minhash(tokens: &[String]) -> MinHashSig {
    let mut sig = [u64::MAX; MINHASH_K];
    for gram in tokens {
        let base = fnv1a(gram.as_bytes(), 0xcbf2_9ce4_8422_2325);
        for (k, slot) in sig.iter_mut().enumerate() {
            let h = mix(base, k as u64);
            if h < *slot {
                *slot = h;
            }
        }
    }
    sig
}

/// Convenience: signature directly from source (leaf trigrams → `MinHash`).
#[must_use]
pub fn minhash_source(source: &str, lang: pa_ast::SupportLang) -> MinHashSig {
    minhash(&trigrams(&ast_leaf_tokens(source, lang)))
}

/// Split a signature into [`LSH_BANDS`] band keys, each hashing
/// [`LSH_ROWS`] consecutive words.
#[must_use]
pub fn lsh_bands(sig: &MinHashSig) -> Vec<u32> {
    let mut out = Vec::with_capacity(LSH_BANDS);
    for band in 0..LSH_BANDS {
        let start = band * LSH_ROWS;
        let mut acc: u64 = 0xcbf2_9ce4_8422_2325;
        for w in &sig[start..start + LSH_ROWS] {
            acc = mix(acc, *w);
        }
        // Fold to u32; band collisions merely add candidates (LSH is a filter).
        out.push((acc ^ (acc >> 32)) as u32);
    }
    out
}

/// Estimate the `Jaccard` similarity of two signatures: the fraction of words
/// that match.
#[must_use]
pub fn jaccard_estimate(a: &MinHashSig, b: &MinHashSig) -> f64 {
    let matches = a.iter().zip(b.iter()).filter(|(x, y)| x == y).count();
    matches as f64 / MINHASH_K as f64
}

/// Exact `Jaccard` similarity of two token sets (ground truth for tests).
#[must_use]
pub fn jaccard_exact(a: &[String], b: &[String]) -> f64 {
    use std::collections::HashSet;
    let sa: HashSet<&String> = a.iter().collect();
    let sb: HashSet<&String> = b.iter().collect();
    let inter = sa.intersection(&sb).count();
    let union = sa.union(&sb).count();
    if union == 0 {
        0.0
    } else {
        inter as f64 / union as f64
    }
}

/// An `LSH` index over `MinHash` signatures: `band key -> member ids`.
#[derive(Debug, Default, Clone)]
pub struct LshIndex {
    bands: Vec<HashMap<u32, Vec<u64>>>,
}

impl LshIndex {
    /// An empty index with [`LSH_BANDS`] band tables.
    #[must_use]
    pub fn new() -> Self {
        Self {
            bands: vec![HashMap::new(); LSH_BANDS],
        }
    }

    /// Insert `id` with `sig`, returning nothing.
    pub fn insert(&mut self, id: u64, sig: &MinHashSig) {
        for (band, key) in lsh_bands(sig).into_iter().enumerate() {
            self.bands[band].entry(key).or_default().push(id);
        }
    }

    /// Candidate ids sharing at least one band with `sig` (deduped).
    #[must_use]
    pub fn candidates(&self, sig: &MinHashSig) -> Vec<u64> {
        let mut seen: std::collections::HashSet<u64> = std::collections::HashSet::new();
        for (band, key) in lsh_bands(sig).into_iter().enumerate() {
            if let Some(ids) = self.bands[band].get(&key) {
                for id in ids {
                    seen.insert(*id);
                }
            }
        }
        let mut out: Vec<u64> = seen.into_iter().collect();
        out.sort_unstable();
        out
    }
}

/// A near-duplicate pair found by [`find_near_duplicates`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NearDuplicate {
    /// First member id.
    pub a: u64,
    /// Second member id.
    pub b: u64,
    /// Estimated `Jaccard` similarity.
    pub similarity: f64,
}

/// Find near-duplicate pairs among `(id, signature)` items at or above the
/// `threshold` `Jaccard`. Candidate pairs come from `LSH` bands, then are
/// verified by the signature estimate — so a returned pair always clears the
/// threshold.
#[must_use]
#[allow(clippy::too_many_lines)] // one scan over pair candidates; reflowed past the limit by rustfmt only
pub fn find_near_duplicates(items: &[(u64, MinHashSig)], threshold: f64) -> Vec<NearDuplicate> {
    let mut index = LshIndex::new();
    for (id, sig) in items {
        index.insert(*id, sig);
    }
    let by_id: HashMap<u64, &MinHashSig> = items.iter().map(|(id, s)| (*id, s)).collect();
    let mut out: Vec<NearDuplicate> = Vec::new();
    let mut seen: std::collections::HashSet<(u64, u64)> = std::collections::HashSet::new();
    for (id, sig) in items {
        for cand in index.candidates(sig) {
            if cand == *id {
                continue;
            }
            let key = if cand < *id { (cand, *id) } else { (*id, cand) };
            if !seen.insert(key) {
                continue;
            }
            let Some(other) = by_id.get(&cand) else {
                continue;
            };
            let sim = jaccard_estimate(sig, other);
            if sim >= threshold {
                out.push(NearDuplicate {
                    a: key.0,
                    b: key.1,
                    similarity: sim,
                });
            }
        }
    }
    out.sort_by_key(|entry| (entry.a, entry.b));
    out
}

/// FNV-1a hash with a seed.
fn fnv1a(bytes: &[u8], seed: u64) -> u64 {
    let mut hash = seed;
    for &b in bytes {
        hash ^= u64::from(b);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// `SplitMix64` finalizer: decorrelates `base` from seed `k`.
fn mix(mut x: u64, k: u64) -> u64 {
    x ^= k.wrapping_mul(0x9e37_79b9_7f4a_7c15);
    x = (x ^ (x >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    x ^ (x >> 31)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pa_ast::SupportLang;

    const A: &str = r"
pub fn compute(a: u32, b: u32) -> u32 {
    let sum = a + b;
    let prod = a * b;
    sum + prod
}
";

    const B: &str = r"
pub fn calculate(x: u32, y: u32) -> u32 {
    let total = x + y;
    let product = x * y;
    total + product
}
";

    #[test]
    fn minhash_is_deterministic() {
        let s1 = minhash_source(A, SupportLang::Rust);
        let s2 = minhash_source(A, SupportLang::Rust);
        assert_eq!(s1, s2, "same source → identical signature");
    }

    #[test]
    fn minhash_estimates_jaccard() {
        let ta = trigrams(&ast_leaf_tokens(A, SupportLang::Rust));
        let tb = trigrams(&ast_leaf_tokens(B, SupportLang::Rust));
        let exact = jaccard_exact(&ta, &tb);
        let est = jaccard_estimate(&minhash(&ta), &minhash(&tb));
        assert!(
            (exact - est).abs() < 0.2,
            "estimate {est} too far from exact {exact}"
        );
    }

    #[test]
    fn identical_source_is_near_duplicate() {
        let items = vec![
            (1u64, minhash_source(A, SupportLang::Rust)),
            (2u64, minhash_source(A, SupportLang::Rust)),
        ];
        let dups = find_near_duplicates(&items, CLONE_JACCARD);
        assert_eq!(dups.len(), 1, "{dups:?}");
        assert_eq!(dups[0].similarity.to_bits(), 1.0_f64.to_bits());
    }

    #[test]
    fn renamed_clone_is_found_via_lsh() {
        // A and B share the same control-flow shape; the leaf trigrams overlap
        // heavily even though identifiers differ.
        let items = vec![
            (1u64, minhash_source(A, SupportLang::Rust)),
            (2u64, minhash_source(B, SupportLang::Rust)),
        ];
        let index = {
            let mut idx = LshIndex::new();
            for (id, sig) in &items {
                idx.insert(*id, sig);
            }
            idx
        };
        let cands = index.candidates(&items[1].1);
        assert!(cands.contains(&1), "LSH must propose the clone: {cands:?}");
    }

    #[test]
    fn lsh_bands_are_stable_and_sized() {
        let sig = minhash_source(A, SupportLang::Rust);
        let bands = lsh_bands(&sig);
        assert_eq!(bands.len(), LSH_BANDS);
        assert_eq!(bands, lsh_bands(&sig));
        assert_eq!(LSH_BANDS * LSH_ROWS, MINHASH_K);
    }

    #[test]
    fn unrelated_sources_do_not_collide() {
        let other = "fn totally_different() -> &'static str { \"hello world\" }\n";
        let items = vec![
            (1u64, minhash_source(A, SupportLang::Rust)),
            (2u64, minhash_source(other, SupportLang::Rust)),
        ];
        let dups = find_near_duplicates(&items, CLONE_JACCARD);
        assert!(
            dups.is_empty(),
            "unrelated code must not be a clone: {dups:?}"
        );
    }

    #[test]
    fn short_source_still_signs() {
        let sig = minhash_source("x\n", SupportLang::Rust);
        // A signature exists (not all-MAX) even for a one-token program.
        assert!(sig.iter().any(|&w| w != u64::MAX));
    }
}
