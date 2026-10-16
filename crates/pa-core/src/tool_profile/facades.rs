//! S3: REPL facades — the host-side method surface a `python_repl` session
//! exposes as attribute objects (`s.lsp.find_symbol(...)`).
//!
//! The full facade bridge is a `host_request` seam that lives in the kernel
//! (the Python side calls back over the host-request channel); this module
//! carries the *registry* of facades and their methods so a profile can
//! filter them (`excluded_apis` / `included_apis`) and a prompt can render
//! their names. The live dispatch is intentionally out of scope here.

use std::collections::{BTreeMap, BTreeSet};

/// One facade: a named namespace of methods.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Facade {
    /// Facade name (`lsp`, `edit`, `fs`, `mem`, `shell`, `cfg`, `ext`, `jb`).
    pub name: String,
    /// The methods the facade exposes.
    pub methods: Vec<FacadeMethod>,
}

/// One facade method.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FacadeMethod {
    /// Method name (`find_symbol`).
    pub name: String,
    /// One-line summary.
    pub summary: String,
    /// Opt-in method: excluded unless explicitly included.
    pub optional: bool,
    /// Beta method.
    pub beta: bool,
    /// The method may edit files.
    pub can_edit: bool,
}

/// The eight default facades (Serena: `cfg`, `fs`, `edit`, `mem`, `shell`,
/// `ext`, `lsp`, `jb`). The `jb` facade is represented by a small
/// JetBrains-only sample; the LSP backend is the primary surface here.
#[must_use]
pub fn default_facades() -> Vec<Facade> {
    const TABLE: &[(&str, &[&str])] = &[
        ("cfg", &["get_config", "set_config"]),
        (
            "fs",
            &[
                "read_file",
                "list_dir",
                "find_file",
                "create_text_file",
                "delete_file",
            ],
        ),
        (
            "edit",
            &[
                "replace_content",
                "replace_in_files",
                "insert_at_line",
                "replace_lines",
                "delete_lines",
                "replace_symbol_body",
                "insert_before_symbol",
                "insert_after_symbol",
            ],
        ),
        (
            "mem",
            &[
                "write_memory",
                "read_memory",
                "list_memories",
                "edit_memory",
                "delete_memory",
                "rename_memory",
                "embed_memory",
            ],
        ),
        ("shell", &["execute_shell_command"]),
        (
            "ext",
            &[
                "list_queryable_projects",
                "query_project",
                "project_context",
            ],
        ),
        (
            "lsp",
            &[
                "find_symbol",
                "find_referencing_symbols",
                "get_symbols_overview",
                "find_implementations",
                "find_declaration",
                "rename_symbol",
                "safe_delete_symbol",
                "replace_symbol_body",
                "insert_before_symbol",
                "insert_after_symbol",
            ],
        ),
        (
            "jb",
            &[
                "jet_brains_find_symbol",
                "jet_brains_move",
                "jet_brains_inline",
            ],
        ),
    ];
    TABLE
        .iter()
        .map(|(name, methods)| Facade {
            name: (*name).to_string(),
            methods: methods
                .iter()
                .map(|method| FacadeMethod {
                    name: (*method).to_string(),
                    summary: String::new(),
                    optional: false,
                    beta: false,
                    can_edit: method.contains("edit")
                        || method.contains("replace")
                        || method.contains("insert")
                        || method.contains("delete")
                        || method.contains("create")
                        || method.contains("rename"),
                })
                .collect(),
        })
        .collect()
}

/// S3: a facade registry with API-scope filtering.
#[derive(Debug, Clone, Default)]
pub struct FacadeRegistry {
    facades: BTreeMap<String, Facade>,
}

impl FacadeRegistry {
    /// A registry with the default facades.
    #[must_use]
    pub fn with_defaults() -> Self {
        let mut registry = Self::default();
        for facade in default_facades() {
            registry.register(facade);
        }
        registry
    }

    /// Register a facade (replacing any prior entry of the same name).
    pub fn register(&mut self, facade: Facade) {
        self.facades.insert(facade.name.clone(), facade);
    }

    /// The facade names.
    #[must_use]
    pub fn names(&self) -> Vec<&str> {
        self.facades.keys().map(String::as_str).collect()
    }

    /// Look up a facade.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&Facade> {
        self.facades.get(name)
    }

    /// Apply an API scope: whole facades in `excluded_apis` are dropped, and
    /// dotted entries (`mem.onboarding`) drop a single method. `included_apis`
    /// re-admits an excluded facade.
    #[must_use]
    pub fn apply_scope(&self, excluded_apis: &[String], included_apis: &[String]) -> Vec<Facade> {
        let included: BTreeSet<&str> = included_apis.iter().map(String::as_str).collect();
        let mut excluded_facades: BTreeSet<&str> = BTreeSet::new();
        let mut excluded_methods: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
        for entry in excluded_apis {
            if let Some((facade, method)) = entry.split_once('.') {
                excluded_methods.entry(facade).or_default().insert(method);
            } else {
                excluded_facades.insert(entry.as_str());
            }
        }
        self.facades
            .values()
            .filter(|facade| {
                !excluded_facades.contains(facade.name.as_str())
                    || included.contains(facade.name.as_str())
            })
            .map(|facade| {
                let dropped = excluded_methods.get(facade.name.as_str());
                let methods = facade
                    .methods
                    .iter()
                    .filter(|method| dropped.is_none_or(|set| !set.contains(method.name.as_str())))
                    .cloned()
                    .collect();
                Facade {
                    name: facade.name.clone(),
                    methods,
                }
            })
            .collect()
    }
}
