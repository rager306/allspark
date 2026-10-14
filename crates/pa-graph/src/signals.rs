
//! V3 — eleven native similarity signals (no external embeddings).
//!
//! Each symbol is mapped to a fixed-dimension vector per signal; the signals
//! are concatenated into a [`SignalVector`] whose cosine similarity scores two
//! symbols. Nothing here needs a model, a network, or a training step — every
//! signal is a deterministic function of the node, its text, and the graph.
//!
//! | # | Signal | What it captures |
//! |---|---|---|
//! | 1 | [`SignalVector::tfidf`] | lexical overlap of identifier/comment terms |
//! | 2 | [`SignalVector::random_indexing`] | distributional synonymy (co-occurrence) |
//! | 3 | [`SignalVector::minhash`] | structural (trigram) overlap — clone signal |
//! | 4 | [`SignalVector::api_signature`] | same callees → same behaviour surface |
//! | 5 | [`SignalVector::type_signature`] | same parameter/return types |
//! | 6 | [`SignalVector::module_proximity`] | same directory / package |
//! | 7 | [`SignalVector::decorators`] | same annotations/attributes |
//! | 8 | [`SignalVector::ast_profile`] | control-flow / shape histogram |
//! | 9 | [`SignalVector::dataflow`] | argument-expression overlap |
//! | 10 | [`SignalVector::graph_diffusion`] | transitively shared neighbours |
//! | 11 | [`SignalVector::halstead`] | operator/operand complexity profile |
//!
//! All vectors are `L2`-normalized so the concatenation is comparable under a
//! plain cosine.

use std::collections::{HashMap, HashSet};

use crate::error::Result;
use crate::store::GraphStore;
use crate::tokenize::split_ident;
use crate::types::{EdgeType, Node};

/// Fixed dimension of the lexical signals (`TF-IDF`, diffusion).
const LEX_DIM: usize = 96;
/// Dimension of the Random-Indexing signal.
const RI_DIM: usize = 64;
/// Dimension of the `MinHash` signal (equals the signature width).
const MINHASH_DIM: usize = crate::minhash::MINHASH_K;
/// Dimension of the `API`-signature signal.
const API_DIM: usize = 64;
/// Dimension of the type-signature signal.
const TYPE_DIM: usize = 32;
/// Dimension of the module-proximity signal.
const PROX_DIM: usize = 32;
/// Dimension of the decorator signal.
const DECOR_DIM: usize = 16;
/// Dimension of the `AST`-profile signal.
const AST_DIM: usize = 16;
/// Dimension of the data-flow signal.
const DATAFLOW_DIM: usize = 32;
/// Dimension of the `Halstead` signal.
const HALSTEAD_DIM: usize = 8;

/// The eleven concatenated signals for one symbol.
#[derive(Debug, Clone, PartialEq)]
pub struct SignalVector {
    /// 1. `TF-IDF` over split identifier/comment tokens.
    pub tfidf: Vec<f32>,
    /// 2. `Random Indexing` co-occurrence.
    pub random_indexing: Vec<f32>,
    /// 3. `MinHash` signature over token trigrams.
    pub minhash: Vec<f32>,
    /// 4. `API` signature (callees).
    pub api_signature: Vec<f32>,
    /// 5. Type signature (param/return types).
    pub type_signature: Vec<f32>,
    /// 6. Module proximity.
    pub module_proximity: Vec<f32>,
    /// 7. Decorators / attributes.
    pub decorators: Vec<f32>,
    /// 8. `AST` structural profile.
    pub ast_profile: Vec<f32>,
    /// 9. Data-flow (argument expressions).
    pub dataflow: Vec<f32>,
    /// 10. Graph diffusion (neighbour-averaged lexical vector).
    pub graph_diffusion: Vec<f32>,
    /// 11. `Halstead`-lite complexity profile.
    pub halstead: Vec<f32>,
}

impl SignalVector {
    /// Total dimension of the concatenated vector.
    #[must_use]
    pub const fn dim() -> usize {
        LEX_DIM
            + RI_DIM
            + MINHASH_DIM
            + API_DIM
            + TYPE_DIM
            + PROX_DIM
            + DECOR_DIM
            + AST_DIM
            + DATAFLOW_DIM
            + LEX_DIM
            + HALSTEAD_DIM
    }

    /// Concatenate the eleven signals into one vector.
    #[must_use]
    pub fn concat(&self) -> Vec<f32> {
        let mut out = Vec::with_capacity(Self::dim());
        out.extend_from_slice(&self.tfidf);
        out.extend_from_slice(&self.random_indexing);
        out.extend_from_slice(&self.minhash);
        out.extend_from_slice(&self.api_signature);
        out.extend_from_slice(&self.type_signature);
        out.extend_from_slice(&self.module_proximity);
        out.extend_from_slice(&self.decorators);
        out.extend_from_slice(&self.ast_profile);
        out.extend_from_slice(&self.dataflow);
        out.extend_from_slice(&self.graph_diffusion);
        out.extend_from_slice(&self.halstead);
        out
    }

    /// Cosine similarity of the concatenated signals of `self` and `other`.
    #[must_use]
    pub fn similarity(&self, other: &Self) -> f32 {
        crate::hybrid::cosine(&self.concat(), &other.concat())
    }
}

/// A read-only snapshot of the graph the signals are computed against: the
/// nodes, their edges, and corpus statistics (`IDF`, document tokens).
pub struct SymbolGraph {
    nodes: HashMap<i64, Node>,
    order: Vec<i64>,
    out: HashMap<i64, Vec<(i64, EdgeType)>>,
    inc: HashMap<i64, Vec<(i64, EdgeType)>>,
    out_arg_tokens: HashMap<i64, Vec<String>>,
    docs: HashMap<i64, Vec<String>>,
    idf: HashMap<String, f32>,
}

impl SymbolGraph {
    /// Snapshot `project` from `store`.
    ///
    /// # Errors
    /// Propagates SQLite errors.
    pub fn from_store(store: &GraphStore, project: &str) -> Result<Self> {
        let nodes_vec = store.all_nodes(project)?;
        let edges = store.all_edges(project)?;
        let mut nodes = HashMap::with_capacity(nodes_vec.len());
        let mut order = Vec::with_capacity(nodes_vec.len());
        for node in nodes_vec {
            order.push(node.id);
            nodes.insert(node.id, node);
        }
        let mut out: HashMap<i64, Vec<(i64, EdgeType)>> = HashMap::new();
        let mut inc: HashMap<i64, Vec<(i64, EdgeType)>> = HashMap::new();
        let mut out_arg_tokens: HashMap<i64, Vec<String>> = HashMap::new();
        for edge in &edges {
            out.entry(edge.source)
                .or_default()
                .push((edge.target, edge.edge_type));
            inc.entry(edge.target)
                .or_default()
                .push((edge.source, edge.edge_type));
            if edge.edge_type == EdgeType::Calls {
                if let Some(args) = edge.props.get("args").and_then(|v| v.as_array()) {
                    let entry = out_arg_tokens.entry(edge.source).or_default();
                    for arg in args {
                        if let Some(e) = arg.get("e").and_then(|v| v.as_str()) {
                            entry.extend(split_ident(e));
                        }
                    }
                }
            }
        }
        // Corpus stats: token document frequency for IDF.
        let mut docs: HashMap<i64, Vec<String>> = HashMap::new();
        let mut df: HashMap<String, u32> = HashMap::new();
        for node in nodes.values() {
            let tokens = node_tokens(node);
            let unique: HashSet<&String> = tokens.iter().collect();
            for t in unique {
                *df.entry(t.clone()).or_insert(0) += 1;
            }
            docs.insert(node.id, tokens);
        }
        let n_docs = nodes.len().max(1) as f32;
        let idf = df
            .into_iter()
            .map(|(t, d)| {
                let df = d as f32;
                (t, ((n_docs - df + 0.5) / (df + 0.5) + 1.0).ln())
            })
            .collect();
        Ok(Self {
            nodes,
            order,
            out,
            inc,
            out_arg_tokens,
            docs,
            idf,
        })
    }

    /// Number of nodes in the snapshot.
    #[must_use]
    pub fn len(&self) -> usize {
        self.order.len()
    }

    /// Whether the snapshot is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.order.is_empty()
    }

    /// Look up a node by id.
    #[must_use]
    pub fn node(&self, id: i64) -> Option<&Node> {
        self.nodes.get(&id)
    }

    /// All node ids in insertion order.
    #[must_use]
    pub fn node_ids(&self) -> &[i64] {
        &self.order
    }
}

/// Compute the eleven signals for `node` against `graph`.
///
/// A node that is not part of the snapshot still gets a well-formed vector
/// (its structural signals are simply empty).
#[must_use]
pub fn symbol_signals(node: &Node, graph: &SymbolGraph) -> SignalVector {
    let tokens = node_tokens(node);
    let tfidf = tfidf_vector(&tokens, graph);
    let random_indexing = random_indexing_vector(&tokens);
    let minhash = minhash_vector(&tokens);
    let api_signature = api_signature_vector(node, graph);
    let type_signature = type_signature_vector(node);
    let module_proximity = module_proximity_vector(&node.file_path);
    let decorators = decorator_vector(node);
    let ast_profile = ast_profile_vector(node, graph);
    let dataflow = dataflow_vector(node, graph);
    let graph_diffusion = diffusion_vector(node, graph, &tfidf);
    let halstead = halstead_vector(&tokens);
    SignalVector {
        tfidf,
        random_indexing,
        minhash,
        api_signature,
        type_signature,
        module_proximity,
        decorators,
        ast_profile,
        dataflow,
        graph_diffusion,
        halstead,
    }
}

// ── signal 1: TF-IDF ─────────────────────────────────────────────────────

fn tfidf_vector(tokens: &[String], graph: &SymbolGraph) -> Vec<f32> {
    let mut tf: HashMap<&str, u32> = HashMap::new();
    for t in tokens {
        *tf.entry(t.as_str()).or_insert(0) += 1;
    }
    let mut v = vec![0.0f32; LEX_DIM];
    for (t, count) in tf {
        let idf = graph.idf.get(t).copied().unwrap_or(1.0);
        let w = (count as f32) * idf;
        let idx = hash_index(t, LEX_DIM);
        v[idx] += w;
    }
    l2_normalize(v)
}

// ── signal 2: Random Indexing ────────────────────────────────────────────

fn random_indexing_vector(tokens: &[String]) -> Vec<f32> {
    // Each token gets a sparse random vector (8 nonzeros); the sum is the
    // distributional representation. Co-occurring tokens share buckets.
    let mut v = vec![0.0f32; RI_DIM];
    for t in tokens {
        let base = fnv1a(t.as_bytes());
        for k in 0..8u64 {
            let h = mix(base, k);
            let idx = (h as usize) % RI_DIM;
            let sign = if h & (1 << 40) == 0 { 1.0 } else { -1.0 };
            v[idx] += sign;
        }
    }
    l2_normalize(v)
}

// ── signal 3: MinHash ────────────────────────────────────────────────────

fn minhash_vector(tokens: &[String]) -> Vec<f32> {
    let grams = crate::minhash::trigrams(tokens);
    let sig = crate::minhash::minhash(&grams);
    // Normalize each word to [0, 1] so cosine sees a bounded vector.
    sig.iter()
        .map(|&w| if w == u64::MAX { 0.0 } else { (w >> 11) as f32 / (1u64 << 53) as f32 })
        .collect()
}

// ── signal 4: API signature (callees) ────────────────────────────────────

fn api_signature_vector(node: &Node, graph: &SymbolGraph) -> Vec<f32> {
    let mut v = vec![0.0f32; API_DIM];
    if let Some(edges) = graph.out.get(&node.id) {
        for (target, etype) in edges {
            if *etype != EdgeType::Calls {
                continue;
            }
            if let Some(callee) = graph.nodes.get(target) {
                for tok in split_ident(&callee.name) {
                    v[hash_index(&tok, API_DIM)] += 1.0;
                }
            }
        }
    }
    l2_normalize(v)
}

// ── signal 5: type signature ─────────────────────────────────────────────

fn type_signature_vector(node: &Node) -> Vec<f32> {
    let mut v = vec![0.0f32; TYPE_DIM];
    // Prefer an explicit signature prop (params + return), if the indexer
    // recorded one; otherwise fall back to capitalized identifier-like tokens.
    let signature = node.prop_str("signature").unwrap_or("");
    let mut types: Vec<String> = Vec::new();
    if signature.is_empty() {
        for t in split_ident(&node.qualified_name) {
            if t.chars().next().is_some_and(char::is_uppercase) {
                types.push(t);
            }
        }
    } else {
        for tok in signature.split(|c: char| !c.is_alphanumeric() && c != '_') {
            if !tok.is_empty() {
                types.push(tok.to_lowercase());
            }
        }
    }
    for t in types {
        v[hash_index(&t, TYPE_DIM)] += 1.0;
    }
    l2_normalize(v)
}

// ── signal 6: module proximity ───────────────────────────────────────────

fn module_proximity_vector(file_path: &str) -> Vec<f32> {
    let mut v = vec![0.0f32; PROX_DIM];
    // Encode every path prefix so files in the same directory share buckets.
    let parts: Vec<&str> = file_path.split('/').filter(|s| !s.is_empty()).collect();
    for depth in 1..=parts.len() {
        let prefix = parts[..depth].join("/");
        v[hash_index(&prefix, PROX_DIM)] += 1.0;
    }
    l2_normalize(v)
}

// ── signal 7: decorators ─────────────────────────────────────────────────

fn decorator_vector(node: &Node) -> Vec<f32> {
    let mut v = vec![0.0f32; DECOR_DIM];
    let decorators = node.prop_str("decorators").unwrap_or("");
    let mut names: Vec<String> = Vec::new();
    if decorators.is_empty() {
        // Fall back to `#[attr]` / `@decorator` markers in the doc/body.
        for raw in node.body().split_whitespace() {
            if let Some(rest) = raw.strip_prefix("#[") {
                for tok in split_ident(rest.trim_end_matches(']')) {
                    names.push(tok);
                }
            } else if let Some(rest) = raw.strip_prefix('@') {
                for tok in split_ident(rest) {
                    names.push(tok);
                }
            }
        }
    } else {
        for d in decorators.split(',') {
            for tok in split_ident(d) {
                names.push(tok);
            }
        }
    }
    for t in names {
        v[hash_index(&t, DECOR_DIM)] += 1.0;
    }
    l2_normalize(v)
}

// ── signal 8: AST structural profile ─────────────────────────────────────

fn ast_profile_vector(node: &Node, graph: &SymbolGraph) -> Vec<f32> {
    // A cheap structural histogram available without the tree: label one-hot,
    // token-length buckets, and degree buckets (layout: 8 + 2 + 2 + 2 + 2).
    let mut v = vec![0.0f32; AST_DIM];
    let label_idx = match node.label {
        crate::types::Label::Function => 0,
        crate::types::Label::Method => 1,
        crate::types::Label::Class => 2,
        crate::types::Label::Struct => 3,
        crate::types::Label::Enum => 4,
        crate::types::Label::Interface => 5,
        crate::types::Label::Module => 6,
        _ => 7,
    };
    v[label_idx] += 1.0;
    v[8 + token_len_bucket(split_ident(&node.name).len())] += 1.0;
    let out_deg = graph.out.get(&node.id).map_or(0, Vec::len);
    let in_deg = graph.inc.get(&node.id).map_or(0, Vec::len);
    v[10 + degree_bucket(out_deg)] += 1.0;
    v[12 + degree_bucket(in_deg)] += 1.0;
    l2_normalize(v)
}

/// Bucket an identifier's token count into `[0, 1]`.
fn token_len_bucket(n: usize) -> usize {
    usize::from(n > 1)
}

/// Bucket a degree into `[0, 3]`.
fn degree_bucket(n: usize) -> usize {
    match n {
        0 => 0,
        1..=2 => 1,
        3..=5 => 2,
        _ => 3,
    }
}

// ── signal 9: data-flow ──────────────────────────────────────────────────

fn dataflow_vector(node: &Node, graph: &SymbolGraph) -> Vec<f32> {
    let mut v = vec![0.0f32; DATAFLOW_DIM];
    if let Some(tokens) = graph.out_arg_tokens.get(&node.id) {
        for t in tokens {
            v[hash_index(t, DATAFLOW_DIM)] += 1.0;
        }
    }
    l2_normalize(v)
}

// ── signal 10: graph diffusion ───────────────────────────────────────────

fn diffusion_vector(node: &Node, graph: &SymbolGraph, seed: &[f32]) -> Vec<f32> {
    // combined = (1 - α)·seed + α·mean(neighbour seeds).
    const ALPHA: f32 = 0.5;
    let mut acc = vec![0.0f32; LEX_DIM];
    let mut count = 0usize;
    let mut neighbours: Vec<i64> = Vec::new();
    if let Some(edges) = graph.out.get(&node.id) {
        neighbours.extend(edges.iter().map(|(t, _)| *t));
    }
    if let Some(edges) = graph.inc.get(&node.id) {
        neighbours.extend(edges.iter().map(|(s, _)| *s));
    }
    neighbours.sort_unstable();
    neighbours.dedup();
    for n in &neighbours {
        if graph.nodes.contains_key(n) {
            let nbr_vec = tfidf_vector(&graph.docs.get(n).cloned().unwrap_or_default(), graph);
            for (a, b) in acc.iter_mut().zip(nbr_vec.iter()) {
                *a += *b;
            }
            count += 1;
        }
    }
    if count == 0 {
        return l2_normalize(seed.to_vec());
    }
    let inv = 1.0 / count as f32;
    let mut out = vec![0.0f32; LEX_DIM];
    for i in 0..LEX_DIM {
        out[i] = (1.0 - ALPHA) * seed[i] + ALPHA * acc[i] * inv;
    }
    l2_normalize(out)
}

// ── signal 11: Halstead-lite ─────────────────────────────────────────────

fn halstead_vector(tokens: &[String]) -> Vec<f32> {
    let mut operators: HashMap<&str, u32> = HashMap::new();
    let mut operands: HashMap<&str, u32> = HashMap::new();
    for t in tokens {
        if is_operator(t) {
            *operators.entry(t.as_str()).or_insert(0) += 1;
        } else {
            *operands.entry(t.as_str()).or_insert(0) += 1;
        }
    }
    let n1 = operators.len() as f32;
    let n2 = operands.len() as f32;
    let big_n1: f32 = operators.values().map(|&c| c as f32).sum();
    let big_n2: f32 = operands.values().map(|&c| c as f32).sum();
    let vocab = n1 + n2;
    let length = big_n1 + big_n2;
    let volume = length * (vocab.max(1.0)).log2();
    let difficulty = if n2 == 0.0 { 0.0 } else { (n1 / 2.0) * (big_n2 / n2) };
    let effort = difficulty * volume;
    let raw = [n1, n2, big_n1, big_n2, vocab, length, volume, effort];
    let mut v: Vec<f32> = raw.iter().map(|&x| (x / 50.0).tanh()).collect();
    v.truncate(HALSTEAD_DIM);
    l2_normalize(v)
}

fn is_operator(token: &str) -> bool {
    const KEYWORDS: &[&str] = &[
        "if", "else", "for", "while", "return", "let", "match", "fn", "def", "class", "import",
        "from", "use", "new", "await", "async", "try", "catch", "throw", "switch", "case", "in",
        "of", "not", "and", "or", "is", "lambda", "yield", "with", "as", "elif",
    ];
    if KEYWORDS.contains(&token) {
        return true;
    }
    !token.is_empty() && token.chars().all(|c| !c.is_alphanumeric() && c != '_')
}

// ── shared helpers ───────────────────────────────────────────────────────

/// The token bag of a node: split name + qualified name + docstring.
fn node_tokens(node: &Node) -> Vec<String> {
    let mut tokens = split_ident(&node.name);
    tokens.extend(split_ident(&node.qualified_name));
    tokens.extend(crate::tokenize::tokenize(node.body()));
    tokens
}

fn hash_index(token: &str, dim: usize) -> usize {
    (fnv1a(token.as_bytes()) % dim as u64) as usize
}

fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in bytes {
        hash ^= u64::from(b);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

fn mix(mut x: u64, k: u64) -> u64 {
    x ^= k.wrapping_mul(0x9e37_79b9_7f4a_7c15);
    x = (x ^ (x >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    x ^ (x >> 31)
}

fn l2_normalize(mut v: Vec<f32>) -> Vec<f32> {
    let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > f32::EPSILON {
        for x in &mut v {
            *x /= norm;
        }
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{EdgeType, Label};
    use serde_json::json;

    fn node(id: i64, name: &str, qn: &str, file: &str, doc: &str) -> Node {
        Node {
            id,
            project: "p".into(),
            label: Label::Function,
            name: name.into(),
            qualified_name: qn.into(),
            file_path: file.into(),
            start_line: 1,
            end_line: 5,
            props: json!({ "doc": doc }),
        }
    }

    fn graph_with(nodes: &[Node], edges: &[(i64, i64)]) -> SymbolGraph {
        let store = GraphStore::open_in_memory().expect("store");
        store.register_project("p", "/tmp").expect("project");
        for n in nodes {
            store.upsert_node(n).expect("node");
        }
        for (s, t) in edges {
            store
                .insert_edge("p", *s, *t, EdgeType::Calls, &json!({}))
                .expect("edge");
        }
        SymbolGraph::from_store(&store, "p").expect("graph")
    }

    #[test]
    fn signal_dimensions_are_fixed_and_concat_matches() {
        let nodes = vec![node(1, "parse_config", "m::parse_config", "src/a.rs", "parse it")];
        let graph = graph_with(&nodes, &[]);
        let sig = symbol_signals(&nodes[0], &graph);
        assert_eq!(sig.tfidf.len(), LEX_DIM);
        assert_eq!(sig.random_indexing.len(), RI_DIM);
        assert_eq!(sig.minhash.len(), MINHASH_DIM);
        assert_eq!(sig.api_signature.len(), API_DIM);
        assert_eq!(sig.type_signature.len(), TYPE_DIM);
        assert_eq!(sig.module_proximity.len(), PROX_DIM);
        assert_eq!(sig.decorators.len(), DECOR_DIM);
        assert_eq!(sig.ast_profile.len(), AST_DIM);
        assert_eq!(sig.dataflow.len(), DATAFLOW_DIM);
        assert_eq!(sig.graph_diffusion.len(), LEX_DIM);
        assert_eq!(sig.halstead.len(), HALSTEAD_DIM);
        assert_eq!(sig.concat().len(), SignalVector::dim());
    }

    #[test]
    fn identical_symbols_are_most_similar() {
        let a = node(1, "parse_config", "m::parse_config", "src/a.rs", "parse config");
        let b = node(2, "parse_config", "m::parse_config", "src/b.rs", "parse config");
        let c = node(3, "render_widget", "m::render_widget", "src/z.rs", "draw");
        let graph = graph_with(&[a.clone(), b.clone(), c.clone()], &[]);
        let sa = symbol_signals(&a, &graph);
        let sb = symbol_signals(&b, &graph);
        let sc = symbol_signals(&c, &graph);
        assert!(
            sa.similarity(&sb) > sa.similarity(&sc),
            "same-name symbol must be closer: {} vs {}",
            sa.similarity(&sb),
            sa.similarity(&sc)
        );
    }

    #[test]
    fn module_proximity_rewards_same_directory() {
        let near = module_proximity_vector("src/graph/store.rs");
        let same_dir = module_proximity_vector("src/graph/index.rs");
        let far = module_proximity_vector("tests/integration.rs");
        let a = crate::hybrid::cosine(&near, &same_dir);
        let b = crate::hybrid::cosine(&near, &far);
        assert!(a > b, "same directory must be closer: {a} vs {b}");
    }

    #[test]
    fn api_signature_captures_shared_callees() {
        // 1 and 2 both call 3; 4 calls nobody.
        let nodes = vec![
            node(1, "alpha", "m::alpha", "a.rs", ""),
            node(2, "beta", "m::beta", "b.rs", ""),
            node(3, "helper", "m::helper", "h.rs", ""),
            node(4, "loner", "m::loner", "l.rs", ""),
        ];
        let graph = graph_with(&nodes, &[(1, 3), (2, 3)]);
        let s1 = symbol_signals(&nodes[0], &graph);
        let s2 = symbol_signals(&nodes[1], &graph);
        let s4 = symbol_signals(&nodes[3], &graph);
        assert!(
            s1.similarity(&s2) > s1.similarity(&s4),
            "shared callee must dominate"
        );
    }

    #[test]
    fn signals_are_deterministic() {
        let nodes = vec![node(1, "parse_config", "m::parse_config", "src/a.rs", "parse")];
        let graph = graph_with(&nodes, &[]);
        let a = symbol_signals(&nodes[0], &graph);
        let b = symbol_signals(&nodes[0], &graph);
        assert_eq!(a, b);
    }

    #[test]
    fn halstead_separates_operators_and_operands() {
        assert!(is_operator("if"));
        assert!(is_operator("+"));
        assert!(!is_operator("parse_config"));
        let tokens: Vec<String> = ["if", "x", ">", "0", "return", "x"]
            .iter()
            .map(|s| (*s).to_string())
            .collect();
        let v = halstead_vector(&tokens);
        assert_eq!(v.len(), HALSTEAD_DIM);
        assert!(v.iter().any(|&x| x > 0.0));
    }
}
