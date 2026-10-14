//! Architecture views over the code graph (G6).
//!
//! [`architecture`] renders one of eight structural views of a project. The
//! graph algorithms ride on `petgraph`: strongly-connected components (cycles),
//! condensation + longest-path layering (layers), and connected components
//! (clusters). The dependency graph these run on is *file-level*: symbol
//! `CALLS`/`IMPORTS`/`USAGE` edges are folded up to the files that own their
//! endpoints, so the topology reflects modules, not individual functions.
//!
//! The views are:
//!
//! - [`ArchitectureView::Overview`] — node/edge/language/file counts.
//! - [`ArchitectureView::Structure`] — file-level dependency edges with weights.
//! - [`ArchitectureView::Layers`] — files bucketed by longest-path depth.
//! - [`ArchitectureView::Clusters`] — weakly-connected file communities.
//! - [`ArchitectureView::Cycles`] — dependency cycles (SCCs of size > 1).
//! - [`ArchitectureView::Hotspots`] — highest-degree symbols.
//! - [`ArchitectureView::EntryPoints`] — call roots (in-degree 0, out-degree > 0).
//! - [`ArchitectureView::FileTree`] — the directory tree with per-node counts.

use std::collections::{BTreeMap, HashMap};

use petgraph::algo::connected_components;
use petgraph::graph::{DiGraph, NodeIndex};
use petgraph::visit::EdgeRef;
use petgraph::Direction;
use serde::{Deserialize, Serialize};

use crate::error::Result;
use crate::store::GraphStore;
use crate::types::{EdgeType, Label, Node};

/// Which architecture view to render.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ArchitectureView {
    /// Counts by node label, edge type, language, and file.
    Overview,
    /// File-level dependency edges with call/import weights.
    Structure,
    /// Files bucketed by longest-path dependency depth.
    Layers,
    /// Weakly-connected file communities.
    Clusters,
    /// Dependency cycles (strongly-connected components).
    Cycles,
    /// Highest-degree symbols.
    Hotspots,
    /// Call roots: symbols nothing calls that themselves call out.
    EntryPoints,
    /// The directory tree with symbol/file counts.
    FileTree,
}

impl ArchitectureView {
    /// Parse a view from its wire name; unknown names default to `Overview`.
    #[must_use]
    pub fn from_str_or_default(value: &str) -> Self {
        match value {
            "structure" => Self::Structure,
            "layers" => Self::Layers,
            "clusters" => Self::Clusters,
            "cycles" => Self::Cycles,
            "hotspots" => Self::Hotspots,
            "entry_points" | "entryPoints" => Self::EntryPoints,
            "file_tree" | "fileTree" => Self::FileTree,
            _ => Self::Overview,
        }
    }

    /// Stable wire name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Overview => "overview",
            Self::Structure => "structure",
            Self::Layers => "layers",
            Self::Clusters => "clusters",
            Self::Cycles => "cycles",
            Self::Hotspots => "hotspots",
            Self::EntryPoints => "entry_points",
            Self::FileTree => "file_tree",
        }
    }
}

/// A node/edge/language/file census.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Overview {
    /// Total nodes.
    pub nodes: usize,
    /// Total edges.
    pub edges: usize,
    /// Files with a stored hash.
    pub files: usize,
    /// Node counts by label wire name.
    pub labels: BTreeMap<String, usize>,
    /// Edge counts by edge-type wire name.
    pub edge_types: BTreeMap<String, usize>,
    /// File counts by language (from the path extension).
    pub languages: BTreeMap<String, usize>,
}

/// A file-level dependency edge.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModuleEdge {
    /// Depending file.
    pub from: String,
    /// Depended-on file.
    pub to: String,
    /// Number of folded symbol edges.
    pub weight: usize,
    /// Distinct edge types that produced the fold.
    pub kinds: Vec<String>,
}

/// One longest-path dependency layer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Layer {
    /// Zero-based layer index (0 = no dependencies).
    pub level: usize,
    /// Files at this depth, sorted.
    pub files: Vec<String>,
}

/// A weakly-connected community of files.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Cluster {
    /// Stable cluster id (0-based, by first-seen order).
    pub id: usize,
    /// Member files, sorted.
    pub files: Vec<String>,
    /// Total folded edges with both endpoints in this cluster.
    pub internal_edges: usize,
}

/// A dependency cycle (a strongly-connected component with a real cycle).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Cycle {
    /// Files in the cycle, sorted.
    pub files: Vec<String>,
    /// The internal edges that close the cycle.
    pub edges: Vec<ModuleEdge>,
}

/// A high-degree symbol.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hotspot {
    /// Qualified name.
    pub qualified_name: String,
    /// Short name.
    pub name: String,
    /// Defining file.
    pub file_path: String,
    /// Incoming edges (callers/importers).
    pub fan_in: usize,
    /// Outgoing edges (callees/imports).
    pub fan_out: usize,
    /// `fan_in + fan_out`.
    pub degree: usize,
}

/// A call root: a symbol with no callers that calls out.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EntryPoint {
    /// Qualified name.
    pub qualified_name: String,
    /// Short name.
    pub name: String,
    /// Defining file.
    pub file_path: String,
    /// Outgoing calls.
    pub out_degree: usize,
}

/// One directory in the file tree.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TreeNode {
    /// Path segment name.
    pub name: String,
    /// Files directly in this directory.
    pub files: Vec<String>,
    /// Symbols defined anywhere under this directory.
    pub symbol_count: usize,
    /// Child directories, sorted by name.
    pub children: Vec<TreeNode>,
}

/// The full architecture report. Only the sections for the requested `view`
/// are populated; the rest stay at their defaults.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Architecture {
    /// Project name.
    pub project: String,
    /// The rendered view.
    pub view: ArchitectureView,
    /// Populated for [`ArchitectureView::Overview`].
    pub overview: Option<Overview>,
    /// Populated for [`ArchitectureView::Structure`].
    pub structure: Vec<ModuleEdge>,
    /// Populated for [`ArchitectureView::Layers`].
    pub layers: Vec<Layer>,
    /// Populated for [`ArchitectureView::Clusters`].
    pub clusters: Vec<Cluster>,
    /// Populated for [`ArchitectureView::Cycles`].
    pub cycles: Vec<Cycle>,
    /// Populated for [`ArchitectureView::Hotspots`].
    pub hotspots: Vec<Hotspot>,
    /// Populated for [`ArchitectureView::EntryPoints`].
    pub entry_points: Vec<EntryPoint>,
    /// Populated for [`ArchitectureView::FileTree`].
    pub file_tree: Option<TreeNode>,
}

/// Build the requested architecture `view` for `project`.
///
/// # Errors
/// Propagates SQLite errors.
pub fn architecture(
    store: &GraphStore,
    project: &str,
    view: ArchitectureView,
) -> Result<Architecture> {
    let nodes = store.all_nodes(project)?;
    let edges = store.all_edges(project)?;
    let mut arch = Architecture {
        project: project.to_string(),
        view,
        overview: None,
        structure: Vec::new(),
        layers: Vec::new(),
        clusters: Vec::new(),
        cycles: Vec::new(),
        hotspots: Vec::new(),
        entry_points: Vec::new(),
        file_tree: None,
    };

    // Fold symbol edges up to files for the graph views.
    let node_file: HashMap<i64, String> =
        nodes.iter().map(|n| (n.id, n.file_path.clone())).collect();
    let module_edges = fold_to_files(&edges, &node_file);

    match view {
        ArchitectureView::Overview => {
            arch.overview = Some(overview(store, project, &nodes, &edges)?);
        }
        ArchitectureView::Structure => {
            arch.structure = module_edges;
        }
        ArchitectureView::Layers => {
            arch.layers = layers(&module_edges);
        }
        ArchitectureView::Clusters => {
            arch.clusters = clusters(&module_edges);
        }
        ArchitectureView::Cycles => {
            arch.cycles = cycles(&module_edges);
        }
        ArchitectureView::Hotspots => {
            arch.hotspots = hotspots(&nodes, &edges);
        }
        ArchitectureView::EntryPoints => {
            arch.entry_points = entry_points(&nodes, &edges);
        }
        ArchitectureView::FileTree => {
            arch.file_tree = Some(file_tree(&nodes));
        }
    }
    Ok(arch)
}

impl GraphStore {
    /// See [`architecture`].
    ///
    /// # Errors
    /// Propagates SQLite errors.
    pub fn architecture(&self, project: &str, view: ArchitectureView) -> Result<Architecture> {
        architecture(self, project, view)
    }
}

/// Fold symbol-level edges to file-level edges, dropping same-file and
/// non-file endpoints (e.g. `CONTAINS` from a file to its own symbols).
fn fold_to_files(
    edges: &[crate::types::Edge],
    node_file: &HashMap<i64, String>,
) -> Vec<ModuleEdge> {
    // Only dependency-shaped edges carry architecture signal; CONTAINS is
    // intra-file by construction and HAS_BRANCH/FILE_CHANGES_WITH are git
    // provenance, not code structure.
    let mut acc: BTreeMap<(String, String), (usize, std::collections::BTreeSet<String>)> =
        BTreeMap::new();
    for e in edges {
        if !matches!(
            e.edge_type,
            EdgeType::Calls | EdgeType::Imports | EdgeType::Uses | EdgeType::Inherits
        ) {
            continue;
        }
        let (Some(from), Some(to)) = (node_file.get(&e.source), node_file.get(&e.target)) else {
            continue;
        };
        if from.is_empty() || to.is_empty() || from == to {
            continue;
        }
        let entry = acc
            .entry((from.clone(), to.clone()))
            .or_insert((0, { std::collections::BTreeSet::new() }));
        entry.0 += 1;
        entry.1.insert(e.edge_type.as_str().to_string());
    }
    acc.into_iter()
        .map(|((from, to), (weight, kinds))| ModuleEdge {
            from,
            to,
            weight,
            kinds: kinds.into_iter().collect(),
        })
        .collect()
}

/// Build a directed file graph from folded module edges.
fn file_graph(edges: &[ModuleEdge]) -> (DiGraph<String, usize>, HashMap<String, NodeIndex>) {
    let mut graph: DiGraph<String, usize> = DiGraph::new();
    let mut index: HashMap<String, NodeIndex> = HashMap::new();
    for e in edges {
        let from = *index
            .entry(e.from.clone())
            .or_insert_with(|| graph.add_node(e.from.clone()));
        let to = *index
            .entry(e.to.clone())
            .or_insert_with(|| graph.add_node(e.to.clone()));
        graph.add_edge(from, to, e.weight);
    }
    (graph, index)
}

fn overview(
    store: &GraphStore,
    project: &str,
    nodes: &[Node],
    edges: &[crate::types::Edge],
) -> Result<Overview> {
    let mut labels: BTreeMap<String, usize> = BTreeMap::new();
    for n in nodes {
        *labels.entry(n.label.as_str().to_string()).or_insert(0) += 1;
    }
    let mut edge_types: BTreeMap<String, usize> = BTreeMap::new();
    for e in edges {
        *edge_types
            .entry(e.edge_type.as_str().to_string())
            .or_insert(0) += 1;
    }
    let mut languages: BTreeMap<String, usize> = BTreeMap::new();
    for path in store.all_file_hashes(project)?.keys() {
        let lang = std::path::Path::new(path)
            .extension()
            .and_then(|e| e.to_str())
            .and_then(pa_ast::SupportLang::from_alias)
            .map_or("other", pa_ast::SupportLang::canonical_name)
            .to_string();
        *languages.entry(lang).or_insert(0) += 1;
    }
    Ok(Overview {
        nodes: nodes.len(),
        edges: edges.len(),
        files: store.file_count(project)?,
        labels,
        edge_types,
        languages,
    })
}

/// Longest-path layering over the condensed dependency DAG.
///
/// A file's level is one past the deepest layer among the files it *depends
/// on* (transitively, through the condensation): a leaf dependency such as a
/// database module is level 0, and an application entry file that reaches it
/// sits at the top. Cycles collapse to one condensed node, so every member of
/// a cycle shares a layer.
fn layers(edges: &[ModuleEdge]) -> Vec<Layer> {
    let (graph, _) = file_graph(edges);
    if graph.node_count() == 0 {
        return Vec::new();
    }
    // Condense to a DAG; each condensed node carries its member file names.
    let condensed = petgraph::algo::condensation(graph, true);
    // `toposort` orders each node before its successors, so iterating in
    // reverse visits sinks (no outgoing dependencies) first: their level is 0
    // and every predecessor is one past its deepest successor.
    let order = petgraph::algo::toposort(&condensed, None).unwrap_or_default();
    let mut level: HashMap<NodeIndex, usize> = HashMap::new();
    for node in order.into_iter().rev() {
        let depth = condensed
            .edges_directed(node, Direction::Outgoing)
            .map(|e| level.get(&e.target()).copied().unwrap_or(0) + 1)
            .max()
            .unwrap_or(0);
        level.insert(node, depth);
    }
    let mut buckets: BTreeMap<usize, Vec<String>> = BTreeMap::new();
    for (idx, members) in condensed.node_indices().zip(condensed.node_weights()) {
        let depth = level.get(&idx).copied().unwrap_or(0);
        buckets
            .entry(depth)
            .or_default()
            .extend(members.iter().cloned());
    }
    buckets
        .into_iter()
        .map(|(level, mut files)| {
            files.sort();
            files.dedup();
            Layer { level, files }
        })
        .collect()
}

/// Weakly-connected file communities, sized descending.
///
/// A `Cluster` is a connected component of the *undirected* projection of the
/// file dependency graph: two files that depend on each other in either
/// direction land in the same community.
fn clusters(edges: &[ModuleEdge]) -> Vec<Cluster> {
    let (graph, _) = file_graph(edges);
    if graph.node_count() == 0 {
        return Vec::new();
    }
    // Undirected projection: node `i` maps to the same integer index so the
    // component id can be read straight back onto `graph`.
    let mut ug = petgraph::graph::UnGraph::<usize, ()>::new_undirected();
    let mut idx_map: HashMap<NodeIndex, petgraph::graph::NodeIndex> = HashMap::new();
    for n in graph.node_indices() {
        let mapped = ug.add_node(n.index());
        idx_map.insert(n, mapped);
    }
    for e in graph.edge_indices() {
        if let Some((a, b)) = graph.edge_endpoints(e) {
            ug.add_edge(idx_map[&a], idx_map[&b], ());
        }
    }
    // `connected_components` counts them; a BFS labels each node with its id so
    // the members can be grouped.
    let _ = connected_components(&ug);
    let mut comp_of: HashMap<usize, usize> = HashMap::new();
    let mut comp_id = 0usize;
    for start in ug.node_indices() {
        if comp_of.contains_key(&ug[start]) {
            continue;
        }
        let mut stack = vec![start];
        comp_of.insert(ug[start], comp_id);
        while let Some(node) = stack.pop() {
            for nb in ug.neighbors(node) {
                if let std::collections::hash_map::Entry::Vacant(e) = comp_of.entry(ug[nb]) {
                    e.insert(comp_id);
                    stack.push(nb);
                }
            }
        }
        comp_id += 1;
    }
    let mut members: BTreeMap<usize, Vec<String>> = BTreeMap::new();
    for node in graph.node_indices() {
        let cid = comp_of[&node.index()];
        members.entry(cid).or_default().push(graph[node].clone());
    }
    let mut out: Vec<Cluster> = members
        .into_values()
        .map(|mut files| {
            files.sort();
            Cluster {
                id: 0,
                files,
                internal_edges: 0,
            }
        })
        .collect();
    out.sort_by(|a, b| {
        b.files
            .len()
            .cmp(&a.files.len())
            .then(a.files.cmp(&b.files))
    });
    for (i, c) in out.iter_mut().enumerate() {
        c.id = i;
        let set: std::collections::HashSet<&String> = c.files.iter().collect();
        c.internal_edges = edges
            .iter()
            .filter(|e| set.contains(&e.from) && set.contains(&e.to))
            .count();
    }
    out
}

/// Dependency cycles: strongly-connected components of size > 1, plus
/// self-loops.
fn cycles(edges: &[ModuleEdge]) -> Vec<Cycle> {
    let (graph, _) = file_graph(edges);
    if graph.node_count() == 0 {
        return Vec::new();
    }
    let mut out: Vec<Cycle> = Vec::new();
    for component in petgraph::algo::tarjan_scc(&graph) {
        let is_self_loop = component.len() == 1 && {
            let n = component[0];
            graph.edges(n).any(|e| e.target() == n)
        };
        if component.len() < 2 && !is_self_loop {
            continue;
        }
        let mut files: Vec<String> = component.iter().map(|n| graph[*n].clone()).collect();
        files.sort();
        let set: std::collections::HashSet<&String> = files.iter().collect();
        let mut internal: Vec<ModuleEdge> = edges
            .iter()
            .filter(|e| set.contains(&e.from) && set.contains(&e.to))
            .cloned()
            .collect();
        internal.sort_by(|a, b| a.from.cmp(&b.from).then(a.to.cmp(&b.to)));
        out.push(Cycle {
            files,
            edges: internal,
        });
    }
    out.sort_by(|a, b| {
        b.files
            .len()
            .cmp(&a.files.len())
            .then(a.files.cmp(&b.files))
    });
    out
}

/// Highest-degree symbols, most connected first.
fn hotspots(nodes: &[Node], edges: &[crate::types::Edge]) -> Vec<Hotspot> {
    let mut fan_in: HashMap<i64, usize> = HashMap::new();
    let mut fan_out: HashMap<i64, usize> = HashMap::new();
    for e in edges {
        *fan_out.entry(e.source).or_insert(0) += 1;
        *fan_in.entry(e.target).or_insert(0) += 1;
    }
    let mut out: Vec<Hotspot> = nodes
        .iter()
        // Structural containers (files) and git provenance (branches) are not
        // code symbols; their degree comes from CONTAINS/HAS_BRANCH bookkeeping.
        .filter(|n| !matches!(n.label, Label::File | Label::Branch))
        .map(|n| {
            let fi = fan_in.get(&n.id).copied().unwrap_or(0);
            let fo = fan_out.get(&n.id).copied().unwrap_or(0);
            Hotspot {
                qualified_name: n.qualified_name.clone(),
                name: n.name.clone(),
                file_path: n.file_path.clone(),
                fan_in: fi,
                fan_out: fo,
                degree: fi + fo,
            }
        })
        .filter(|h| h.degree > 0)
        .collect();
    out.sort_by(|a, b| {
        b.degree
            .cmp(&a.degree)
            .then(a.qualified_name.cmp(&b.qualified_name))
    });
    out
}

/// Call roots: symbols with no incoming `CALLS` edges that do call out.
fn entry_points(nodes: &[Node], edges: &[crate::types::Edge]) -> Vec<EntryPoint> {
    let mut called: std::collections::HashSet<i64> = std::collections::HashSet::new();
    let mut calls_out: HashMap<i64, usize> = HashMap::new();
    for e in edges {
        if e.edge_type == EdgeType::Calls {
            called.insert(e.target);
            *calls_out.entry(e.source).or_insert(0) += 1;
        }
    }
    let mut out: Vec<EntryPoint> = nodes
        .iter()
        .filter(|n| !matches!(n.label, Label::File | Label::Branch))
        .filter(|n| !called.contains(&n.id))
        .filter_map(|n| {
            let out_degree = calls_out.get(&n.id).copied().unwrap_or(0);
            (out_degree > 0).then(|| EntryPoint {
                qualified_name: n.qualified_name.clone(),
                name: n.name.clone(),
                file_path: n.file_path.clone(),
                out_degree,
            })
        })
        .collect();
    out.sort_by(|a, b| {
        b.out_degree
            .cmp(&a.out_degree)
            .then(a.qualified_name.cmp(&b.qualified_name))
    });
    out
}

/// Build a directory tree from symbol file paths.
fn file_tree(nodes: &[Node]) -> TreeNode {
    let mut root = TreeNode {
        name: String::new(),
        ..TreeNode::default()
    };
    for n in nodes {
        if n.file_path.is_empty() {
            continue;
        }
        let segments: Vec<&str> = n.file_path.split('/').filter(|s| !s.is_empty()).collect();
        let Some((file, dirs)) = segments.split_last() else {
            continue;
        };
        let mut cursor = &mut root;
        for dir in dirs {
            let pos = if let Some(p) = cursor.children.iter().position(|c| c.name == *dir) {
                p
            } else {
                cursor.children.push(TreeNode {
                    name: (*dir).to_string(),
                    ..TreeNode::default()
                });
                cursor.children.len() - 1
            };
            cursor = &mut cursor.children[pos];
        }
        if !cursor.files.iter().any(|f| f == file) {
            cursor.files.push((*file).to_string());
        }
    }
    // Roll symbol counts up the tree and sort children/files.
    rollup(&mut root, nodes);
    root
}

/// Recursively sort and total the tree; returns the symbol count under `node`.
fn rollup(node: &mut TreeNode, nodes: &[Node]) -> usize {
    let mut total = 0usize;
    for child in &mut node.children {
        total += rollup(child, nodes);
    }
    // Count symbols whose file path ends in one of this directory's files.
    for file in &node.files {
        total += nodes
            .iter()
            .filter(|n| n.label != Label::File && path_ends_with(&n.file_path, file))
            .count();
    }
    node.files.sort();
    node.children.sort_by(|a, b| a.name.cmp(&b.name));
    node.symbol_count = total;
    total
}

/// Whether a full path's final segment equals `file` (used by the tree rollup).
fn path_ends_with(path: &str, file: &str) -> bool {
    path.rsplit('/').next().is_some_and(|last| last == file)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Edge, Node};
    use serde_json::json;

    fn node(id: i64, qn: &str, file: &str, label: Label) -> Node {
        Node {
            id,
            project: "p".into(),
            label,
            name: qn.rsplit("::").next().unwrap_or(qn).to_string(),
            qualified_name: qn.into(),
            file_path: file.into(),
            start_line: 1,
            end_line: 2,
            props: json!({}),
        }
    }

    fn edge(src: i64, tgt: i64, ty: EdgeType) -> Edge {
        Edge {
            id: 0,
            project: "p".into(),
            source: src,
            target: tgt,
            edge_type: ty,
            props: json!({}),
        }
    }

    #[test]
    fn fold_to_files_aggregates_and_drops_same_file() {
        let nodes = [
            node(1, "a::x", "a.rs", Label::Function),
            node(2, "b::y", "b.rs", Label::Function),
            node(3, "a::z", "a.rs", Label::Function),
        ];
        let node_file: HashMap<i64, String> =
            nodes.iter().map(|n| (n.id, n.file_path.clone())).collect();
        let edges = [
            edge(1, 2, EdgeType::Calls),
            edge(3, 2, EdgeType::Calls),
            edge(1, 3, EdgeType::Calls),    // same file → dropped
            edge(1, 2, EdgeType::Contains), // not a dependency → dropped
        ];
        let folded = fold_to_files(&edges, &node_file);
        assert_eq!(folded.len(), 1, "{folded:?}");
        assert_eq!(folded[0].from, "a.rs");
        assert_eq!(folded[0].to, "b.rs");
        assert_eq!(folded[0].weight, 2);
        assert_eq!(folded[0].kinds, vec!["CALLS".to_string()]);
    }

    #[test]
    fn cycles_finds_scc_of_two_files() {
        let edges = vec![
            ModuleEdge {
                from: "a.rs".into(),
                to: "b.rs".into(),
                weight: 1,
                kinds: vec!["CALLS".into()],
            },
            ModuleEdge {
                from: "b.rs".into(),
                to: "a.rs".into(),
                weight: 1,
                kinds: vec!["CALLS".into()],
            },
            ModuleEdge {
                from: "b.rs".into(),
                to: "c.rs".into(),
                weight: 1,
                kinds: vec!["CALLS".into()],
            },
        ];
        let cyc = cycles(&edges);
        assert_eq!(cyc.len(), 1, "{cyc:?}");
        assert_eq!(cyc[0].files, vec!["a.rs".to_string(), "b.rs".to_string()]);
        assert_eq!(cyc[0].edges.len(), 2, "{:?}", cyc[0].edges);
    }

    #[test]
    fn acyclic_graph_has_no_cycles() {
        let edges = vec![
            ModuleEdge {
                from: "a.rs".into(),
                to: "b.rs".into(),
                weight: 1,
                kinds: vec!["CALLS".into()],
            },
            ModuleEdge {
                from: "b.rs".into(),
                to: "c.rs".into(),
                weight: 1,
                kinds: vec!["CALLS".into()],
            },
        ];
        assert_eq!(cycles(&edges).len(), 0);
    }

    #[test]
    fn layers_assign_increasing_depth() {
        let edges = vec![
            ModuleEdge {
                from: "app.rs".into(),
                to: "svc.rs".into(),
                weight: 1,
                kinds: vec!["CALLS".into()],
            },
            ModuleEdge {
                from: "svc.rs".into(),
                to: "db.rs".into(),
                weight: 1,
                kinds: vec!["CALLS".into()],
            },
        ];
        let layers = layers(&edges);
        // app depends on svc depends on db: db is level 0 (nothing depends on
        // it), app is deepest.
        let depth = |file: &str| {
            layers
                .iter()
                .find(|l| l.files.iter().any(|f| f == file))
                .map(|l| l.level)
        };
        assert!(
            depth("db.rs").unwrap() < depth("svc.rs").unwrap(),
            "{layers:?}"
        );
        assert!(
            depth("svc.rs").unwrap() < depth("app.rs").unwrap(),
            "{layers:?}"
        );
    }

    #[test]
    fn clusters_split_disconnected_files() {
        let edges = vec![
            ModuleEdge {
                from: "a.rs".into(),
                to: "b.rs".into(),
                weight: 1,
                kinds: vec!["CALLS".into()],
            },
            ModuleEdge {
                from: "x.rs".into(),
                to: "y.rs".into(),
                weight: 1,
                kinds: vec!["CALLS".into()],
            },
        ];
        let cl = clusters(&edges);
        assert_eq!(cl.len(), 2, "{cl:?}");
        assert!(cl.iter().all(|c| c.files.len() == 2));
    }

    #[test]
    fn hotspots_rank_by_degree() {
        let nodes = [
            node(1, "a::hub", "a.rs", Label::Function),
            node(2, "a::leaf", "a.rs", Label::Function),
        ];
        let edges = [edge(1, 2, EdgeType::Calls), edge(2, 1, EdgeType::Calls)];
        let h = hotspots(&nodes, &edges);
        assert_eq!(h.len(), 2);
        assert_eq!(h[0].degree, 2);
    }

    #[test]
    fn entry_points_are_uncalled_callers() {
        let nodes = [
            node(1, "a::main", "a.rs", Label::Function),
            node(2, "a::helper", "a.rs", Label::Function),
        ];
        let edges = [edge(1, 2, EdgeType::Calls)];
        let ep = entry_points(&nodes, &edges);
        assert_eq!(ep.len(), 1, "{ep:?}");
        assert_eq!(ep[0].name, "main");
        assert_eq!(ep[0].out_degree, 1);
    }

    #[test]
    fn file_tree_nests_directories() {
        let nodes = [
            node(1, "x::a", "src/deep/a.rs", Label::Function),
            node(2, "x::b", "src/b.rs", Label::Function),
        ];
        let tree = file_tree(&nodes);
        assert_eq!(tree.symbol_count, 2);
        let src = tree.children.iter().find(|c| c.name == "src").expect("src");
        assert!(src.files.contains(&"b.rs".to_string()), "{src:?}");
        let deep = src
            .children
            .iter()
            .find(|c| c.name == "deep")
            .expect("deep");
        assert!(deep.files.contains(&"a.rs".to_string()));
        assert_eq!(deep.symbol_count, 1);
    }
}
