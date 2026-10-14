//! Value types shared by the store, indexer, and queries.

use serde::{Deserialize, Serialize};

/// Kind of a graph node. Mirrors the `codebase-memory-mcp` label vocabulary
/// trimmed to what the `pa-ast` extractor emits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Label {
    /// A free function.
    Function,
    /// A method (function inside an impl/class body).
    Method,
    /// A class (Python/TS/Java) or `impl` block owner.
    Class,
    /// A struct / record.
    Struct,
    /// An enum.
    Enum,
    /// A trait / interface.
    Interface,
    /// A module / namespace.
    Module,
    /// A whole source file.
    File,
    /// A directory.
    Folder,
    /// A git branch (provenance node written by `detect_changes`).
    Branch,
}

impl Label {
    /// Stable lowercase wire name stored in the `nodes.label` column.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Function => "Function",
            Self::Method => "Method",
            Self::Class => "Class",
            Self::Struct => "Struct",
            Self::Enum => "Enum",
            Self::Interface => "Interface",
            Self::Module => "Module",
            Self::File => "File",
            Self::Folder => "Folder",
            Self::Branch => "Branch",
        }
    }

    /// Parse a label back from its wire name; unknown names map to [`Label::Function`].
    #[must_use]
    pub fn from_str_or_default(value: &str) -> Self {
        match value {
            "Method" => Self::Method,
            "Class" => Self::Class,
            "Struct" => Self::Struct,
            "Enum" => Self::Enum,
            "Interface" => Self::Interface,
            "Module" => Self::Module,
            "File" => Self::File,
            "Folder" => Self::Folder,
            "Branch" => Self::Branch,
            _ => Self::Function,
        }
    }
}

impl std::fmt::Display for Label {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Kind of a graph edge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum EdgeType {
    /// A resolved call from one symbol to another.
    Calls,
    /// A module import.
    Imports,
    /// Lexical containment (file → symbol, module → symbol).
    Contains,
    /// A structural inheritance / interface implementation.
    Inherits,
    /// A reference that could not be resolved to a definition.
    Uses,
    /// Two files that changed together in the same commit(s) (`detect_changes`).
    FileChangesWith,
    /// A git branch node owns the file nodes it changed (`detect_changes`).
    HasBranch,
}

impl EdgeType {
    /// Stable wire name stored in the `edges.edge_type` column.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Calls => "CALLS",
            Self::Imports => "IMPORTS",
            Self::Contains => "CONTAINS",
            Self::Inherits => "INHERITS",
            Self::Uses => "USAGE",
            Self::FileChangesWith => "FILE_CHANGES_WITH",
            Self::HasBranch => "HAS_BRANCH",
        }
    }

    /// Parse an edge type back from its wire name; unknown names map to [`EdgeType::Uses`].
    #[must_use]
    pub fn from_str_or_default(value: &str) -> Self {
        match value {
            "CALLS" => Self::Calls,
            "IMPORTS" => Self::Imports,
            "CONTAINS" => Self::Contains,
            "INHERITS" => Self::Inherits,
            "FILE_CHANGES_WITH" => Self::FileChangesWith,
            "HAS_BRANCH" => Self::HasBranch,
            _ => Self::Uses,
        }
    }
}

impl std::fmt::Display for EdgeType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A graph node as stored in SQLite.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Node {
    /// Row id (0 before insert).
    pub id: i64,
    /// Project the node belongs to.
    pub project: String,
    /// Node kind.
    pub label: Label,
    /// Short display name (e.g. `open`).
    pub name: String,
    /// Fully-qualified name (e.g. `pa_graph::GraphStore::open`).
    pub qualified_name: String,
    /// Repo-relative path of the defining file.
    pub file_path: String,
    /// 1-based inclusive start line.
    pub start_line: u32,
    /// 1-based inclusive end line.
    pub end_line: u32,
    /// Free-form properties (docstring, signature, ...).
    pub props: serde_json::Value,
}

impl Node {
    /// Read a string property, if present.
    #[must_use]
    pub fn prop_str(&self, key: &str) -> Option<&str> {
        self.props.get(key).and_then(serde_json::Value::as_str)
    }

    /// The node's docstring/body, used as the FTS `body` column.
    #[must_use]
    pub fn body(&self) -> &str {
        self.prop_str("doc").unwrap_or("")
    }
}

/// A graph edge as stored in SQLite.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Edge {
    /// Row id (0 before insert).
    pub id: i64,
    /// Project the edge belongs to.
    pub project: String,
    /// Source node id.
    pub source: i64,
    /// Target node id.
    pub target: i64,
    /// Edge kind.
    pub edge_type: EdgeType,
    /// Free-form properties (confidence, strategy, args, ...).
    pub props: serde_json::Value,
}

/// A candidate call target with the resolution confidence the indexer assigned.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CallCandidate {
    /// Callee qualified name.
    pub callee: String,
    /// Confidence in `[0.0, 1.0]`.
    pub confidence: f32,
    /// Resolution strategy that produced the edge.
    pub strategy: CallStrategy,
    /// Number of same-name candidates considered.
    pub candidates: usize,
    /// Positional call arguments (`i` = index, `e` = expression text).
    pub args: Vec<CallArg>,
    /// 1-based source line of the call site.
    pub line: u32,
}

/// How a call edge was resolved, from most to least certain.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CallStrategy {
    /// The callee was imported in the caller's module under a known name.
    ImportMap,
    /// The callee is defined in the same module/file.
    SameModule,
    /// Exactly one definition with this name exists project-wide.
    UniqueName,
    /// Several definitions share a name suffix; the closest one was chosen.
    Suffix,
}

impl CallStrategy {
    /// Stable wire name stored in the edge properties.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ImportMap => "import_map",
            Self::SameModule => "same_module",
            Self::UniqueName => "unique",
            Self::Suffix => "suffix",
        }
    }

    /// Baseline confidence assigned to this strategy before candidate count
    /// is folded in.
    #[must_use]
    pub const fn base_confidence(self) -> f32 {
        match self {
            Self::ImportMap => 0.95,
            Self::SameModule => 0.90,
            Self::UniqueName => 0.75,
            Self::Suffix => 0.55,
        }
    }
}

/// A positional argument of a call site (data-flow seed).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CallArg {
    /// Zero-based argument index.
    pub i: u32,
    /// Source expression text.
    pub e: String,
}

/// Outcome of a full or incremental index run.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct IndexReport {
    /// Project name.
    pub project: String,
    /// Files whose hashes were new this run.
    pub added: usize,
    /// Files whose hashes changed since the last run.
    pub modified: usize,
    /// Files present in the index but gone from disk.
    pub removed: usize,
    /// Files re-parsed (added + modified).
    pub parsed: usize,
    /// Files whose hashes were unchanged and skipped.
    pub unchanged: usize,
    /// Nodes written.
    pub nodes: usize,
    /// Edges written.
    pub edges: usize,
    /// Files that failed to parse.
    pub failed: usize,
    /// Files indexed despite carrying tree-sitter `ERROR` regions.
    pub partial: usize,
    /// Files whose parse is dominated by one error region (mostly unusable).
    pub unusable: usize,
    /// Files skipped before parsing (unsupported, oversized, unreadable).
    pub skipped: usize,
}

/// Incremental classification of one file against the stored hash.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileChange {
    /// Hash matches the stored row.
    Unchanged,
    /// A stored row exists but the content hash differs.
    Modified,
    /// No stored row for this path.
    Added,
}

impl FileChange {
    /// Whether this classification requires re-parsing.
    #[must_use]
    pub const fn needs_parse(self) -> bool {
        matches!(self, Self::Modified | Self::Added)
    }
}
