//! G16 — module decompilation: symbols → communities → roles → witness tree.
//!
//! [`decompile`] runs a four-stage pipeline over a symbol set and its call
//! graph:
//!
//! 1. **parse-symbols** — take the `(node, kind)` list as given.
//! 2. **ref-graph** — read the call edges (`caller → callee`).
//! 3. **partition** — reuse [`crate::mincut::label_propagation`] to split the
//!    call graph into communities.
//! 4. **infer** — assign each node a [`Role`] from its in/out call degree
//!    *within its own community*: a node with ≥ 2 incoming calls is a
//!    [`Role::Facade`] (many callers funnel through it), a node with calls only
//!    going *out* is an [`Role::Entry`], everything else is [`Role::Internal`].
//!
//! Each community also gets a **witness tree** — a hash-linked certification
//! root over its members, so a decomposition can be re-checked by recomputing
//! the hashes. The hash of a parent folds in its children, so any change to a
//! role assignment propagates to the community root.

#![forbid(unsafe_code)]

use std::collections::{BTreeMap, BTreeSet};

use crate::mincut::{community_map, CommunityId};
use crate::sparsifier::WeightedEdge;

/// A node id in the decompiled graph.
pub type NodeId = i64;

/// The inferred role of a symbol inside its community.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// ≥ 2 incoming calls from within the community.
    Facade,
    /// Calls go only outward (no incoming calls, at least one outgoing).
    Entry,
    /// Anything else.
    Internal,
}

impl Role {
    /// A stable lowercase name, used in the witness hash.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Role::Facade => "facade",
            Role::Entry => "entry",
            Role::Internal => "internal",
        }
    }
}

/// A hash-linked certification node.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WitnessNode {
    /// Human-readable label (e.g. `"community 0"` or `"node 7"`).
    pub label: String,
    /// `BLAKE3` over the label and the children's hashes, length-prefixed.
    pub hash: String,
    /// Child nodes, sorted by label.
    pub children: Vec<WitnessNode>,
}

/// One inferred community.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Community {
    /// Community id (from label propagation).
    pub id: CommunityId,
    /// Member node ids, ascending.
    pub members: Vec<NodeId>,
    /// Inferred role per member.
    pub kind_assignments: BTreeMap<NodeId, Role>,
    /// The certification tree: root = the community, children = its nodes.
    pub witness: WitnessNode,
}

/// The full decomposition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decomposition {
    /// Communities, ordered by id.
    pub communities: Vec<Community>,
}

impl WitnessNode {
    /// Build a witness node: hash `label` together with each child's hash, in
    /// child order. Length-prefixing keeps distinct boundaries from colliding.
    #[must_use]
    pub fn build(label: impl Into<String>, mut children: Vec<WitnessNode>) -> Self {
        children.sort_by(|a, b| a.label.cmp(&b.label));
        let label = label.into();
        let mut hasher = blake3::Hasher::new();
        let bytes = label.as_bytes();
        hasher.update(&(bytes.len() as u64).to_le_bytes());
        hasher.update(bytes);
        for child in &children {
            let hb = child.hash.as_bytes();
            hasher.update(&(hb.len() as u64).to_le_bytes());
            hasher.update(hb);
        }
        Self {
            label,
            hash: hasher.finalize().to_hex().to_string(),
            children,
        }
    }

    /// Recompute this node's hash from its label and current children and
    /// compare it against the stored `hash`. Returns `true` iff they match.
    #[must_use]
    pub fn verify(&self) -> bool {
        let mut hasher = blake3::Hasher::new();
        let bytes = self.label.as_bytes();
        hasher.update(&(bytes.len() as u64).to_le_bytes());
        hasher.update(bytes);
        for child in &self.children {
            let hb = child.hash.as_bytes();
            hasher.update(&(hb.len() as u64).to_le_bytes());
            hasher.update(hb);
        }
        hasher.finalize().to_hex().to_string() == self.hash
    }
}

/// Decompile `symbols` (a list of `(node, kind)` pairs) over `call_edges`.
///
/// `call_edges` are directed as `a = caller`, `b = callee`; the partition step
/// treats them as undirected, which is what community detection wants.
#[must_use]
pub fn decompile(symbols: &[(NodeId, String)], call_edges: &[WeightedEdge]) -> Decomposition {
    let mut node_set: BTreeSet<NodeId> = symbols.iter().map(|(n, _)| *n).collect();
    for e in call_edges {
        node_set.insert(e.a);
        node_set.insert(e.b);
    }
    let nodes: Vec<NodeId> = node_set.into_iter().collect();

    let communities = community_map(call_edges, 20);

    // Group nodes by community; uncovered nodes fall back to community 0 only
    // if the graph has no edges — otherwise they get their own community id.
    let mut next_fallback = communities.values().copied().max().map_or(0, |m| m + 1);
    let mut by_community: BTreeMap<CommunityId, Vec<NodeId>> = BTreeMap::new();
    for &n in &nodes {
        let c = communities.get(&n).copied().unwrap_or_else(|| {
            let c = next_fallback;
            next_fallback += 1;
            c
        });
        by_community.entry(c).or_default().push(n);
    }

    let mut out: Vec<Community> = Vec::new();
    for (id, members) in by_community {
        // In/out call degree within this community.
        let member_set: BTreeSet<NodeId> = members.iter().copied().collect();
        let mut incoming: BTreeMap<NodeId, usize> = BTreeMap::new();
        let mut outgoing: BTreeMap<NodeId, usize> = BTreeMap::new();
        for n in &members {
            incoming.insert(*n, 0);
            outgoing.insert(*n, 0);
        }
        for e in call_edges {
            if member_set.contains(&e.a) && member_set.contains(&e.b) {
                *outgoing.entry(e.a).or_insert(0) += 1;
                *incoming.entry(e.b).or_insert(0) += 1;
            }
        }

        let mut kind_assignments: BTreeMap<NodeId, Role> = BTreeMap::new();
        let mut leaves: Vec<WitnessNode> = Vec::new();
        for &n in &members {
            let inc = incoming.get(&n).copied().unwrap_or(0);
            let outdeg = outgoing.get(&n).copied().unwrap_or(0);
            let role = if inc >= 2 {
                Role::Facade
            } else if inc == 0 && outdeg > 0 {
                Role::Entry
            } else {
                Role::Internal
            };
            kind_assignments.insert(n, role);
            leaves.push(WitnessNode::build(
                format!("node {n}:{}", role.as_str()),
                Vec::new(),
            ));
        }

        let witness = WitnessNode::build(format!("community {id}"), leaves);
        out.push(Community {
            id,
            members,
            kind_assignments,
            witness,
        });
    }

    Decomposition { communities: out }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A synthetic module: node 0 is an entry (calls out only), node 1 is a
    /// facade (called by 0 and 2), node 2 is internal (calls the facade).
    fn synthetic_module() -> (Vec<(NodeId, String)>, Vec<WeightedEdge>) {
        let symbols = vec![
            (0, "fn".to_string()),
            (1, "struct".to_string()),
            (2, "fn".to_string()),
            (3, "fn".to_string()),
        ];
        // Calls: 0→1, 2→1 (facade 1 has 2 incoming), 0→2, and 3→0 tying 3 in.
        let edges = vec![
            WeightedEdge::new(0, 1, 1.0),
            WeightedEdge::new(2, 1, 1.0),
            WeightedEdge::new(0, 2, 1.0),
            WeightedEdge::new(3, 0, 1.0),
        ];
        (symbols, edges)
    }

    #[test]
    fn roles_are_inferred_from_call_degree() {
        let (symbols, edges) = synthetic_module();
        let d = decompile(&symbols, &edges);
        assert_eq!(d.communities.len(), 1, "the whole module is one community");
        let c = &d.communities[0];
        assert_eq!(c.kind_assignments[&1], Role::Facade, "1 has two callers");
        assert_eq!(
            c.kind_assignments[&0],
            Role::Internal,
            "0 has one caller (3)"
        );
        assert_eq!(c.kind_assignments[&2], Role::Internal);
        assert_eq!(c.kind_assignments[&3], Role::Entry, "3 only calls out");
    }

    #[test]
    fn witness_hash_is_reproducible() {
        let (symbols, edges) = synthetic_module();
        let a = decompile(&symbols, &edges);
        let b = decompile(&symbols, &edges);
        assert_eq!(a, b);
        assert!(a.communities[0].witness.verify());
        assert_eq!(a.communities[0].witness.children.len(), 4);
    }

    #[test]
    fn witness_hash_changes_when_a_role_changes() {
        let (symbols, edges) = synthetic_module();
        let d = decompile(&symbols, &edges);
        let root = &d.communities[0].witness;
        // Tamper: rebuild the root with one child's role flipped.
        let mut leaves = root.children.clone();
        leaves[0] = WitnessNode::build("node 0:facade".to_string(), Vec::new());
        let tampered = WitnessNode::build("community 0".to_string(), leaves);
        assert_ne!(tampered.hash, root.hash);
        assert!(
            tampered.verify(),
            "the tampered tree is still self-consistent"
        );
    }

    #[test]
    fn disconnected_modules_split_into_communities() {
        let symbols = vec![(0, "fn".into()), (1, "fn".into()), (2, "fn".into())];
        let edges = vec![WeightedEdge::new(0, 1, 1.0)]; // 2 is isolated
        let d = decompile(&symbols, &edges);
        let all: Vec<NodeId> = d
            .communities
            .iter()
            .flat_map(|c| c.members.clone())
            .collect();
        assert_eq!(all.len(), 3);
        assert!(d.communities.iter().all(|c| c.witness.verify()));
    }
}
