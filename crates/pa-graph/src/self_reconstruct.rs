//! R21 — self-reconstructing index: checksum detection plus targeted rebuild.
//!
//! A persisted index can rot: a truncated BLOB, a half-written row, a byte
//! flipped on disk. [`SelfReconstructingIndex`] stores a per-part checksum and
//! detects corruption by re-hashing the part; [`verify`](SelfReconstructingIndex::verify)
//! reports the damaged parts and [`rebuild_corrupt`](SelfReconstructingIndex::rebuild_corrupt)
//! rebuilds exactly those from the source, leaving healthy parts untouched.

use std::collections::BTreeMap;

/// The checksum algorithm used for parts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ChecksumKind {
    /// BLAKE3 (default; fast and cryptographic).
    #[default]
    Blake3,
    /// A small FNV-1a 64-bit checksum (cheap, non-cryptographic).
    Fnv1a,
}

/// One index part: a name, its bytes, and the expected checksum.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Part {
    /// Part name (e.g. a file path or shard id).
    pub name: String,
    /// The stored bytes.
    pub bytes: Vec<u8>,
    /// The expected checksum (hex).
    pub checksum: String,
}

impl Part {
    /// Build a part and compute its checksum under `kind`.
    #[must_use]
    pub fn new(name: impl Into<String>, bytes: Vec<u8>, kind: ChecksumKind) -> Self {
        let checksum = checksum_of(&bytes, kind);
        Self {
            name: name.into(),
            bytes,
            checksum,
        }
    }
}

/// Compute the checksum of `bytes` under `kind`, as lowercase hex.
#[must_use]
pub fn checksum_of(bytes: &[u8], kind: ChecksumKind) -> String {
    match kind {
        ChecksumKind::Blake3 => blake3::hash(bytes).to_hex().to_string(),
        ChecksumKind::Fnv1a => format!("{:016x}", fnv1a(bytes)),
    }
}

fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// An index split into independently checksummed parts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelfReconstructingIndex {
    parts: BTreeMap<String, Part>,
    kind: ChecksumKind,
    rebuilds: usize,
}

impl SelfReconstructingIndex {
    /// An empty index using `kind`.
    #[must_use]
    pub fn new(kind: ChecksumKind) -> Self {
        Self {
            parts: BTreeMap::new(),
            kind,
            rebuilds: 0,
        }
    }

    /// Insert (or replace) a part, checksumming it.
    pub fn insert(&mut self, name: impl Into<String>, bytes: Vec<u8>) {
        let part = Part::new(name, bytes, self.kind);
        self.parts.insert(part.name.clone(), part);
    }

    /// The number of parts.
    #[must_use]
    pub fn len(&self) -> usize {
        self.parts.len()
    }

    /// Whether the index has no parts.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.parts.is_empty()
    }

    /// The number of rebuilds performed.
    #[must_use]
    pub fn rebuilds(&self) -> usize {
        self.rebuilds
    }

    /// Mutable access to the stored parts (for tests that simulate on-disk
    /// corruption; production code should mutate through [`SelfReconstructingIndex::insert`]).
    pub fn parts_mut(&mut self) -> &mut BTreeMap<String, Part> {
        &mut self.parts
    }

    /// Names of the parts whose bytes no longer match their checksum.
    #[must_use]
    pub fn verify(&self) -> Vec<String> {
        self.parts
            .values()
            .filter(|part| checksum_of(&part.bytes, self.kind) != part.checksum)
            .map(|part| part.name.clone())
            .collect()
    }

    /// Whether every part checks out.
    #[must_use]
    pub fn is_healthy(&self) -> bool {
        self.verify().is_empty()
    }

    /// Rebuild the corrupt parts from `source`, leaving healthy parts alone.
    ///
    /// `source` maps a part name to its authoritative bytes; a corrupt part
    /// with no source entry is dropped (it cannot be reconstructed). Returns
    /// the names rebuilt (or dropped).
    pub fn rebuild_corrupt(&mut self, source: &BTreeMap<String, Vec<u8>>) -> Vec<String> {
        let corrupt = self.verify();
        for name in &corrupt {
            self.rebuilds += 1;
            match source.get(name) {
                Some(bytes) => {
                    self.parts.insert(
                        name.clone(),
                        Part::new(name.clone(), bytes.clone(), self.kind),
                    );
                }
                None => {
                    self.parts.remove(name);
                }
            }
        }
        corrupt
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn healthy_index_verifies_clean() {
        let mut index = SelfReconstructingIndex::new(ChecksumKind::Blake3);
        index.insert("a", b"hello".to_vec());
        index.insert("b", b"world".to_vec());
        assert!(index.is_healthy());
        assert!(index.verify().is_empty(), "healthy index verifies clean");
    }

    #[test]
    fn corruption_is_detected() {
        let mut index = SelfReconstructingIndex::new(ChecksumKind::Blake3);
        index.insert("a", b"hello".to_vec());
        index.insert("b", b"world".to_vec());
        // Flip a byte in part a.
        index.parts.get_mut("a").expect("a").bytes[0] = b'j';
        assert_eq!(index.verify(), vec!["a".to_string()]);
    }

    #[test]
    fn rebuild_restores_corrupt_parts_from_source() {
        let mut index = SelfReconstructingIndex::new(ChecksumKind::Blake3);
        index.insert("a", b"hello".to_vec());
        index.insert("b", b"world".to_vec());
        index.parts.get_mut("b").expect("b").bytes = b"WORLD".to_vec();
        let mut source = BTreeMap::new();
        source.insert("b".to_string(), b"world".to_vec());
        let rebuilt = index.rebuild_corrupt(&source);
        assert_eq!(rebuilt, vec!["b".to_string()]);
        assert!(index.is_healthy(), "the part is restored");
        assert_eq!(index.rebuilds(), 1);
    }

    #[test]
    fn unreconstructable_part_is_dropped() {
        let mut index = SelfReconstructingIndex::new(ChecksumKind::Fnv1a);
        index.insert("a", b"hello".to_vec());
        index.parts.get_mut("a").expect("a").bytes = b"jello".to_vec();
        let dropped = index.rebuild_corrupt(&BTreeMap::new());
        assert_eq!(dropped, vec!["a".to_string()]);
        assert!(index.is_empty());
    }

    #[test]
    fn healthy_parts_are_not_rebuilt() {
        let mut index = SelfReconstructingIndex::new(ChecksumKind::Blake3);
        index.insert("a", b"hello".to_vec());
        index.insert("b", b"world".to_vec());
        index.parts.get_mut("a").expect("a").bytes = b"jello".to_vec();
        let mut source = BTreeMap::new();
        source.insert("a".to_string(), b"hello".to_vec());
        source.insert("b".to_string(), b"CHANGED".to_vec());
        index.rebuild_corrupt(&source);
        assert_eq!(index.parts["b"].bytes, b"world", "healthy b is untouched");
        assert_eq!(index.rebuilds(), 1, "only a was rebuilt");
    }
}
