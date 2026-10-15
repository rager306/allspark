//! R2 — capability-gated search.
//!
//! A search over a code graph must not leak nodes the caller has no right to
//! see. [`CapGatedIndex`] attaches a [`CapMask`] (a `u64` bitmask of granted
//! capabilities) to every indexed node; a query supplies the capabilities it
//! *requires* and only nodes whose mask is a superset of the requirement
//! (`node_caps & required == required`) are returned.
//!
//! The check is a pure bit test: no allocation, no hashing, deterministic
//! insertion-order output. [`CapMask::ADMIN`] holds every bit, so an admin
//! token satisfies any requirement.

#![forbid(unsafe_code)]

use std::ops::BitOr;

/// Identifier of an indexed node.
pub type NodeId = u64;

/// A `u64` bitmask of granted capabilities.
///
/// Bits are independent; a node may hold any combination. Use the associated
/// constants ([`CapMask::READ`], [`CapMask::WRITE`], …) and combine them with
/// `|` or [`CapMask::union`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct CapMask(pub u64);

impl CapMask {
    /// No capabilities at all.
    pub const NONE: Self = Self(0);
    /// Read access to a node.
    pub const READ: Self = Self(1 << 0);
    /// Write/mutate access.
    pub const WRITE: Self = Self(1 << 1);
    /// Execute (invoke / call) access.
    pub const EXEC: Self = Self(1 << 2);
    /// Network egress access.
    pub const NET: Self = Self(1 << 3);
    /// Access to memory-resident state.
    pub const MEMORY: Self = Self(1 << 4);
    /// Access to secret material.
    pub const SECRET: Self = Self(1 << 5);
    /// Administrative access; holds every bit, so it satisfies any
    /// requirement.
    pub const ADMIN: Self = Self(u64::MAX);
    /// Every capability (same value as [`CapMask::ADMIN`]).
    pub const ALL: Self = Self(u64::MAX);

    /// Wrap a raw bitmask.
    #[must_use]
    pub const fn new(bits: u64) -> Self {
        Self(bits)
    }

    /// The raw bits.
    #[must_use]
    pub const fn bits(self) -> u64 {
        self.0
    }

    /// The union of two masks.
    #[must_use]
    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    /// Whether the two masks share at least one bit.
    #[must_use]
    pub const fn intersects(self, other: Self) -> bool {
        self.0 & other.0 != 0
    }

    /// Whether this mask grants *every* bit in `required`
    /// (`self & required == required`).
    #[must_use]
    pub const fn satisfies(self, required: Self) -> bool {
        self.0 & required.0 == required.0
    }

    /// Whether `bit` is granted (alias for [`CapMask::satisfies`] on a
    /// single-bit mask).
    #[must_use]
    pub const fn contains(self, bit: Self) -> bool {
        self.satisfies(bit)
    }
}

impl BitOr for CapMask {
    type Output = Self;

    fn bitor(self, rhs: Self) -> Self::Output {
        self.union(rhs)
    }
}

/// One indexed node with its capability mask.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct GatedNode {
    id: NodeId,
    caps: CapMask,
}

/// A capability-gated node index.
#[derive(Debug, Clone, Default)]
pub struct CapGatedIndex {
    nodes: Vec<GatedNode>,
}

impl CapGatedIndex {
    /// An empty index.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Number of indexed nodes.
    #[must_use]
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    /// Whether the index is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// Insert a node with `caps`. Re-inserting an existing id overwrites its
    /// mask in place (last write wins) so insertion order is stable.
    pub fn insert(&mut self, node: NodeId, caps: CapMask) {
        if let Some(existing) = self.nodes.iter_mut().find(|n| n.id == node) {
            existing.caps = caps;
        } else {
            self.nodes.push(GatedNode { id: node, caps });
        }
    }

    /// The mask stored for `node`, if present.
    #[must_use]
    pub fn caps_of(&self, node: NodeId) -> Option<CapMask> {
        self.nodes
            .iter()
            .find(|n| n.id == node)
            .map(|n| n.caps)
    }

    /// Search the index: return the ids for which `query` holds *and* whose
    /// mask satisfies `required`, in insertion order.
    #[must_use]
    pub fn search<F>(&self, query: F, required: CapMask) -> Vec<NodeId>
    where
        F: Fn(NodeId) -> bool,
    {
        self.nodes
            .iter()
            .filter(|n| n.caps.satisfies(required) && query(n.id))
            .map(|n| n.id)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mask_union_and_satisfies() {
        let rw = CapMask::READ | CapMask::WRITE;
        assert!(rw.satisfies(CapMask::READ));
        assert!(rw.satisfies(CapMask::READ | CapMask::WRITE));
        assert!(!rw.satisfies(CapMask::EXEC));
        assert!(!rw.satisfies(CapMask::READ | CapMask::EXEC));
        assert_eq!(rw.bits(), 0b11);
        assert!(rw.intersects(CapMask::WRITE | CapMask::EXEC));
        assert!(!rw.intersects(CapMask::NET));
    }

    #[test]
    fn nodes_without_required_bit_are_excluded() {
        let mut index = CapGatedIndex::new();
        index.insert(1, CapMask::READ);
        index.insert(2, CapMask::READ | CapMask::WRITE);
        index.insert(3, CapMask::ADMIN);

        assert_eq!(index.search(|_| true, CapMask::READ), vec![1, 2, 3]);
        assert_eq!(index.search(|_| true, CapMask::WRITE), vec![2, 3]);
        assert_eq!(index.search(|_| true, CapMask::EXEC), vec![3]);
        // The query predicate still filters.
        assert_eq!(index.search(|id| id != 2, CapMask::READ), vec![1, 3]);
        assert_eq!(index.search(|_| true, CapMask::NONE).len(), 3);
    }

    #[test]
    fn admin_satisfies_everything() {
        let mut index = CapGatedIndex::new();
        index.insert(7, CapMask::ADMIN);
        for required in [
            CapMask::READ,
            CapMask::WRITE,
            CapMask::EXEC,
            CapMask::SECRET,
            CapMask::READ | CapMask::WRITE | CapMask::EXEC,
        ] {
            assert_eq!(index.search(|_| true, required), vec![7]);
        }
        assert_eq!(index.len(), 1);
    }

    #[test]
    fn reinsert_overwrites_in_place() {
        let mut index = CapGatedIndex::new();
        index.insert(1, CapMask::READ);
        index.insert(2, CapMask::READ);
        index.insert(1, CapMask::NONE);
        assert_eq!(index.len(), 2);
        assert_eq!(index.search(|_| true, CapMask::READ), vec![2]);
        assert_eq!(index.caps_of(1), Some(CapMask::NONE));
        assert_eq!(index.caps_of(99), None);
    }
}
