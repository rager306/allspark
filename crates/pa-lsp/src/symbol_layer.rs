//! Symbol layer over tree-sitter (`pa-ast`): the Serena-style name-path tree.
//!
//! FORK-ONLY (allspark). Where [`crate::symbols`] matches name paths over an
//! LSP `documentSymbol` tree, this module *builds* an equivalent tree straight
//! from the syntax tree with `pa-ast`/tree-sitter — no language server needed.
//! That is what makes `get_symbols_overview` (S9), `find_symbol` (S10),
//! `replace_symbol_body`/`insert_*_symbol` (S13), and `safe_delete_symbol`
//! (S14) work even for a file no server has opened, and it gives byte-exact
//! ranges for edits.
//!
//! A symbol is a named declaration node. Containers (class/struct/impl/trait/
//! enum/module) carry their members as children; the walk descends only through
//! container member lists, so local variables inside a function body are not
//! mistaken for symbols. The resulting tree flattens into the very same
//! [`crate::symbols::FlatSymbol`] shape the LSP path uses, so name-path
//! resolution is shared.

use std::path::Path;

use lsp_types::{Range, SymbolKind};
use pa_ast::language::SupportLang;
use pa_ast::parse_cache::parse_cached;
use tree_sitter::Node;

use crate::symbols::{self, FlatSymbol};
use crate::text::{range_from_bytes, LineIndex};

/// One node in the syntax-derived symbol tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SymbolNode {
    /// The symbol's own name (leaf of its name path).
    pub name: String,
    /// The protocol symbol kind.
    pub kind: SymbolKind,
    /// The full name path from the file root.
    pub path: Vec<String>,
    /// The overload index when several siblings share `name` (0-based).
    pub overload_idx: Option<usize>,
    /// The name's own range (what a rename points at).
    pub selection_range: Range,
    /// The whole declaration's range (signature + body).
    pub range: Range,
    /// Depth from the file root (top-level symbols are 0).
    pub depth: usize,
    /// Member symbols, in source order.
    pub children: Vec<SymbolNode>,
}

impl SymbolNode {
    /// The `Class/method` name path.
    #[must_use]
    pub fn name_path(&self) -> String {
        symbols::FlatSymbol {
            name: self.name.clone(),
            kind: self.kind,
            path: self.path.clone(),
            overload_idx: self.overload_idx,
            selection_range: self.selection_range,
            range: self.range,
        }
        .name_path()
    }

    /// Convert to the flat, matcher-friendly shape.
    #[must_use]
    pub fn flat(&self) -> FlatSymbol {
        FlatSymbol {
            name: self.name.clone(),
            kind: self.kind,
            path: self.path.clone(),
            overload_idx: self.overload_idx,
            selection_range: self.selection_range,
            range: self.range,
        }
    }
}

/// The result of parsing a file into a symbol tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SymbolTree {
    /// Canonical language name when the file parsed.
    pub language: Option<String>,
    /// Whether tree-sitter parsed the source without errors.
    pub parsed: bool,
    /// Top-level symbols.
    pub roots: Vec<SymbolNode>,
}

impl SymbolTree {
    /// Flatten the whole tree into matcher-friendly symbols, in document order.
    #[must_use]
    pub fn flatten(&self) -> Vec<FlatSymbol> {
        let mut out = Vec::new();
        for root in &self.roots {
            flatten_into(root, &mut out);
        }
        out
    }

    /// The tree trimmed to `depth` levels of children (`0` = top level only,
    /// `None` = unlimited). Used by the progressive-shortening renderer.
    #[must_use]
    pub fn trimmed(&self, depth: Option<u32>) -> Vec<SymbolNode> {
        self.roots
            .iter()
            .map(|node| trim_node(node, depth))
            .collect()
    }
}

fn trim_node(node: &SymbolNode, depth: Option<u32>) -> SymbolNode {
    let children = match depth {
        Some(0) => Vec::new(),
        Some(d) => node
            .children
            .iter()
            .map(|child| trim_node(child, Some(d - 1)))
            .collect(),
        None => node
            .children
            .iter()
            .map(|child| trim_node(child, None))
            .collect(),
    };
    SymbolNode {
        children,
        ..node.clone()
    }
}

fn flatten_into(node: &SymbolNode, out: &mut Vec<FlatSymbol>) {
    out.push(node.flat());
    for child in &node.children {
        flatten_into(child, out);
    }
}

/// Parse `text` (with `path` naming the language) into a symbol tree.
///
/// Returns `parsed=false` and no symbols when the language is unknown, the
/// grammar is unavailable, or tree-sitter reports a syntax error — the same
/// conservative stance `pa-ast` takes for summaries.
#[must_use]
pub fn symbol_tree(text: &str, path: Option<&Path>) -> SymbolTree {
    let Some(language) = path.and_then(SupportLang::from_path) else {
        return SymbolTree {
            language: None,
            parsed: false,
            roots: Vec::new(),
        };
    };
    let Ok(Some(tree)) = parse_cached(text, language) else {
        return SymbolTree {
            language: Some(language.canonical_name().to_string()),
            parsed: false,
            roots: Vec::new(),
        };
    };
    let root = tree.root_node();
    if root.has_error() {
        return SymbolTree {
            language: Some(language.canonical_name().to_string()),
            parsed: false,
            roots: Vec::new(),
        };
    }
    let index = LineIndex::new(text);
    let mut raw = Vec::new();
    collect_raw(root, text, &index, &[], None, &mut raw);
    let roots = assemble_tree(raw);
    SymbolTree {
        language: Some(language.canonical_name().to_string()),
        parsed: true,
        roots,
    }
}

/// A declaration found during the walk, before the tree is assembled.
#[derive(Debug, Clone)]
struct RawSymbol {
    name: String,
    kind: SymbolKind,
    path: Vec<String>,
    selection_range: Range,
    range: Range,
}

/// Collect the symbol declarations reachable from `node` (a file root or a
/// container), in source order.
///
/// `impl` blocks are transparent: their members are lifted to the type they
/// implement, so `impl Foo { fn bar }` contributes `Foo/bar` instead of a
/// second `Foo` symbol. This is what lets a struct and its impls merge into one
/// name path.
fn collect_raw(
    node: Node<'_>,
    text: &str,
    index: &LineIndex,
    prefix: &[String],
    container_kind: Option<&str>,
    out: &mut Vec<RawSymbol>,
) {
    for member in member_nodes(node) {
        if is_transparent_impl(member.kind()) {
            let Some(impl_name) = member
                .child_by_field_name("type")
                .and_then(|node| node.utf8_text(text.as_bytes()).ok())
                .map(str::trim)
                .filter(|name| !name.is_empty())
            else {
                continue;
            };
            let mut impl_prefix = prefix.to_vec();
            impl_prefix.push(impl_name.to_string());
            collect_raw(member, text, index, &impl_prefix, Some("impl_item"), out);
            continue;
        }
        // A Go method's receiver names its owning type: nest `Area` under
        // `Shape` rather than leaving it top-level.
        if member.kind() == "method_declaration" {
            if let Some(receiver) = go_receiver_type_name(member, text) {
                let Some(kind) = symbol_kind(member.kind(), Some("impl_item")) else {
                    continue;
                };
                let Some(name_node) = member.child_by_field_name("name") else {
                    continue;
                };
                let name = name_node
                    .utf8_text(text.as_bytes())
                    .unwrap_or("")
                    .trim()
                    .to_string();
                if name.is_empty() {
                    continue;
                }
                let mut path = prefix.to_vec();
                path.push(receiver);
                path.push(name.clone());
                out.push(RawSymbol {
                    name,
                    kind,
                    path,
                    selection_range: node_range(name_node, text, index),
                    range: node_range(member, text, index),
                });
                continue;
            }
        }

        let Some(kind) = symbol_kind(member.kind(), container_kind) else {
            continue;
        };
        let Some(name_node) = name_node(member) else {
            continue;
        };
        let name = name_node
            .utf8_text(text.as_bytes())
            .unwrap_or("")
            .trim()
            .to_string();
        if name.is_empty() {
            continue;
        }
        let mut path = prefix.to_vec();
        path.push(name.clone());
        out.push(RawSymbol {
            name,
            kind,
            path: path.clone(),
            selection_range: node_range(name_node, text, index),
            range: node_range(member, text, index),
        });
        if is_container(member) {
            collect_raw(member, text, index, &path, Some(member.kind()), out);
        }
    }
}

/// An interned node record: children are held as indices until the tree is
/// materialized, so a parent can be filled in after its children exist.
#[derive(Debug)]
struct Record {
    name: String,
    kind: SymbolKind,
    path: Vec<String>,
    overload_idx: Option<usize>,
    selection_range: Range,
    range: Range,
    children: Vec<usize>,
    /// A synthesized ancestor for an `impl` with no matching type declaration;
    /// a later real declaration with the same path upgrades it in place.
    placeholder: bool,
}

/// Assemble a tree from the flat, path-carrying declarations. Missing
/// intermediate paths (e.g. a bare `impl Foo` with no `struct Foo`) are
/// synthesized as class nodes so the name path still resolves.
fn assemble_tree(raws: Vec<RawSymbol>) -> Vec<SymbolNode> {
    use std::collections::HashMap;

    // Count (parent path, name, kind) to assign overload indices. The kind is
    // keyed by its stable name because `SymbolKind` is not `Hash`.
    let mut totals: HashMap<(Vec<String>, String, &'static str), usize> = HashMap::new();
    for raw in &raws {
        let parent = raw.path[..raw.path.len() - 1].to_vec();
        *totals
            .entry((parent, raw.name.clone(), kind_name(raw.kind)))
            .or_insert(0) += 1;
    }

    let mut records: Vec<Record> = Vec::with_capacity(raws.len());
    let mut index_by_path: HashMap<Vec<String>, usize> = HashMap::new();
    let mut roots: Vec<usize> = Vec::new();
    let mut seen: HashMap<(Vec<String>, String, &'static str), usize> = HashMap::new();

    for raw in raws {
        // Ensure every ancestor exists so the child can be attached.
        for depth in 1..raw.path.len() {
            let ancestor = raw.path[..depth].to_vec();
            if index_by_path.contains_key(&ancestor) {
                continue;
            }
            let idx = records.len();
            records.push(Record {
                name: ancestor.last().cloned().unwrap_or_default(),
                kind: SymbolKind::CLASS,
                path: ancestor.clone(),
                overload_idx: None,
                selection_range: raw.selection_range,
                range: raw.range,
                children: Vec::new(),
                placeholder: true,
            });
            index_by_path.insert(ancestor.clone(), idx);
            if depth == 1 {
                roots.push(idx);
            } else {
                let parent_path = raw.path[..depth - 1].to_vec();
                let parent = index_by_path[&parent_path];
                records[parent].children.push(idx);
            }
        }

        let parent_path = raw.path[..raw.path.len() - 1].to_vec();
        let key = (parent_path.clone(), raw.name.clone(), kind_name(raw.kind));
        let idx = seen.entry(key.clone()).or_insert(0);
        let overload_idx = (totals[&key] > 1).then_some(*idx);
        *idx += 1;

        // If an earlier `impl` synthesized a placeholder for this exact path,
        // upgrade it in place instead of creating a duplicate root. A *real*
        // duplicate (two same-named siblings) is not a placeholder, so it falls
        // through and gets its own record plus an overload index.
        if let Some(&existing) = index_by_path.get(&raw.path) {
            if records[existing].placeholder {
                records[existing].name = raw.name;
                records[existing].kind = raw.kind;
                records[existing].overload_idx = overload_idx;
                records[existing].selection_range = raw.selection_range;
                records[existing].range = raw.range;
                records[existing].placeholder = false;
                continue;
            }
        }

        let node_idx = records.len();
        records.push(Record {
            name: raw.name,
            kind: raw.kind,
            path: raw.path.clone(),
            overload_idx,
            selection_range: raw.selection_range,
            range: raw.range,
            children: Vec::new(),
            placeholder: false,
        });
        index_by_path.insert(raw.path.clone(), node_idx);
        if parent_path.is_empty() {
            roots.push(node_idx);
        } else if let Some(&parent) = index_by_path.get(&parent_path) {
            records[parent].children.push(node_idx);
        } else {
            roots.push(node_idx);
        }
    }

    roots
        .into_iter()
        .map(|idx| build_node(idx, &records))
        .collect()
}

/// Materialize a record (and its subtree) into a [`SymbolNode`].
fn build_node(idx: usize, records: &[Record]) -> SymbolNode {
    let record = &records[idx];
    let children = record
        .children
        .iter()
        .map(|&child| build_node(child, records))
        .collect();
    SymbolNode {
        name: record.name.clone(),
        kind: record.kind,
        path: record.path.clone(),
        overload_idx: record.overload_idx,
        selection_range: record.selection_range,
        range: record.range,
        depth: record.path.len() - 1,
        children,
    }
}

/// The bare type name from a Go method's receiver, e.g. `Shape` from
/// `func (s Shape) Area()` or `func (s *Shape) Area()`.
fn go_receiver_type_name(method: Node<'_>, text: &str) -> Option<String> {
    let receiver = method.child_by_field_name("receiver")?;
    let param = receiver.named_child(0)?;
    let type_node = param.child_by_field_name("type")?;
    let raw = type_node.utf8_text(text.as_bytes()).ok()?.trim();
    // Strip a pointer/qualified prefix: `*Shape`, `pkg.Shape` -> `Shape`.
    let bare = raw
        .trim_start_matches(['*', '&'])
        .rsplit('.')
        .next()
        .unwrap_or(raw)
        .trim();
    (!bare.is_empty()).then(|| bare.to_string())
}

/// Whether a member is an `impl` block: a *transparent* container whose members
/// belong to the type it implements.
fn is_transparent_impl(kind: &str) -> bool {
    kind == "impl_item"
}

/// The symbol-bearing members directly inside `node`.
fn member_nodes(node: Node<'_>) -> Vec<Node<'_>> {
    // Containers expose their members through a field; a plain file root lists
    // its declarations as direct children.
    for field in ["body", "type", "declaration"] {
        if let Some(child) = node.child_by_field_name(field) {
            if is_wrapper_list(child.kind()) {
                return list_children(child);
            }
        }
    }
    list_children(node)
}

/// Named children of `node`, descending through transparent wrappers so that
/// `export_statement`/`decorated_definition`/`type_declaration` do not hide the
/// declaration they wrap.
fn list_children(node: Node<'_>) -> Vec<Node<'_>> {
    let mut out = Vec::new();
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if is_wrapper_list(child.kind()) {
            out.extend(list_children(child));
        } else {
            out.push(child);
        }
    }
    out
}

/// Structural nodes that only group members (bodies/lists/type wrappers).
fn is_wrapper_list(kind: &str) -> bool {
    matches!(
        kind,
        "declaration_list"
            | "class_body"
            | "field_declaration_list"
            | "enum_variant_list"
            | "interface_body"
            | "block"
            | "struct_type"
            | "interface_type"
            | "export_statement"
            | "decorated_definition"
            | "type_declaration"
            | "var_declaration"
            | "lexical_declaration"
            | "variable_declaration"
    )
}

/// The node carrying a symbol's name (usually the `name` field; `impl` uses its
/// type).
fn name_node(node: Node<'_>) -> Option<Node<'_>> {
    node.child_by_field_name("name")
        .or_else(|| node.child_by_field_name("type").filter(|_| node.kind() == "impl_item"))
        .or_else(|| node.child_by_field_name("declarator"))
}

/// The protocol kind for a declaration node, or `None` when it is not a symbol.
///
/// `container_kind` is the enclosing symbol node's kind: a function inside a
/// class/impl/trait is a METHOD, while a free function is a FUNCTION. Some
/// grammars (Rust, C++) spell both with the same node kind, so the context is
/// what disambiguates.
fn symbol_kind(kind: &str, container_kind: Option<&str>) -> Option<SymbolKind> {
    let in_type_container = matches!(
        container_kind,
        Some(
            "impl_item"
                | "trait_item"
                | "class_body"
                | "class_declaration"
                | "class_definition"
                | "class_specifier"
                | "abstract_class_declaration"
                | "interface_body"
                | "interface_declaration"
                | "object_declaration"
        )
    );
    Some(match kind {
        "struct_item" | "struct_specifier" => SymbolKind::STRUCT,
        "class_definition" | "class_declaration" | "class_specifier"
        | "abstract_class_declaration" | "type_alias_declaration" | "type_item"
        | "type_spec" | "impl_item" => SymbolKind::CLASS,
        "trait_item" | "interface_declaration" | "interface_type" => SymbolKind::INTERFACE,
        "enum_item" | "enum_declaration" | "enum_specifier" => SymbolKind::ENUM,
        "mod_item" | "module" => SymbolKind::MODULE,
        "namespace_definition" => SymbolKind::NAMESPACE,
        "object_declaration" => SymbolKind::OBJECT,
        "function_item" | "function_definition" | "function_declaration" if in_type_container => {
            SymbolKind::METHOD
        }
        "function_item" | "function_definition" | "function_declaration" | "macro_definition" => {
            SymbolKind::FUNCTION
        }
        "method_definition" | "method_declaration" | "function_signature_item" => SymbolKind::METHOD,
        "constructor_declaration" => SymbolKind::CONSTRUCTOR,
        "const_item" | "static_item" | "const_declaration" | "const_spec" => SymbolKind::CONSTANT,
        "field_declaration" | "field_spec" => SymbolKind::FIELD,
        "property_signature" | "property_declaration" | "public_field_definition" => {
            SymbolKind::PROPERTY
        }
        "enum_variant" | "enum_variant_declaration" => SymbolKind::ENUM_MEMBER,
        "variable_declarator" | "var_spec" | "let_declaration" => SymbolKind::VARIABLE,
        _ => return None,
    })
}

/// Whether a symbol node can hold nested members.
fn is_container(node: Node<'_>) -> bool {
    matches!(
        node.kind(),
        "struct_item"
            | "class_definition"
            | "class_declaration"
            | "class_specifier"
            | "abstract_class_declaration"
            | "impl_item"
            | "trait_item"
            | "interface_declaration"
            | "enum_item"
            | "enum_declaration"
            | "mod_item"
            | "module"
            | "namespace_definition"
            | "type_spec"
            | "type_declaration"
            | "object_declaration"
    )
}

/// An LSP range from a tree-sitter node. tree-sitter columns are byte columns,
/// so the offsets are converted to UTF-16 LSP positions through the line index.
fn node_range(node: Node<'_>, text: &str, index: &LineIndex) -> Range {
    range_from_bytes(text, index, node.start_byte(), node.end_byte())
}

/// Resolve a name-path pattern to the matching symbols in `tree`. An empty
/// pattern matches every symbol.
#[must_use]
pub fn find(tree: &SymbolTree, pattern: &str, substring: bool) -> Vec<FlatSymbol> {
    let flat = tree.flatten();
    if pattern.trim().is_empty() {
        return flat;
    }
    symbols::resolve(&flat, pattern, substring)
}

/// Filter flat symbols by protocol kinds (empty = keep all).
#[must_use]
pub fn filter_kinds(symbols: &[FlatSymbol], kinds: &[SymbolKind]) -> Vec<FlatSymbol> {
    if kinds.is_empty() {
        return symbols.to_vec();
    }
    symbols
        .iter()
        .filter(|symbol| kinds.contains(&symbol.kind))
        .cloned()
        .collect()
}

/// A single byte-exact symbol edit over the file text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SymbolEdit {
    /// The byte range replaced (or the insertion point when `start == end`).
    pub start_byte: usize,
    /// The exclusive end byte.
    pub end_byte: usize,
    /// The replacement text.
    pub new_text: String,
}

impl SymbolEdit {
    /// Apply the edit to `text`.
    #[must_use]
    pub fn apply(&self, text: &str) -> String {
        let mut out = String::with_capacity(text.len() + self.new_text.len());
        out.push_str(&text[..self.start_byte]);
        out.push_str(&self.new_text);
        out.push_str(&text[self.end_byte..]);
        out
    }
}

/// Resolve a symbol's byte range `[start, end)` by name path.
///
/// # Errors
/// Fails when no symbol matches, or the match is ambiguous.
pub fn symbol_bytes(
    text: &str,
    path: Option<&Path>,
    pattern: &str,
) -> anyhow::Result<(usize, usize)> {
    let tree = symbol_tree(text, path);
    anyhow::ensure!(tree.parsed, "the file did not parse into a symbol tree");
    let matches = find(&tree, pattern, false);
    match matches.as_slice() {
        [] => anyhow::bail!("no symbol matches `{pattern}`"),
        [one] => {
            let index = LineIndex::new(text);
            Ok(crate::text::byte_range(text, &index, &one.range))
        }
        many => {
            let names: Vec<String> = many.iter().map(FlatSymbol::name_path).collect();
            anyhow::bail!(
                "`{pattern}` is ambiguous ({} matches: {})",
                many.len(),
                names.join(", ")
            )
        }
    }
}

/// Build an edit replacing the whole symbol (signature + body) named by
/// `pattern` with `new_body`.
///
/// # Errors
/// Fails when the symbol cannot be resolved.
pub fn replace_body_edit(
    text: &str,
    path: Option<&Path>,
    pattern: &str,
    new_body: &str,
) -> anyhow::Result<SymbolEdit> {
    let (start, end) = symbol_bytes(text, path, pattern)?;
    Ok(SymbolEdit {
        start_byte: start,
        end_byte: end,
        new_text: new_body.to_string(),
    })
}

/// Build an edit inserting `content` immediately before the symbol.
///
/// # Errors
/// Fails when the symbol cannot be resolved.
pub fn insert_before_edit(
    text: &str,
    path: Option<&Path>,
    pattern: &str,
    content: &str,
) -> anyhow::Result<SymbolEdit> {
    let (start, _) = symbol_bytes(text, path, pattern)?;
    // Anchor the insert at the start of the symbol's line so indentation of the
    // following declaration is preserved.
    let line_start = text[..start].rfind('\n').map_or(0, |pos| pos + 1);
    Ok(SymbolEdit {
        start_byte: line_start,
        end_byte: line_start,
        new_text: format!("{content}\n"),
    })
}

/// Build an edit inserting `content` immediately after the symbol.
///
/// # Errors
/// Fails when the symbol cannot be resolved.
pub fn insert_after_edit(
    text: &str,
    path: Option<&Path>,
    pattern: &str,
    content: &str,
) -> anyhow::Result<SymbolEdit> {
    let (_, end) = symbol_bytes(text, path, pattern)?;
    Ok(SymbolEdit {
        start_byte: end,
        end_byte: end,
        new_text: format!("\n{content}"),
    })
}

/// Build an edit deleting the symbol, including the trailing blank line its
/// removal would otherwise leave behind.
///
/// # Errors
/// Fails when the symbol cannot be resolved.
pub fn delete_edit(text: &str, path: Option<&Path>, pattern: &str) -> anyhow::Result<SymbolEdit> {
    let (start, end) = symbol_bytes(text, path, pattern)?;
    let mut line_start = text[..start].rfind('\n').map_or(0, |pos| pos + 1);
    let mut line_end = end;
    if text[line_end..].starts_with('\n') {
        line_end += 1;
    }
    // Also swallow leading blank lines above the declaration.
    let mut scan = line_start;
    while scan > 0 {
        let prev_end = scan - 1;
        let prev_start = text[..prev_end].rfind('\n').map_or(0, |pos| pos + 1);
        if text[prev_start..prev_end].trim().is_empty() {
            line_start = prev_start;
            scan = prev_start;
        } else {
            break;
        }
    }
    Ok(SymbolEdit {
        start_byte: line_start,
        end_byte: line_end,
        new_text: String::new(),
    })
}

/// The 1-based line ranges covered by import/use declarations in `text`
/// (S11 `include_imports=false`).
///
/// A "reference" that is really an `import`/`use` is not a semantic use of the
/// symbol, so `find_referencing_symbols` drops those by default. The ranges are
/// 1-based to match reference locations.
#[must_use]
pub fn import_line_ranges(text: &str, path: Option<&Path>) -> Vec<(u32, u32)> {
    let Some(language) = path.and_then(SupportLang::from_path) else {
        return Vec::new();
    };
    let Ok(Some(tree)) = parse_cached(text, language) else {
        return Vec::new();
    };
    let root = tree.root_node();
    let mut out = Vec::new();
    let mut cursor = root.walk();
    for node in root.named_children(&mut cursor) {
        if is_import_kind(node.kind()) {
            out.push((
                u32::try_from(node.start_position().row).unwrap_or(u32::MAX) + 1,
                u32::try_from(node.end_position().row).unwrap_or(u32::MAX) + 1,
            ));
        }
    }
    out
}

/// Declaration kinds that import a name rather than use it.
fn is_import_kind(kind: &str) -> bool {
    matches!(
        kind,
        "use_declaration"
            | "import_statement"
            | "import_declaration"
            | "import_from_statement"
            | "using_declaration"
            | "import_spec"
            | "import_spec_list"
            | "preproc_include"
            | "package_clause"
    )
}

/// Whether `line` (1-based) falls inside one of `ranges`.
#[must_use]
pub fn line_in_ranges(line: u32, ranges: &[(u32, u32)]) -> bool {
    ranges.iter().any(|(start, end)| line >= *start && line <= *end)
}

/// The protocol kind named by `name` (case-insensitive), for `kinds` filters.
#[must_use]
pub fn kind_from_name(name: &str) -> Option<SymbolKind> {
    let lowered = name.trim().to_ascii_lowercase();
    Some(match lowered.as_str() {
        "file" => SymbolKind::FILE,
        "module" => SymbolKind::MODULE,
        "namespace" => SymbolKind::NAMESPACE,
        "package" => SymbolKind::PACKAGE,
        "class" => SymbolKind::CLASS,
        "method" => SymbolKind::METHOD,
        "property" => SymbolKind::PROPERTY,
        "field" => SymbolKind::FIELD,
        "constructor" => SymbolKind::CONSTRUCTOR,
        "enum" => SymbolKind::ENUM,
        "interface" => SymbolKind::INTERFACE,
        "function" => SymbolKind::FUNCTION,
        "variable" => SymbolKind::VARIABLE,
        "constant" => SymbolKind::CONSTANT,
        "struct" => SymbolKind::STRUCT,
        "event" => SymbolKind::EVENT,
        "operator" => SymbolKind::OPERATOR,
        "type_parameter" => SymbolKind::TYPE_PARAMETER,
        "enum_member" => SymbolKind::ENUM_MEMBER,
        "object" => SymbolKind::OBJECT,
        "key" => SymbolKind::KEY,
        "array" => SymbolKind::ARRAY,
        "boolean" => SymbolKind::BOOLEAN,
        "number" => SymbolKind::NUMBER,
        "string" => SymbolKind::STRING,
        "null" => SymbolKind::NULL,
        _ => return None,
    })
}

/// The lowercase name of a protocol kind, for output.
#[must_use]
pub fn kind_name(kind: SymbolKind) -> &'static str {
    match kind {
        SymbolKind::FILE => "file",
        SymbolKind::MODULE => "module",
        SymbolKind::NAMESPACE => "namespace",
        SymbolKind::PACKAGE => "package",
        SymbolKind::CLASS => "class",
        SymbolKind::METHOD => "method",
        SymbolKind::PROPERTY => "property",
        SymbolKind::FIELD => "field",
        SymbolKind::CONSTRUCTOR => "constructor",
        SymbolKind::ENUM => "enum",
        SymbolKind::INTERFACE => "interface",
        SymbolKind::FUNCTION => "function",
        SymbolKind::VARIABLE => "variable",
        SymbolKind::CONSTANT => "constant",
        SymbolKind::STRING => "string",
        SymbolKind::NUMBER => "number",
        SymbolKind::BOOLEAN => "boolean",
        SymbolKind::ARRAY => "array",
        SymbolKind::OBJECT => "object",
        SymbolKind::KEY => "key",
        SymbolKind::NULL => "null",
        SymbolKind::ENUM_MEMBER => "enum_member",
        SymbolKind::STRUCT => "struct",
        SymbolKind::EVENT => "event",
        SymbolKind::OPERATOR => "operator",
        SymbolKind::TYPE_PARAMETER => "type_parameter",
        _ => "unknown",
    }
}

/// Parse a comma/space separated `kinds` argument into protocol kinds.
#[must_use]
pub fn parse_kinds(raw: &str) -> Vec<SymbolKind> {
    raw.split([',', ' '])
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .filter_map(kind_from_name)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn rust_path() -> PathBuf {
        PathBuf::from("sample.rs")
    }

    const RUST: &str = "use std::fmt;\n\
pub struct Foo {\n    x: i32,\n}\n\
impl Foo {\n    pub fn bar(&self) -> i32 {\n        self.x\n    }\n    fn baz(&self) {}\n}\n\
pub fn top() {}\n\
const C: i32 = 1;\n\
enum E {\n    A,\n    B,\n}\n";

    #[test]
    fn overview_finds_top_level_symbols() {
        let tree = symbol_tree(RUST, Some(&rust_path()));
        assert!(tree.parsed);
        let roots: Vec<String> = tree.roots.iter().map(SymbolNode::name_path).collect();
        assert!(roots.contains(&"Foo".to_string()), "{roots:?}");
        assert!(roots.contains(&"top".to_string()), "{roots:?}");
        assert!(roots.contains(&"C".to_string()), "{roots:?}");
        // Impl methods nest under their type, not as top-level roots.
        let all: Vec<String> = tree.flatten().iter().map(FlatSymbol::name_path).collect();
        assert!(all.contains(&"Foo/bar".to_string()), "{all:?}");
        assert!(all.contains(&"Foo/baz".to_string()), "{all:?}");
        // `use` statements are not symbols.
        assert!(!all.iter().any(|n| n.contains("std")));
    }

    #[test]
    fn impl_methods_become_methods_not_functions() {
        let tree = symbol_tree(RUST, Some(&rust_path()));
        let flat = tree.flatten();
        let bar = flat.iter().find(|s| s.name_path() == "Foo/bar").unwrap();
        assert_eq!(bar.kind, SymbolKind::METHOD);
        let top = flat.iter().find(|s| s.name_path() == "top").unwrap();
        assert_eq!(top.kind, SymbolKind::FUNCTION);
    }

    #[test]
    fn find_resolves_relative_and_absolute_paths() {
        let tree = symbol_tree(RUST, Some(&rust_path()));
        assert_eq!(find(&tree, "Foo/bar", false).len(), 1);
        assert_eq!(find(&tree, "/Foo/bar", false).len(), 1);
        assert_eq!(find(&tree, "bar", false).len(), 1);
        assert_eq!(find(&tree, "/bar", false).len(), 0);
    }

    #[test]
    fn substring_matching_finds_partial_names() {
        let tree = symbol_tree(RUST, Some(&rust_path()));
        // "ar" is a substring of "bar" only.
        assert_eq!(find(&tree, "ar", true).len(), 1);
        assert_eq!(find(&tree, "ar", false).len(), 0);
    }

    #[test]
    fn kinds_filter_narrows_results() {
        let tree = symbol_tree(RUST, Some(&rust_path()));
        let flat = tree.flatten();
        let functions = filter_kinds(&flat, &[SymbolKind::FUNCTION]);
        assert_eq!(functions.len(), 1);
        assert_eq!(functions[0].name_path(), "top");
        let methods = filter_kinds(&flat, &[SymbolKind::METHOD]);
        assert_eq!(methods.len(), 2);
    }

    #[test]
    fn replace_body_edit_is_byte_exact() {
        let edit = replace_body_edit(RUST, Some(&rust_path()), "top", "pub fn top() { let x = 1; }").unwrap();
        let out = edit.apply(RUST);
        assert!(out.contains("pub fn top() { let x = 1; }"));
        assert!(!out.contains("pub fn top() {}"));
    }

    #[test]
    fn insert_after_symbol_places_content_below() {
        let edit = insert_after_edit(RUST, Some(&rust_path()), "C", "const D: i32 = 2;").unwrap();
        let out = edit.apply(RUST);
        let c = out.find("const C").unwrap();
        let d = out.find("const D").unwrap();
        assert!(d > c);
    }

    #[test]
    fn delete_symbol_removes_the_declaration() {
        let edit = delete_edit(RUST, Some(&rust_path()), "Foo/baz").unwrap();
        let out = edit.apply(RUST);
        assert!(!out.contains("fn baz"));
        assert!(out.contains("fn bar"));
    }

    #[test]
    fn unknown_extension_yields_an_unparsed_tree() {
        let tree = symbol_tree("hello", Some(Path::new("notes.txt")));
        assert!(!tree.parsed);
        assert_eq!(tree.roots.len(), 0);
    }

    #[test]
    fn python_class_methods_nest_under_the_class() {
        let py = "class Foo:\n    def bar(self):\n        return 1\n    def baz(self):\n        pass\n\ndef top():\n    pass\n";
        let tree = symbol_tree(py, Some(Path::new("m.py")));
        let flat = tree.flatten();
        let paths: Vec<String> = flat.iter().map(FlatSymbol::name_path).collect();
        assert!(paths.contains(&"Foo/bar".to_string()), "{paths:?}");
        assert!(paths.contains(&"top".to_string()), "{paths:?}");
    }

    #[test]
    fn typescript_exported_declarations_are_unwrapped() {
        let ts = "export class Foo {\n  bar() { return 1; }\n}\nexport function top() {}\n";
        let tree = symbol_tree(ts, Some(Path::new("m.ts")));
        let flat = tree.flatten();
        let paths: Vec<String> = flat.iter().map(FlatSymbol::name_path).collect();
        assert!(paths.contains(&"Foo".to_string()), "{paths:?}");
        assert!(paths.contains(&"Foo/bar".to_string()), "{paths:?}");
        assert!(paths.contains(&"top".to_string()), "{paths:?}");
    }

    #[test]
    fn overload_index_disambiguates_same_named_siblings() {
        let ts = "class F {\n  m() {}\n  m() {}\n}\n";
        let tree = symbol_tree(ts, Some(Path::new("m.ts")));
        let flat = tree.flatten();
        let names: Vec<String> = flat
            .iter()
            .filter(|s| s.name == "m")
            .map(FlatSymbol::name_path)
            .collect();
        assert_eq!(names, vec!["F/m[0]".to_string(), "F/m[1]".to_string()]);
    }

    #[test]
    fn trimmed_depth_limits_children() {
        let tree = symbol_tree(RUST, Some(&rust_path()));
        let depth0 = tree.trimmed(Some(0));
        assert!(depth0.iter().all(|s| s.children.is_empty()));
        let depth1 = tree.trimmed(Some(1));
        let foo = depth1.iter().find(|s| s.name == "Foo").unwrap();
        assert_ne!(foo.children.len(), 0);
        assert!(foo.children.iter().all(|c| c.children.is_empty()));
    }

    #[test]
    fn import_lines_are_detected_for_rust_and_python() {
        let rust = "use std::fmt;\npub fn top() {}\n";
        let ranges = import_line_ranges(rust, Some(Path::new("m.rs")));
        assert_eq!(ranges, vec![(1, 1)]);
        assert!(line_in_ranges(1, &ranges));
        assert!(!line_in_ranges(2, &ranges));

        let py = "import os\nfrom a import b\n\ndef top():\n    pass\n";
        let ranges = import_line_ranges(py, Some(Path::new("m.py")));
        assert!(line_in_ranges(1, &ranges));
        assert!(line_in_ranges(2, &ranges));
        assert!(!line_in_ranges(4, &ranges));
    }

    #[test]
    fn import_lines_include_rust_multiline_use() {
        let rust = "use std::{\n    fmt,\n    io,\n};\npub fn top() {}\n";
        let ranges = import_line_ranges(rust, Some(Path::new("m.rs")));
        assert_eq!(ranges, vec![(1, 4)]);
    }

    #[test]
    fn kind_name_round_trips() {
        for name in ["function", "class", "method", "struct", "constant"] {
            let kind = kind_from_name(name).unwrap();
            assert_eq!(kind_name(kind), name);
        }
        assert!(kind_from_name("nonsense").is_none());
        assert_eq!(parse_kinds("function, class"), vec![SymbolKind::FUNCTION, SymbolKind::CLASS]);
    }
}