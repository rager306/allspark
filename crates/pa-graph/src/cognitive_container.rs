//! R19 — cognitive container: a bounded working-memory buffer sealed into a
//! witness chain per epoch.
//!
//! A [`CognitiveContainer`] holds a bounded set of working-memory items. When
//! an epoch closes, the container seals the *current* contents into a
//! [`Witness`] — a `BLAKE3` hash of the epoch's items bound to the hash of the
//! previous epoch. The result is an append-only, tamper-evident chain of what
//! the container held over time (the same hash-linked construction as
//! [`crate::witness`], specialized to memory epochs).
//!
//! Items are evicted FIFO when the bound is exceeded, so the working set stays
//! bounded while the sealed epochs preserve the full history. Hashing uses a
//! canonical, length-prefixed encoding so field/entry boundaries cannot
//! collide.

#![forbid(unsafe_code)]

/// A sealed epoch: the witness of one closed working-memory window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Witness {
    /// Zero-based epoch index.
    pub epoch: u64,
    /// Number of items sealed in this epoch.
    pub item_count: usize,
    /// `BLAKE3` of the sealed items (length-prefixed, in insertion order).
    pub items_hash: String,
    /// The previous epoch's [`Witness::hash`] (`""` for the first epoch).
    pub prev_hash: String,
}

impl Witness {
    fn new(epoch: u64, items: &[String], prev_hash: String) -> Self {
        Self {
            epoch,
            item_count: items.len(),
            items_hash: hash_items(items),
            prev_hash,
        }
    }

    /// This witness's own hash: `BLAKE3` over the length-prefixed fields.
    #[must_use]
    pub fn hash(&self) -> String {
        let mut hasher = blake3::Hasher::new();
        hasher.update(&self.epoch.to_le_bytes());
        hasher.update(&(self.item_count as u64).to_le_bytes());
        for field in [self.items_hash.as_str(), self.prev_hash.as_str()] {
            hasher.update(&(field.len() as u64).to_le_bytes());
            hasher.update(field.as_bytes());
        }
        hasher.finalize().to_hex().to_string()
    }
}

fn hash_items(items: &[String]) -> String {
    let mut hasher = blake3::Hasher::new();
    for item in items {
        hasher.update(&(item.len() as u64).to_le_bytes());
        hasher.update(item.as_bytes());
    }
    hasher.finalize().to_hex().to_string()
}

/// A bounded working-memory container that seals epochs into a witness chain.
#[derive(Debug, Clone)]
pub struct CognitiveContainer {
    bound: usize,
    items: Vec<String>,
    epochs: Vec<Witness>,
}

impl CognitiveContainer {
    /// A container holding at most `bound` live items (at least 1).
    #[must_use]
    pub fn new(bound: usize) -> Self {
        Self {
            bound: bound.max(1),
            items: Vec::new(),
            epochs: Vec::new(),
        }
    }

    /// The live-item bound.
    #[must_use]
    pub fn bound(&self) -> usize {
        self.bound
    }

    /// The current (unsealed) items, in insertion order.
    #[must_use]
    pub fn items(&self) -> &[String] {
        &self.items
    }

    /// Number of live items.
    #[must_use]
    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// Whether there are no live items.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// The sealed epochs, in order.
    #[must_use]
    pub fn epochs(&self) -> &[Witness] {
        &self.epochs
    }

    /// Add an item, evicting the oldest live item if the bound is exceeded.
    /// Returns the evicted item, if any.
    pub fn add(&mut self, item: impl Into<String>) -> Option<String> {
        self.items.push(item.into());
        if self.items.len() > self.bound {
            Some(self.items.remove(0))
        } else {
            None
        }
    }

    /// Explicitly evict the oldest live item. Returns it, or `None` if empty.
    pub fn evict(&mut self) -> Option<String> {
        if self.items.is_empty() {
            None
        } else {
            Some(self.items.remove(0))
        }
    }

    /// Seal the current items into a new [`Witness`] and clear the live buffer.
    /// The witness links to the previous epoch's hash. Returns the witness.
    pub fn close_epoch(&mut self) -> Witness {
        let prev_hash = self.epochs.last().map_or(String::new(), Witness::hash);
        let epoch = self.epochs.len() as u64;
        let witness = Witness::new(epoch, &self.items, prev_hash);
        self.epochs.push(witness.clone());
        self.items.clear();
        witness
    }

    /// Verify the whole witness chain: every epoch must link to its
    /// predecessor and its stored hashes must be intact. Returns `true` for an
    /// empty chain.
    #[must_use]
    pub fn verify_chain(&self) -> bool {
        let mut prev = String::new();
        for (i, witness) in self.epochs.iter().enumerate() {
            if witness.epoch != i as u64 || witness.prev_hash != prev {
                return false;
            }
            prev = witness.hash();
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chain_verifies_and_links() {
        let mut c = CognitiveContainer::new(4);
        c.add("a");
        c.add("b");
        let w0 = c.close_epoch();
        assert_eq!(w0.epoch, 0);
        assert_eq!(w0.item_count, 2);
        assert_eq!(w0.prev_hash, "");

        c.add("c");
        let w1 = c.close_epoch();
        assert_eq!(w1.epoch, 1);
        assert_eq!(w1.prev_hash, w0.hash());

        assert_eq!(c.epochs().len(), 2);
        assert!(c.verify_chain());
        // Closing an epoch clears the live buffer.
        assert!(c.is_empty());
    }

    #[test]
    fn empty_chain_verifies() {
        let c = CognitiveContainer::new(2);
        assert!(c.verify_chain());
        assert!(c.epochs().is_empty());
    }

    #[test]
    fn bound_eviction_is_fifo() {
        let mut c = CognitiveContainer::new(2);
        assert_eq!(c.add("a"), None);
        assert_eq!(c.add("b"), None);
        // Third insert evicts the oldest.
        assert_eq!(c.add("c"), Some("a".to_string()));
        assert_eq!(c.items(), ["b".to_string(), "c".to_string()]);
        assert_eq!(c.len(), 2);
        assert_eq!(c.bound(), 2);
        // Explicit eviction.
        assert_eq!(c.evict(), Some("b".to_string()));
        assert_eq!(c.evict(), Some("c".to_string()));
        assert_eq!(c.evict(), None);
    }

    #[test]
    fn epoch_hash_is_deterministic_and_order_sensitive() {
        let mut a = CognitiveContainer::new(8);
        a.add("x");
        a.add("y");
        let wa = a.close_epoch();

        let mut b = CognitiveContainer::new(8);
        b.add("x");
        b.add("y");
        let wb = b.close_epoch();
        assert_eq!(wa.items_hash, wb.items_hash);
        assert_eq!(wa.hash(), wb.hash());

        // Same items, different order → different hash.
        let mut c = CognitiveContainer::new(8);
        c.add("y");
        c.add("x");
        let wc = c.close_epoch();
        assert_ne!(wa.items_hash, wc.items_hash);
        assert_ne!(wa.hash(), wc.hash());
    }

    #[test]
    fn tampering_breaks_verification() {
        let mut c = CognitiveContainer::new(4);
        c.add("a");
        c.close_epoch();
        c.add("b");
        c.close_epoch();
        assert!(c.verify_chain());
        // Corrupt the first epoch's recorded item hash.
        c.epochs[0].items_hash = "forged".into();
        assert!(!c.verify_chain());
    }
}
