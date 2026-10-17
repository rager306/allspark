//! LSP-surface cache (L11): per-file consumed surface + a reference Bloom
//! filter, persisted in SQLite and invalidated by surface hash.
//!
//! A file's *surface* is what other files consume from it: its definitions
//! (name/kind/line) plus the names it references (callees, imports). The
//! canonical definition JSON is hashed to `surface_sha`; when a freshly
//! computed `surface_sha` equals the stored one, the cached row is valid and
//! the expensive extraction can be skipped.
//!
//! The `ref_bloom` column is a Bloom filter over the file's reference names: a
//! cheap, false-positive-only membership test answering "could this file
//! reference `foo`?" without touching the graph. It is a *filter*, never a
//! proof — a negative is exact, a positive needs confirmation.
//!
//! Canonicalization note: `serde_json` is built with `preserve_order`, so a
//! `Value` map keeps insertion order rather than sorting keys. The definition
//! JSON is therefore rendered by hand (fixed key order, defs sorted by
//! `(kind, name, start_line)`) so byte equality is hash equality.

use std::collections::BTreeSet;

use pa_ast::SupportLang;
use serde::{Deserialize, Serialize};

use crate::error::Result;
use crate::store::GraphStore;

/// One definition in a file's LSP surface.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SurfaceDef {
    /// Definition kind (e.g. `Function`, `Struct`).
    pub kind: String,
    /// Short name.
    pub name: String,
    /// 1-based start line.
    pub start_line: u32,
}

/// A persisted `lsp_surface` row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LspSurfaceRow {
    /// Project the row belongs to.
    pub project: String,
    /// Repo-relative path.
    pub rel_path: String,
    /// Canonical definition JSON.
    pub defs_json: String,
    /// Hash of the file surface (the cache key).
    pub surface_sha: String,
    /// Hex-encoded reference Bloom filter.
    pub ref_bloom: String,
    /// Last update timestamp.
    pub updated_at: String,
}

/// The result of a cache lookup: definitions, hash, and reference filter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CachedSurface {
    /// Definitions of the file, canonically ordered.
    pub defs: Vec<SurfaceDef>,
    /// Surface hash (the cache key).
    pub surface_sha: String,
    /// Reference-name Bloom filter.
    pub ref_bloom: RefBloom,
    /// `true` when the row was served from the cache unchanged.
    pub from_cache: bool,
}

/// A small, deterministic Bloom filter over reference names.
///
/// Sized from the expected item count; `k` probes are derived from a single
/// FNV-1a hash (double hashing: `h_i = h1 + i·h2`). Serializable to hex so it
/// round-trips through SQLite.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefBloom {
    bits: Vec<u8>,
    k: u32,
}

/// Default false-positive rate target for the reference filter.
const DEFAULT_FP_PER_MILLE: u32 = 10; // 1%

impl RefBloom {
    /// Build a filter sized for `expected` items at ~1% false positives.
    #[must_use]
    pub fn new(expected: usize) -> Self {
        Self::with_fp_per_mille(expected, DEFAULT_FP_PER_MILLE)
    }

    /// Build a filter sized for `expected` items at the given false-positive
    /// rate in parts per thousand (clamped to `[1, 500]`).
    #[must_use]
    pub fn with_fp_per_mille(expected: usize, fp_per_mille: u32) -> Self {
        let n = expected.max(1) as f64;
        // m = -(n ln p) / (ln 2)^2, k = (m/n) ln 2.
        let p = (f64::from(fp_per_mille.clamp(1, 500)) / 1000.0).max(1e-4);
        let ln2 = std::f64::consts::LN_2;
        let m = (-(n * p.ln()) / (ln2 * ln2)).ceil() as usize;
        let m = m.clamp(64, 1 << 20);
        let k = ((m as f64 / n) * ln2).round().clamp(1.0, 16.0) as u32;
        Self {
            bits: vec![0u8; m.div_ceil(8)],
            k,
        }
    }

    /// The filter's bit width.
    #[must_use]
    pub fn bit_len(&self) -> usize {
        self.bits.len() * 8
    }

    /// The number of probe functions.
    #[must_use]
    pub fn probes(&self) -> u32 {
        self.k
    }

    /// Build a filter from a set of reference names.
    #[must_use]
    pub fn from_names(names: &[String]) -> Self {
        let mut filter = Self::new(names.len());
        for name in names {
            filter.insert(name);
        }
        filter
    }

    /// Insert `name` into the filter.
    pub fn insert(&mut self, name: &str) {
        let (h1, h2) = Self::hashes(name);
        for i in 0..self.k {
            let bit =
                (h1.wrapping_add(u64::from(i).wrapping_mul(h2)) % (self.bit_len() as u64)) as usize;
            self.bits[bit / 8] |= 1 << (bit % 8);
        }
    }

    /// Test membership. A `false` result is exact; `true` may be a false
    /// positive.
    #[must_use]
    pub fn contains(&self, name: &str) -> bool {
        let (h1, h2) = Self::hashes(name);
        for i in 0..self.k {
            let bit =
                (h1.wrapping_add(u64::from(i).wrapping_mul(h2)) % (self.bit_len() as u64)) as usize;
            if self.bits[bit / 8] & (1 << (bit % 8)) == 0 {
                return false;
            }
        }
        true
    }

    /// Self-describing encoding: `"{k}:{hex}"` (the probe count travels with
    /// the bit array, so a decoded filter is immediately usable).
    #[must_use]
    pub fn to_hex(&self) -> String {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        let mut hex = String::with_capacity(self.bits.len() * 2);
        for &b in &self.bits {
            hex.push(HEX[(b >> 4) as usize] as char);
            hex.push(HEX[(b & 0x0f) as usize] as char);
        }
        format!("{}:{}", self.k, hex)
    }

    /// Decode a filter previously produced by [`RefBloom::to_hex`]. Returns
    /// `None` for a malformed encoding.
    #[must_use]
    pub fn from_hex(encoded: &str) -> Option<Self> {
        let (k_str, hex) = encoded.split_once(':')?;
        let k: u32 = k_str.parse().ok()?;
        if hex.is_empty() || hex.len() % 2 != 0 {
            return None;
        }
        let mut bits = Vec::with_capacity(hex.len() / 2);
        let bytes = hex.as_bytes();
        for pair in bytes.chunks(2) {
            let hi = (pair[0] as char).to_digit(16)?;
            let lo = (pair[1] as char).to_digit(16)?;
            bits.push(((hi << 4) | lo) as u8);
        }
        Some(Self {
            bits,
            k: k.clamp(1, 16),
        })
    }

    fn hashes(name: &str) -> (u64, u64) {
        let h1 = fnv1a(name.as_bytes(), 0xcbf2_9ce4_8422_2325);
        let h2 = fnv1a(name.as_bytes(), 0x9e37_79b9_7f4a_7c15) | 1; // odd → coprime with 2^n
        (h1, h2)
    }
}

fn fnv1a(bytes: &[u8], seed: u64) -> u64 {
    let mut hash = seed;
    for &b in bytes {
        hash ^= u64::from(b);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// Render definition JSON canonically: fixed key order, defs sorted by
/// `(kind, name, start_line)`.
#[must_use]
pub fn canonical_defs_json(defs: &[SurfaceDef]) -> String {
    let mut sorted: Vec<&SurfaceDef> = defs.iter().collect();
    sorted.sort_by(|a, b| {
        a.kind
            .cmp(&b.kind)
            .then_with(|| a.name.cmp(&b.name))
            .then_with(|| a.start_line.cmp(&b.start_line))
    });
    let mut out = String::from("[");
    for (i, d) in sorted.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push_str("{\"k\":\"");
        out.push_str(&escape(&d.kind));
        out.push_str("\",\"n\":\"");
        out.push_str(&escape(&d.name));
        out.push_str("\",\"l\":");
        out.push_str(&d.start_line.to_string());
        out.push('}');
    }
    out.push(']');
    out
}

fn escape(value: &str) -> String {
    use std::fmt::Write as _;
    let mut out = String::with_capacity(value.len());
    for c in value.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out
}

/// Compute the definitions and reference names of `source` from the AST.
#[must_use]
pub fn surface_parts(
    source: &str,
    lang: SupportLang,
    module: &str,
) -> (Vec<SurfaceDef>, Vec<String>) {
    let surface = crate::surface::surface_of(source, lang, module);
    let defs: Vec<SurfaceDef> = surface
        .symbols
        .iter()
        .map(|s| SurfaceDef {
            kind: s.kind.clone(),
            name: s.name.clone(),
            start_line: s.start_line,
        })
        .collect();
    // References: callee names + imported target names, deduped and sorted.
    let extracted = crate::extract::extract(source, lang, module);
    let mut refs: BTreeSet<String> = BTreeSet::new();
    for call in &extracted.calls {
        refs.insert(call.callee.clone());
    }
    for import in &extracted.imports {
        refs.insert(import.target_name.clone());
    }
    (defs, refs.into_iter().collect())
}

/// Fetch or recompute a file's LSP surface, storing it when the surface hash
/// changed (or the row is absent).
///
/// The cache is keyed by `surface_sha`: a stored row whose hash matches the
/// freshly computed one is returned as-is (`from_cache = true`) without
/// re-deriving the reference filter.
///
/// # Errors
/// Propagates SQLite errors.
pub fn cache_lsp_surface(
    store: &GraphStore,
    project: &str,
    rel_path: &str,
    source: &str,
    lang: SupportLang,
) -> Result<CachedSurface> {
    let module = module_of(rel_path);
    let (defs, refs) = surface_parts(source, lang, &module);
    let defs_json = canonical_defs_json(&defs);
    let surface_sha = crate::surface::surface_hash(source, lang, &module);

    if let Some(row) = store.lsp_surface(project, rel_path)? {
        if row.surface_sha == surface_sha {
            // Cache hit: rebuild the in-memory filter from the stored hex.
            let ref_bloom =
                RefBloom::from_hex(&row.ref_bloom).unwrap_or_else(|| RefBloom::from_names(&refs));
            return Ok(CachedSurface {
                defs,
                surface_sha,
                ref_bloom,
                from_cache: true,
            });
        }
    }

    let ref_bloom = RefBloom::from_names(&refs);
    store.put_lsp_surface(
        project,
        rel_path,
        &defs_json,
        &surface_sha,
        &ref_bloom.to_hex(),
    )?;
    Ok(CachedSurface {
        defs,
        surface_sha,
        ref_bloom,
        from_cache: false,
    })
}

/// Module prefix for a file path, matching the indexer's convention.
fn module_of(rel: &str) -> String {
    let without_ext = rel.rsplit_once('.').map_or(rel, |(stem, _)| stem);
    without_ext.replace('/', "::")
}

#[cfg(test)]
mod tests {
    use super::*;

    const SRC: &str = r"
use crate::util::Helper;

pub fn parse_config(path: &str) -> u32 {
    let h = Helper::new();
    h.load(path)
}
";

    #[test]
    fn bloom_no_false_negatives() {
        let names = vec!["foo".to_string(), "bar".to_string(), "baz".to_string()];
        let bloom = RefBloom::from_names(&names);
        for n in &names {
            assert!(bloom.contains(n), "inserted name must be present: {n}");
        }
    }

    #[test]
    fn bloom_false_positive_rate_is_low() {
        let names: Vec<String> = (0..500).map(|i| format!("name_{i}")).collect();
        let bloom = RefBloom::from_names(&names);
        let mut fp = 0;
        let probes = 5000;
        for i in 0..probes {
            let probe = format!("absent_{i}");
            if bloom.contains(&probe) {
                fp += 1;
            }
        }
        // ~1% target; allow generous slack for small samples.
        assert!(fp < probes / 20, "false positives too high: {fp}/{probes}");
    }

    #[test]
    fn bloom_hex_round_trips() {
        let names = vec!["alpha".to_string(), "beta".to_string()];
        let bloom = RefBloom::from_names(&names);
        let restored = RefBloom::from_hex(&bloom.to_hex()).expect("decode");
        assert_eq!(bloom, restored);
        assert!(restored.contains("alpha"));
    }

    #[test]
    fn canonical_json_is_sorted_and_stable() {
        let a = vec![
            SurfaceDef {
                kind: "Function".into(),
                name: "b".into(),
                start_line: 2,
            },
            SurfaceDef {
                kind: "Function".into(),
                name: "a".into(),
                start_line: 1,
            },
        ];
        let mut b = a.clone();
        b.reverse();
        assert_eq!(canonical_defs_json(&a), canonical_defs_json(&b));
        assert_eq!(
            canonical_defs_json(&a),
            "[{\"k\":\"Function\",\"n\":\"a\",\"l\":1},{\"k\":\"Function\",\"n\":\"b\",\"l\":2}]"
        );
    }

    #[test]
    fn cache_hit_on_unchanged_surface() {
        let store = GraphStore::open_in_memory().expect("store");
        store.register_project("p", "/tmp").expect("project");
        let first = cache_lsp_surface(&store, "p", "m.rs", SRC, SupportLang::Rust).expect("first");
        assert!(!first.from_cache);
        assert_eq!(store.lsp_surface_count("p").expect("count"), 1);
        // A body-only edit keeps the surface hash → cache hit.
        let edited = SRC.replace("h.load(path)", "h.load(path); // body edit");
        let second =
            cache_lsp_surface(&store, "p", "m.rs", &edited, SupportLang::Rust).expect("second");
        assert!(second.from_cache, "surface unchanged → cache hit");
        assert_eq!(second.surface_sha, first.surface_sha);
    }

    #[test]
    fn cache_miss_on_changed_surface() {
        let store = GraphStore::open_in_memory().expect("store");
        store.register_project("p", "/tmp").expect("project");
        cache_lsp_surface(&store, "p", "m.rs", SRC, SupportLang::Rust).expect("first");
        let renamed = SRC.replace("fn parse_config", "fn load_config");
        let second =
            cache_lsp_surface(&store, "p", "m.rs", &renamed, SupportLang::Rust).expect("second");
        assert!(!second.from_cache, "surface changed → cache miss");
    }

    #[test]
    fn reference_names_include_callees_and_imports() {
        let (defs, refs) = surface_parts(SRC, SupportLang::Rust, "m");
        assert!(defs.iter().any(|d| d.name == "parse_config"));
        assert!(
            refs.contains(&"Helper".to_string()),
            "import target: {refs:?}"
        );
        assert!(refs.contains(&"load".to_string()), "callee: {refs:?}");
    }
}
