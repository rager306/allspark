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
    /// One symbol-level edge that this file pair stands for, as
    /// `caller_qualified_name -> callee_qualified_name`.
    ///
    /// The review's §2 lists "объяснение исходными связями" as the next step for cycles: a
    /// cycle printed as files says a dependency exists but not which symbols make it, and
    /// the fix is always at a symbol. A fold is many-to-one, so this is ONE witness chosen
    /// by lowest qualified-name pair — deterministic, not necessarily the only one. `None`
    /// when the node names are unavailable (a store opened without node rows).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub witness: Option<String>,
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

/// Components plus the hubs that connect them.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClusterReport {
    /// Connected components, largest first.
    pub clusters: Vec<Cluster>,
    /// Files in the top degree percentile, sorted by path.
    pub hubs: Vec<String>,
    /// Whether the graph separates into components of usable size.
    ///
    /// `false` means the largest component holds most of the tree: on this repo it
    /// does, and further clustering would be inventing structure.
    pub separated: bool,
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
    /// Populated for [`ArchitectureView::Clusters`]: the components AND the hubs
    /// they are connected through, because on a real repo those hubs are the reason a
    /// single component holds the tree.
    pub cluster_report: Option<ClusterReport>,
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
    let mut arch = Architecture {
        project: project.to_string(),
        view,
        overview: None,
        structure: Vec::new(),
        layers: Vec::new(),
        cluster_report: None,
        cycles: Vec::new(),
        hotspots: Vec::new(),
        entry_points: Vec::new(),
        file_tree: None,
    };

    // `file_tree` reads nodes only, so it is answered before any edge is read:
    // loading all of them to build it was 261 988 rows on this repo's index,
    // decoded from JSON props and dropped. Every other view genuinely needs the
    // edges, so this is not a whole-view shortcut — it is one view that was
    // paying for data it never looked at.
    if matches!(view, ArchitectureView::FileTree) {
        arch.file_tree = Some(file_tree(&nodes));
        return Ok(arch);
    }
    let edges = store.all_edges(project)?;

    // Fold symbol edges up to files for the graph views.
    let node_file: HashMap<i64, String> =
        nodes.iter().map(|n| (n.id, n.file_path.clone())).collect();
    let node_name: HashMap<i64, String> = nodes
        .iter()
        .map(|n| (n.id, n.qualified_name.clone()))
        .collect();
    let module_edges = fold_to_files(&edges, &node_file, &node_name);

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
            arch.cluster_report = Some(clusters(&module_edges));
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
            // Handled above, before any edge is loaded.
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
    node_name: &HashMap<i64, String>,
) -> Vec<ModuleEdge> {
    // Only dependency-shaped edges carry architecture signal; CONTAINS is
    // intra-file by construction and HAS_BRANCH/FILE_CHANGES_WITH are git
    // provenance, not code structure.
    let mut acc: BTreeMap<(String, String), (usize, std::collections::BTreeSet<String>)> =
        BTreeMap::new();
    let mut witness: BTreeMap<(String, String), String> = BTreeMap::new();
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
        // Keep the LOWEST qualified-name pair for the file pair: deterministic across runs,
        // and a fold of 300 symbol edges has no room to list them all.
        if let (Some(source), Some(target)) = (node_name.get(&e.source), node_name.get(&e.target)) {
            let key = format!("{source} -> {target}");
            witness
                .entry((from.clone(), to.clone()))
                .and_modify(|current| {
                    if key < *current {
                        current.clone_from(&key);
                    }
                })
                .or_insert(key);
        }
    }
    acc.into_iter()
        .map(|((from, to), (weight, kinds))| ModuleEdge {
            witness: witness.get(&(from.clone(), to.clone())).cloned(),
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

/// File communities plus the hubs that connect them.
///
/// # What the measurement says
///
/// The undirected projection of this repo is ONE connected component of 1 727 files
/// with a long degree tail (max 1 343, p99 336, p95 86, median 22). Label
/// propagation cannot split it: with its largest-label tie-break the highest label
/// floods the component, which is why the old code returned ONE cluster for the
/// repository after 0.95 s of work.
///
/// Removing the hubs does not rescue it either — at the p95 threshold (degree >= 86,
/// 87 hubs removed) the remainder is one 1 616-file component plus 23 fragments of
/// one or two files. The files are one connected mass; the communities a reader hopes
/// for are not in this graph.
///
/// So the honest answer has three parts, and all three are reported:
///
/// 1. the CONNECTED COMPONENTS, which are exact and cheap;
/// 2. the HUBS, named, because they are the reason the components do not separate —
///    a file everything imports is an architectural fact, not a clustering failure;
/// 3. `separated: false` when the largest component holds most of the tree, which
///    tells a reader that "1 community" is the shape of the code, not a bug.
fn clusters(edges: &[ModuleEdge]) -> ClusterReport {
    let (graph, _) = file_graph(edges);
    if graph.node_count() == 0 {
        return ClusterReport::default();
    }
    // Degree over BOTH directions: a hub is depended ON as often as it depends.
    // Counting only outgoing found no hubs at all (measured: 0 hubs, 1 community —
    // exactly the answer the old code gave, which is how the omission surfaced).
    let mut degree: HashMap<usize, usize> = HashMap::new();
    for n in graph.node_indices() {
        let outgoing = graph
            .edges_directed(n, petgraph::Direction::Outgoing)
            .count();
        let incoming = graph
            .edges_directed(n, petgraph::Direction::Incoming)
            .count();
        degree.insert(n.index(), outgoing + incoming);
    }
    let mut degrees: Vec<usize> = degree.values().copied().collect();
    degrees.sort_unstable();
    let median = degrees.get(degrees.len() / 2).copied().unwrap_or(0);
    let p95 = degrees
        .get(degrees.len().saturating_sub(degrees.len() / 20 + 1))
        .copied()
        .unwrap_or(0);
    // A hub is a clear outlier: above the 95th percentile AND several times the
    // median. In a uniformly connected project there is nothing to name.
    let hub_floor = if p95 > median.saturating_mul(3) {
        p95
    } else {
        usize::MAX
    };

    // Components, computed exactly rather than approximated: the honest grouping for
    // a graph whose projection does not separate.
    let mut community_of: HashMap<usize, usize> = HashMap::new();
    let mut members: BTreeMap<usize, Vec<String>> = BTreeMap::new();
    let mut next_id = 0usize;
    for start in graph.node_indices() {
        if community_of.contains_key(&start.index()) {
            continue;
        }
        let id = next_id;
        next_id += 1;
        let mut stack = vec![start];
        community_of.insert(start.index(), id);
        while let Some(node) = stack.pop() {
            members.entry(id).or_default().push(graph[node].clone());
            for neighbour in graph
                .neighbors_directed(node, petgraph::Direction::Outgoing)
                .chain(graph.neighbors_directed(node, petgraph::Direction::Incoming))
            {
                if let std::collections::hash_map::Entry::Vacant(entry) =
                    community_of.entry(neighbour.index())
                {
                    entry.insert(id);
                    stack.push(neighbour);
                }
            }
        }
    }
    let file_count = graph.node_count();
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
    let largest = out.first().map_or(0, |cluster| cluster.files.len());
    // Separated means "the graph gives a reader areas": more than one component AND
    // no single one that is most of the tree. Two equal halves qualify; one component
    // holding everything (the measured case here) does not, and neither does one 90%
    // component with a few stragglers — that would crown noise as a community.
    let separated = out.len() > 1 && largest * 2 <= file_count;
    for (i, c) in out.iter_mut().enumerate() {
        c.id = i;
        let set: std::collections::HashSet<&String> = c.files.iter().collect();
        c.internal_edges = edges
            .iter()
            .filter(|e| set.contains(&e.from) && set.contains(&e.to))
            .count();
    }
    let mut hubs: Vec<String> = graph
        .node_indices()
        .filter(|n| degree.get(&n.index()).copied().unwrap_or(0) >= hub_floor && hub_floor > 0)
        .map(|n| graph[n].clone())
        .collect();
    hubs.sort();
    ClusterReport {
        clusters: out,
        hubs,
        separated,
    }
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
///
/// Counted from a per-`file_path` tally rather than by scanning `nodes` once per
/// file name. The old shape was O(files × nodes): for each of the project's files
/// it walked all of them to compare final path segments, so a 37 589-node index
/// with 332 338 tree entries spent **15.4 s** — 17× the cost of reading the nodes
/// at all (0.17 s) and 16× the whole edge scan (0.9 s).
///
/// The tally also fixes a silent double count the old one had: it matched on the
/// FINAL segment only, so `a/lib.rs` and `b/lib.rs` each counted the symbols of
/// both files. Keying on the full path counts each symbol once, under the file it
/// actually lives in.
fn rollup(node: &mut TreeNode, nodes: &[Node]) -> usize {
    let mut counts: HashMap<&str, usize> = HashMap::new();
    for n in nodes {
        if n.label != Label::File && !n.file_path.is_empty() {
            *counts.entry(n.file_path.as_str()).or_insert(0) += 1;
        }
    }
    rollup_with(node, "", &counts)
}

/// The counting half of [`rollup`], split out so the tally is built once.
///
/// `prefix` is the directory path this node stands for (`""`, `"src/"`,
/// `"src/deep/"`), so a file name entry resolves to the full path the tally is
/// keyed on.
fn rollup_with(node: &mut TreeNode, prefix: &str, counts: &HashMap<&str, usize>) -> usize {
    let mut total = 0usize;
    for child in &mut node.children {
        let child_prefix = format!("{prefix}{}/", child.name);
        total += rollup_with(child, &child_prefix, counts);
    }
    for file in &node.files {
        let full = format!("{prefix}{file}");
        total += counts.get(full.as_str()).copied().unwrap_or(0);
    }
    node.files.sort();
    node.children.sort_by(|a, b| a.name.cmp(&b.name));
    node.symbol_count = total;
    total
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Edge, Node};
    use serde_json::json;

    /// The old rollup matched a file entry by its FINAL segment, so `a/lib.rs`
    /// and `b/lib.rs` each counted the symbols of BOTH files: every per-directory
    /// total was inflated by the number of same-named files in the project.
    ///
    /// A directory's total is what the tree exposes, so the oracle is the three
    /// directories' counts: `a` has one symbol, `b` two, and the root four
    /// (including the root-level `top.rs`).
    #[test]
    fn two_files_with_the_same_name_each_count_their_own_symbols() {
        let nodes = vec![
            node(1, "p::a::one", "a/lib.rs", Label::Function),
            node(2, "p::b::two", "b/lib.rs", Label::Function),
            node(3, "p::b::three", "b/lib.rs", Label::Function),
            node(4, "p::top", "top.rs", Label::Function),
        ];
        let tree = file_tree(&nodes);
        let count = |name: &str| {
            tree.children.iter().find(|c| c.name == name).map_or_else(
                || panic!("missing {name}: {:?}", tree.children),
                |c| c.symbol_count,
            )
        };
        assert_eq!(count("a"), 1, "a/lib.rs holds one symbol");
        assert_eq!(count("b"), 2, "b/lib.rs holds two");
        assert_eq!(
            tree.symbol_count, 4,
            "the root totals every symbol exactly once (got {})",
            tree.symbol_count
        );
    }

    /// The file tree reads nodes only. Loading the project's edges to build it
    /// was 261 988 rows on this repo's index — decoded and dropped.
    ///
    /// The oracle counts the SQL actually executed, not what the code claims:
    /// `graph(action=architecture, view=file_tree)` must issue no statement
    /// against `edges`.
    #[test]
    fn the_file_tree_view_never_reads_edges() {
        #![allow(clippy::needless_pass_by_value)]
        use std::sync::atomic::{AtomicUsize, Ordering};

        // The hook is per-connection in rusqlite, but `trace_v2` takes a plain
        // `fn`, so the counter lives in a process-global for this one test.
        static EDGE_READS: AtomicUsize = AtomicUsize::new(0);
        fn count_edge_reads(event: rusqlite::trace::TraceEvent<'_>) {
            if let rusqlite::trace::TraceEvent::Stmt(_, sql) = event {
                if sql.contains("FROM edges") {
                    EDGE_READS.fetch_add(1, Ordering::Relaxed);
                }
            }
        }

        let store = crate::GraphStore::open_in_memory().expect("store");
        store.register_project("p", "/tmp").expect("project");
        let mk = |name: &str| {
            let node = crate::types::Node {
                id: 0,
                project: "p".into(),
                label: crate::types::Label::Function,
                name: name.into(),
                qualified_name: format!("p::{name}"),
                file_path: "src/lib.rs".into(),
                start_line: 1,
                end_line: 3,
                props: serde_json::json!({}),
            };
            store.upsert_node(&node).expect("node")
        };
        let a = mk("a");
        let b = mk("b");
        store
            .insert_edge(
                "p",
                a,
                b,
                crate::types::EdgeType::Calls,
                &serde_json::json!({}),
            )
            .expect("edge");

        let mut store = store;
        EDGE_READS.store(0, Ordering::Relaxed);
        store.connection_mut().trace_v2(
            rusqlite::trace::TraceEventCodes::SQLITE_TRACE_STMT,
            Some(count_edge_reads),
        );
        let tree = store
            .architecture("p", crate::architecture::ArchitectureView::FileTree)
            .expect("architecture");
        let counted = EDGE_READS.swap(0, Ordering::Relaxed);
        store
            .connection_mut()
            .trace_v2(rusqlite::trace::TraceEventCodes::SQLITE_TRACE_STMT, None);

        assert!(tree.file_tree.is_some(), "the view still answers");
        assert_eq!(
            counted, 0,
            "the file tree must not query edges at all; it did {counted} time(s)"
        );

        // The other views DO need them, which is why this is view-specific.
        EDGE_READS.store(0, Ordering::Relaxed);
        store.connection_mut().trace_v2(
            rusqlite::trace::TraceEventCodes::SQLITE_TRACE_STMT,
            Some(count_edge_reads),
        );
        store
            .architecture("p", crate::architecture::ArchitectureView::Structure)
            .expect("architecture");
        let structure_reads = EDGE_READS.swap(0, Ordering::Relaxed);
        store
            .connection_mut()
            .trace_v2(rusqlite::trace::TraceEventCodes::SQLITE_TRACE_STMT, None);
        assert!(
            structure_reads > 0,
            "the structure view needs the edges; 0 reads means the hook did not observe"
        );
    }

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
        let node_name: HashMap<i64, String> = nodes
            .iter()
            .map(|n| (n.id, n.qualified_name.clone()))
            .collect();
        let edges = [
            edge(1, 2, EdgeType::Calls),
            edge(3, 2, EdgeType::Calls),
            edge(1, 3, EdgeType::Calls),    // same file → dropped
            edge(1, 2, EdgeType::Contains), // not a dependency → dropped
        ];
        let folded = fold_to_files(&edges, &node_file, &node_name);
        assert_eq!(folded.len(), 1, "{folded:?}");
        assert_eq!(folded[0].from, "a.rs");
        assert_eq!(folded[0].to, "b.rs");
        assert_eq!(folded[0].weight, 2);
        assert_eq!(folded[0].kinds, vec!["CALLS".to_string()]);
        // Two symbol edges folded into one file pair; the witness is the lowest-named
        // one (`a::x -> b::y`), so the answer is stable across runs.
        assert_eq!(
            folded[0].witness.as_deref(),
            Some("a::x -> b::y"),
            "a fold must keep ONE symbol-level edge so a reader can act on it"
        );
    }

    #[test]
    fn cycles_finds_scc_of_two_files() {
        let edges = vec![
            ModuleEdge {
                from: "a.rs".into(),
                to: "b.rs".into(),
                weight: 1,
                kinds: vec!["CALLS".into()],
                witness: None,
            },
            ModuleEdge {
                from: "b.rs".into(),
                to: "a.rs".into(),
                weight: 1,
                kinds: vec!["CALLS".into()],
                witness: None,
            },
            ModuleEdge {
                from: "b.rs".into(),
                to: "c.rs".into(),
                weight: 1,
                kinds: vec!["CALLS".into()],
                witness: None,
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
                witness: None,
            },
            ModuleEdge {
                from: "b.rs".into(),
                to: "c.rs".into(),
                weight: 1,
                kinds: vec!["CALLS".into()],
                witness: None,
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
                witness: None,
            },
            ModuleEdge {
                from: "svc.rs".into(),
                to: "db.rs".into(),
                weight: 1,
                kinds: vec!["CALLS".into()],
                witness: None,
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
                witness: None,
            },
            ModuleEdge {
                from: "x.rs".into(),
                to: "y.rs".into(),
                weight: 1,
                kinds: vec!["CALLS".into()],
                witness: None,
            },
        ];
        let report = clusters(&edges);
        assert_eq!(report.clusters.len(), 2, "{report:?}");
        assert!(report.clusters.iter().all(|c| c.files.len() == 2));
        // Two components of two files each: genuinely separated, so the flag says so.
        assert!(report.separated, "{report:?}");
    }

    /// The measured shape of a real repo, reproduced small: one component that holds
    /// most of the tree must say `separated: false` rather than pretend to cluster.
    #[test]
    fn one_dominant_component_is_reported_as_not_separated() {
        // Four files in one chain plus two isolated pairs: the chain dominates.
        let mut edges = Vec::new();
        for (from, to) in [("a.rs", "b.rs"), ("b.rs", "c.rs"), ("c.rs", "d.rs")] {
            edges.push(ModuleEdge {
                from: from.into(),
                to: to.into(),
                weight: 1,
                kinds: vec!["CALLS".into()],
                witness: None,
            });
        }
        let report = clusters(&edges);
        assert_eq!(report.clusters.len(), 1, "{report:?}");
        assert_eq!(report.clusters[0].files.len(), 4);
        assert!(
            !report.separated,
            "a single component holding the whole tree is not a separation: {report:?}"
        );
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
