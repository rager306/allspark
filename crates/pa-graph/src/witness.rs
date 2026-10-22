//! M6 — witness receipts: hash-linked proofs of an answer.
//!
//! A [`WitnessReceipt`] is a compact, self-describing proof that a query was
//! answered: it binds the query hash, the result hash, a timestamp, and the
//! hash of the previous receipt. A [`ReceiptChain`] links receipts into a
//! blockchain-lite append-only log: tampering with any receipt breaks every
//! link after it, and [`ReceiptChain::verify_chain`] detects that.
//!
//! Hashing is `BLAKE3` over a canonical, length-prefixed encoding so distinct
//! field boundaries cannot collide.

/// A single witness receipt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WitnessReceipt {
    /// Hash of the query that was answered.
    pub query_hash: String,
    /// Hash of the answer.
    pub result_hash: String,
    /// Logical timestamp (caller-supplied; monotonic in a chain).
    pub timestamp: u64,
    /// Hash of the previous receipt (`""` for the chain head).
    pub chain_prev_hash: String,
}

impl WitnessReceipt {
    /// Build a receipt. `prev_hash` is the previous receipt's own hash, or
    /// `""` for the head of a chain.
    #[must_use]
    pub fn new(
        query_hash: impl Into<String>,
        result_hash: impl Into<String>,
        timestamp: u64,
        prev_hash: impl Into<String>,
    ) -> Self {
        Self {
            query_hash: query_hash.into(),
            result_hash: result_hash.into(),
            timestamp,
            chain_prev_hash: prev_hash.into(),
        }
    }

    /// The receipt's own hash: `BLAKE3` over the length-prefixed fields.
    #[must_use]
    pub fn hash(&self) -> String {
        let mut hasher = blake3::Hasher::new();
        for field in [
            self.query_hash.as_str(),
            self.result_hash.as_str(),
            self.chain_prev_hash.as_str(),
        ] {
            hasher.update(&(field.len() as u64).to_le_bytes());
            hasher.update(field.as_bytes());
        }
        hasher.update(&self.timestamp.to_le_bytes());
        hasher.finalize().to_hex().to_string()
    }

    /// Verify the receipt's structure and its own hash.
    ///
    /// `expected_hash` is the hash the caller recorded for this receipt; a
    /// mismatch means the receipt was tampered with or corrupted.
    #[must_use]
    pub fn verify(&self, expected_hash: &str) -> bool {
        !self.query_hash.is_empty() && !self.result_hash.is_empty() && self.hash() == expected_hash
    }
}

/// An append-only chain of hash-linked receipts.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReceiptChain {
    receipts: Vec<WitnessReceipt>,
}

/// Hash a *sorted* `(path, content_hash)` list into one run identity.
///
/// Sorting first makes the digest independent of walk order, so two index runs
/// over the same tree produce the same value even if the filesystem returned
/// them differently. Length prefixes keep `("a","bc")` distinct from
/// `("ab","c")`.
#[must_use]
pub fn hash_sorted_pairs(pairs: &[(String, String)]) -> String {
    let mut sorted: Vec<&(String, String)> = pairs.iter().collect();
    sorted.sort();
    let mut hasher = blake3::Hasher::new();
    for (path, hash) in sorted {
        hasher.update(&(path.len() as u64).to_be_bytes());
        hasher.update(path.as_bytes());
        hasher.update(&(hash.len() as u64).to_be_bytes());
        hasher.update(hash.as_bytes());
    }
    hasher.finalize().to_hex().to_string()
}

impl ReceiptChain {
    /// An empty chain.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Number of receipts.
    #[must_use]
    pub fn len(&self) -> usize {
        self.receipts.len()
    }

    /// Whether the chain is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.receipts.is_empty()
    }

    /// The stored receipts, in append order.
    #[must_use]
    pub fn receipts(&self) -> &[WitnessReceipt] {
        &self.receipts
    }

    /// Mutable access to the stored receipts (for tests that simulate
    /// tampering; production code should treat receipts as append-only).
    pub fn receipts_mut(&mut self) -> &mut [WitnessReceipt] {
        &mut self.receipts
    }

    /// The head receipt's hash (`""` for an empty chain).
    #[must_use]
    pub fn head_hash(&self) -> String {
        self.receipts
            .last()
            .map_or(String::new(), WitnessReceipt::hash)
    }

    /// Append a receipt for `(query_hash, result_hash)` at `timestamp`,
    /// linking it to the current head. Returns the new receipt's hash.
    pub fn append(
        &mut self,
        query_hash: impl Into<String>,
        result_hash: impl Into<String>,
        timestamp: u64,
    ) -> String {
        let prev = self.head_hash();
        let receipt = WitnessReceipt::new(query_hash, result_hash, timestamp, prev);
        let hash = receipt.hash();
        self.receipts.push(receipt);
        hash
    }

    /// Verify the whole chain: every link must point at the previous
    /// receipt's hash and every receipt must be structurally valid.
    ///
    /// Returns `true` for an empty chain (the empty log is trivially valid).
    #[must_use]
    pub fn verify_chain(&self) -> bool {
        let mut prev = String::new();
        for receipt in &self.receipts {
            if receipt.chain_prev_hash != prev {
                return false;
            }
            if receipt.query_hash.is_empty() || receipt.result_hash.is_empty() {
                return false;
            }
            prev = receipt.hash();
        }
        true
    }

    /// Whether the chain detects a tampered receipt.
    #[must_use]
    pub fn is_tampered(&self) -> bool {
        !self.verify_chain()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn receipt_hash_is_stable_and_binds_fields() {
        let receipt = WitnessReceipt::new("q", "r", 1, "");
        assert_eq!(receipt.hash(), receipt.hash());
        let other = WitnessReceipt::new("q", "r2", 1, "");
        assert_ne!(receipt.hash(), other.hash(), "result hash is bound");
        let later = WitnessReceipt::new("q", "r", 2, "");
        assert_ne!(receipt.hash(), later.hash(), "timestamp is bound");
    }

    #[test]
    fn verify_checks_the_recorded_hash() {
        let receipt = WitnessReceipt::new("q", "r", 1, "");
        let hash = receipt.hash();
        assert!(receipt.verify(&hash));
        assert!(!receipt.verify("deadbeef"));
        assert!(!WitnessReceipt::new("", "r", 1, "").verify("anything"));
    }

    #[test]
    fn chain_links_and_verifies() {
        let mut chain = ReceiptChain::new();
        let h1 = chain.append("q1", "r1", 1);
        let h2 = chain.append("q2", "r2", 2);
        assert_eq!(chain.len(), 2);
        assert_eq!(chain.head_hash(), h2);
        assert_eq!(chain.receipts()[1].chain_prev_hash, h1);
        assert!(chain.verify_chain());
    }

    #[test]
    fn empty_chain_verifies() {
        let chain = ReceiptChain::new();
        assert!(chain.verify_chain());
        assert_eq!(chain.head_hash(), "");
    }

    #[test]
    fn tampering_breaks_verification() {
        let mut chain = ReceiptChain::new();
        chain.append("q1", "r1", 1);
        chain.append("q2", "r2", 2);
        assert!(chain.verify_chain());
        // Forge the first receipt's result.
        chain.receipts[0].result_hash = "forged".into();
        assert!(!chain.verify_chain(), "a forged receipt breaks the chain");
        assert!(chain.is_tampered());
    }
}
