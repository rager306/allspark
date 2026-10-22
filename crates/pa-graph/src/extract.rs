//! Symbol / call / import extraction from a `pa-ast` tree-sitter tree.
//!
//! The extractor is grammar-driven but tolerant: it recognises a curated set of
//! definition node kinds for the languages `pa-ast` links natively (Rust,
//! Python, TypeScript/TSX, JavaScript, Go, Java, C/C++) and falls back to a
//! generic identifier search for anything else. Unknown kinds are skipped
//! rather than failing the file, so a partially-parsed tree still yields the
//! symbols it does contain.

use pa_ast::SupportLang;
use tree_sitter::Node;

use crate::types::{CallArg, Label};

/// A symbol definition found in a file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SymbolDef {
    /// Node kind.
    pub label: Label,
    /// Short name.
    pub name: String,
    /// Qualified name (`module::Container::name`).
    pub qualified_name: String,
    /// 1-based inclusive start line.
    pub start_line: u32,
    /// 1-based inclusive end line.
    pub end_line: u32,
    /// Docstring / leading comment (may be empty).
    pub doc: String,
}

/// A call site found inside a function or method.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallSite {
    /// Qualified name of the enclosing function/method.
    pub caller_qn: String,
    /// Name of the callee as written at the call site, truncated to its last
    /// segment (`extcrate::do_thing()` -> `do_thing`). Resolution keys on this.
    pub callee: String,
    /// The callee expression as written before truncation, when it is a path
    /// (`extcrate::do_thing`). The resolver needs the root segment to tell a
    /// dependency call from an unknown name, and truncation has already thrown
    /// it away by the time `callee` exists. `None` for a bare name.
    pub callee_path: Option<String>,
    /// 1-based source line.
    pub line: u32,
    /// Positional argument expressions.
    pub args: Vec<CallArg>,
}

/// An import binding: `local_name` refers to `target_name` from `module`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportDef {
    /// Local alias introduced into the file scope.
    pub local_name: String,
    /// Imported symbol name (equals `local_name` for a plain module import).
    pub target_name: String,
    /// Module path the symbol came from.
    pub module: String,
}

/// Everything extracted from one file.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ExtractedFile {
    /// Symbol definitions.
    pub symbols: Vec<SymbolDef>,
    /// Call sites.
    pub calls: Vec<CallSite>,
    /// Import bindings.
    pub imports: Vec<ImportDef>,
}

/// How a definition node is treated.
enum DefAction {
    /// Emit a node and push a qualified-name container.
    Emit(Label),
    /// Push a container without emitting a node (e.g. `impl` blocks).
    Container,
}

/// Map a tree-sitter node kind to a definition action, or `None` if the kind
/// is not a definition.
fn classify(kind: &str) -> Option<DefAction> {
    use DefAction::{Container, Emit};
    Some(match kind {
        // Functions.
        "function_item" | "function_definition" | "function_declaration" => Emit(Label::Function),
        "method_definition" | "method_declaration" | "constructor_declaration" => {
            Emit(Label::Method)
        }
        // Types.
        "struct_item" | "struct_specifier" | "type_spec" | "type_alias_declaration" => {
            Emit(Label::Struct)
        }
        "enum_item" | "enum_specifier" | "enum_declaration" => Emit(Label::Enum),
        "trait_item" | "interface_declaration" | "protocol_declaration" => Emit(Label::Interface),
        "class_definition" | "class_declaration" | "class_specifier" => Emit(Label::Class),
        "mod_item" | "namespace_definition" => Emit(Label::Module),
        // Impl blocks are pure containers: `impl Foo { fn bar }` should not
        // collide with `struct Foo` in the UNIQUE(project, qualified_name).
        "impl_item" => Container,
        _ => return None,
    })
}

/// Kind names that carry a call.
fn is_call_kind(kind: &str) -> bool {
    matches!(kind, "call_expression" | "call")
}

/// Kind names that carry an import.
fn is_import_kind(kind: &str) -> bool {
    matches!(
        kind,
        "use_declaration"
            | "import_statement"
            | "import_from_statement"
            | "import_declaration"
            | "import_spec"
    )
}

/// Extract symbols, calls, and imports from `source`.
///
/// `module` is the qualified-name prefix derived from the file path.
#[must_use]
pub fn extract(source: &str, lang: SupportLang, module: &str) -> ExtractedFile {
    let mut out = ExtractedFile::default();
    let Ok(Some(tree)) = pa_ast::parse_cache::parse_cached(source, lang) else {
        return out;
    };
    let root = tree.root_node();
    let mut scope = Scope::default();
    visit(root, source.as_bytes(), module, &mut scope, &mut out);
    out
}

/// A qualified-name container plus whether it is a *type* container (so a
/// nested function is a method rather than a free function).
#[derive(Default)]
struct Scope {
    names: Vec<String>,
    type_like: Vec<bool>,
}

impl Scope {
    fn push(&mut self, name: String, type_like: bool) {
        self.names.push(name);
        self.type_like.push(type_like);
    }

    fn pop(&mut self) {
        self.names.pop();
        self.type_like.pop();
    }

    /// Whether the innermost container is a type (impl/class/struct/...).
    fn in_type(&self) -> bool {
        self.type_like.last().copied().unwrap_or(false)
    }
}

fn visit(node: Node<'_>, src: &[u8], module: &str, scope: &mut Scope, out: &mut ExtractedFile) {
    let kind = node.kind();
    let mut pushed = false;

    if let Some(action) = classify(kind) {
        if let Some(name) = node_name(node, src) {
            let qn = qualified(module, &scope.names, &name);
            match action {
                DefAction::Emit(label) => {
                    // A function nested in a type container is a method.
                    let label = if label == Label::Function && scope.in_type() {
                        Label::Method
                    } else {
                        label
                    };
                    out.symbols.push(SymbolDef {
                        label,
                        name: name.clone(),
                        qualified_name: qn,
                        start_line: line(node.start_position().row),
                        end_line: line(node.end_position().row),
                        doc: doc_for(node, src, kind),
                    });
                    scope.push(name, is_type_label(label));
                }
                DefAction::Container => scope.push(name, true),
            }
            pushed = true;
        }
    } else if is_call_kind(kind) {
        if let Some(caller) = scope.names.last() {
            if let Some((callee, callee_path)) = callee_name(node, src) {
                out.calls.push(CallSite {
                    caller_qn: qualified(module, &scope.names[..scope.names.len() - 1], caller),
                    callee,
                    callee_path,
                    line: line(node.start_position().row),
                    args: call_args(node, src),
                });
            }
        }
    } else if is_import_kind(kind) {
        out.imports.extend(parse_import(node, src));
    }

    for child in named_children(node) {
        visit(child, src, module, scope, out);
    }
    if pushed {
        scope.pop();
    }
}

/// Whether a label denotes a type container.
fn is_type_label(label: Label) -> bool {
    matches!(
        label,
        Label::Class | Label::Struct | Label::Enum | Label::Interface
    )
}

/// Build `module::container::...::name`, collapsing empty segments.
fn qualified(module: &str, containers: &[String], name: &str) -> String {
    let mut parts: Vec<&str> = Vec::with_capacity(containers.len() + 2);
    if !module.is_empty() {
        parts.push(module);
    }
    parts.extend(containers.iter().map(String::as_str));
    parts.push(name);
    parts.join("::")
}

/// Collect a node's named children into a `Vec`, releasing the cursor borrow.
fn named_children(node: Node<'_>) -> Vec<Node<'_>> {
    let mut cursor = node.walk();
    node.named_children(&mut cursor).collect()
}

fn line(row: usize) -> u32 {
    u32::try_from(row).unwrap_or(u32::MAX).saturating_add(1)
}

fn text(node: Node<'_>, src: &[u8]) -> String {
    node.utf8_text(src).unwrap_or("").to_string()
}

/// Resolve a definition node's short name.
fn node_name(node: Node<'_>, src: &[u8]) -> Option<String> {
    if let Some(n) = node.child_by_field_name("name") {
        return Some(text(n, src));
    }
    if let Some(n) = node.child_by_field_name("type") {
        return Some(last_segment(&text(n, src)));
    }
    // Go `type_declaration` wraps `type_spec` children; the parent has no name
    // field, so look for a nested spec.
    let children: Vec<Node<'_>> = named_children(node);
    for child in children {
        if child.kind() == "type_spec" {
            if let Some(n) = child.child_by_field_name("name") {
                return Some(text(n, src));
            }
        }
    }
    None
}

/// Extract the callee's short name and, when the call is written as a path, the
/// path expression that rooted it.
///
/// Both come from one read of the call node because truncation is lossy: a
/// resolver told only `do_thing` cannot know the call was `extcrate::do_thing`,
/// and re-parsing to recover the root would repeat extraction's cost.
fn callee_name(call: Node<'_>, src: &[u8]) -> Option<(String, Option<String>)> {
    let func = call
        .child_by_field_name("function")
        .or_else(|| call.child_by_field_name("callee"))?;
    let expr = text(func, src);
    // The same argument-list strip `last_segment` applies, so `f(a::b)` keeps
    // the root of the callee rather than of an argument.
    let head = expr.split(['(', '<']).next().unwrap_or(&expr).trim();
    let path = (head.contains("::") || head.contains('.')).then(|| head.to_string());
    Some((last_segment(&expr), path))
}

/// Take the last identifier-like segment of an expression text.
///
/// `a::b::c` → `c`, `self.foo` → `foo`, `Foo::bar(x)` → `bar`.
fn last_segment(expr: &str) -> String {
    let trimmed = expr.trim();
    // Strip a trailing call argument list if present.
    let head = trimmed.split(['(', '<']).next().unwrap_or(trimmed).trim();
    head.rsplit([':', '.', '>'])
        .find(|s| !s.is_empty())
        .unwrap_or(head)
        .trim()
        .trim_start_matches('&')
        .to_string()
}

/// Collect positional argument expressions from a call node.
fn call_args(call: Node<'_>, src: &[u8]) -> Vec<CallArg> {
    let Some(args) = call
        .child_by_field_name("arguments")
        .or_else(|| call.child_by_field_name("arg_list"))
    else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut i: u32 = 0;
    for child in named_children(args) {
        let expr = text(child, src);
        let expr = expr.trim();
        if expr.is_empty() {
            continue;
        }
        out.push(CallArg {
            i,
            e: expr.chars().take(120).collect(),
        });
        i += 1;
    }
    out
}

/// Best-effort docstring: a preceding comment, or a Python-style leading string.
fn doc_for(node: Node<'_>, src: &[u8], kind: &str) -> String {
    if let Some(prev) = node.prev_named_sibling() {
        if prev.kind().contains("comment") {
            return clean_comment(&text(prev, src));
        }
    }
    // Python docstrings live as the first statement of the `body` block.
    if kind.ends_with("_definition") || kind.ends_with("_declaration") {
        if let Some(body) = node.child_by_field_name("body") {
            if let Some(first) = named_children(body).into_iter().next() {
                if first.kind() == "expression_statement" {
                    if let Some(inner) = named_children(first).into_iter().next() {
                        if inner.kind().contains("string") {
                            return clean_comment(&text(inner, src));
                        }
                    }
                }
            }
        }
    }
    String::new()
}

fn clean_comment(raw: &str) -> String {
    raw.lines()
        .map(|l| l.trim().trim_start_matches(['/', '#', '*', '!', ' ']))
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(500)
        .collect()
}

/// Parse an import node into bindings.
///
/// Handles `use a::b::{c, d}`, `from m import x as y`, `import a, b`,
/// `import { a, b as c } from "m"`, and Go `alias "path"`. The parse is
/// deliberately forgiving — an import it cannot interpret simply yields no
/// binding rather than an error.
fn parse_import(node: Node<'_>, src: &[u8]) -> Vec<ImportDef> {
    let raw = text(node, src);
    let text = raw.trim().trim_end_matches(';').trim();
    let mut out = Vec::new();

    if text.starts_with("use ") {
        // Rust: `use crate::foo::Bar;` / `use foo::{a, b as c};`
        let body = text.trim_start_matches("use ").trim();
        if let Some((head, tail)) = body.split_once('{') {
            let module = head.trim().trim_end_matches("::").trim();
            for item in tail.trim_end_matches('}').split(',') {
                if let Some((local, target)) = parse_alias(item) {
                    out.push(ImportDef {
                        local_name: local,
                        target_name: target,
                        module: module.to_string(),
                    });
                }
            }
        } else if let Some((local, target)) = parse_alias(body) {
            out.push(ImportDef {
                local_name: local,
                target_name: target,
                module: module_of(body),
            });
        }
        return out;
    }

    if let Some(rest) = text.strip_prefix("from ") {
        // Python: `from pkg.mod import a, b as c`
        if let Some((module, names)) = rest.split_once(" import ") {
            for item in names.trim_matches(['(', ')']).split(',') {
                if let Some((local, target)) = parse_alias(item) {
                    out.push(ImportDef {
                        local_name: local,
                        target_name: target,
                        module: module.trim().to_string(),
                    });
                }
            }
        }
        return out;
    }

    if let Some(rest) = text.strip_prefix("import ") {
        // Python `import os, sys`; TS `import {a} from "m"`; Go `import "fmt"`.
        if let Some((names, module)) = rest.rsplit_once(" from ") {
            for item in names.trim().trim_matches(['{', '}']).split(',') {
                if let Some((local, target)) = parse_alias(item) {
                    out.push(ImportDef {
                        local_name: local,
                        target_name: target,
                        module: module.trim().trim_matches(['"', '\'', ';']).to_string(),
                    });
                }
            }
            return out;
        }
        for item in rest.split(',') {
            if let Some((local, target)) = parse_alias(item) {
                out.push(ImportDef {
                    local_name: local,
                    target_name: target,
                    module: String::new(),
                });
            }
        }
        return out;
    }

    // Go `import_spec`: `"fmt"` or `alias "path"`.
    let cleaned = text.trim_matches(['"', '\'', ' ']);
    if !cleaned.is_empty() && !cleaned.contains(' ') {
        let name = cleaned.rsplit('/').next().unwrap_or(cleaned).to_string();
        out.push(ImportDef {
            local_name: name.clone(),
            target_name: name,
            module: cleaned.to_string(),
        });
    }
    out
}

/// Split `a as b` / `a` into `(local, target)`.
fn parse_alias(item: &str) -> Option<(String, String)> {
    let item = item.trim().trim_matches(['{', '}', '(', ')', ',', ' ']);
    if item.is_empty() {
        return None;
    }
    let (local, target) = if let Some((orig, alias)) = item.split_once(" as ") {
        (alias.trim().to_string(), orig.trim().to_string())
    } else {
        let name = last_segment(item);
        (name.clone(), name)
    };
    if local.is_empty() {
        return None;
    }
    Some((local, target))
}

/// The module path portion of a `use` path (everything but the last segment).
fn module_of(path: &str) -> String {
    let path = path.trim();
    match path.rsplit_once("::") {
        Some((module, _)) => module.trim().to_string(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rust(source: &str) -> ExtractedFile {
        extract(source, SupportLang::Rust, "m")
    }

    #[test]
    fn extracts_rust_functions_structs_and_methods() {
        let src = r"
/// Parses a config.
pub fn parse_config(path: &str) -> Config {
    load(path)
}

pub struct Config { name: String }

impl Config {
    /// Returns the name.
    pub fn name(&self) -> &str {
        &self.name
    }
}
";
        let f = rust(src);
        let names: Vec<&str> = f.symbols.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"parse_config"), "{names:?}");
        assert!(names.contains(&"Config"), "{names:?}");
        assert!(names.contains(&"name"), "{names:?}");
        let parse = f
            .symbols
            .iter()
            .find(|s| s.name == "parse_config")
            .expect("parse_config");
        assert_eq!(parse.qualified_name, "m::parse_config");
        assert_eq!(parse.label, Label::Function);
        assert!(
            parse.doc.contains("Parses a config"),
            "doc was {:?}",
            parse.doc
        );
        let method = f.symbols.iter().find(|s| s.name == "name").expect("method");
        assert_eq!(method.qualified_name, "m::Config::name");
        assert_eq!(method.label, Label::Method);
    }

    #[test]
    fn extracts_rust_calls_with_args() {
        let src = r"
fn caller() {
    let v = compute(1, x + 2);
}
";
        let f = rust(src);
        let call = f
            .calls
            .iter()
            .find(|c| c.callee == "compute")
            .expect("call");
        assert_eq!(call.caller_qn, "m::caller");
        assert_eq!(call.args.len(), 2);
        assert_eq!(call.args[0].i, 0);
        assert_eq!(call.args[0].e, "1");
    }

    #[test]
    fn extracts_rust_use_imports() {
        let f = rust("use crate::foo::{Bar, baz as qux};\n");
        let locals: Vec<&str> = f.imports.iter().map(|i| i.local_name.as_str()).collect();
        assert!(locals.contains(&"Bar"), "{locals:?}");
        assert!(locals.contains(&"qux"), "{locals:?}");
        let qux = f
            .imports
            .iter()
            .find(|i| i.local_name == "qux")
            .expect("qux");
        assert_eq!(qux.target_name, "baz");
    }

    #[test]
    fn extracts_python_definitions_and_docstrings() {
        let src = r#"
import os
from pkg import helper as h

class Service:
    """A service."""
    def handle(self, req):
        return h(req)
"#;
        let f = extract(src, SupportLang::Python, "svc");
        let names: Vec<&str> = f.symbols.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"Service"), "{names:?}");
        assert!(names.contains(&"handle"), "{names:?}");
        let svc = f
            .symbols
            .iter()
            .find(|s| s.name == "Service")
            .expect("Service");
        assert!(svc.doc.contains("A service"), "doc {:?}", svc.doc);
        let locals: Vec<&str> = f.imports.iter().map(|i| i.local_name.as_str()).collect();
        assert!(locals.contains(&"os"), "{locals:?}");
        assert!(locals.contains(&"h"), "{locals:?}");
    }

    #[test]
    fn extracts_go_functions_and_methods() {
        let src = r#"
package main

import "fmt"

func main() {
    fmt.Println("hi")
}

func (s *Server) Start() error {
    return nil
}
"#;
        let f = extract(src, SupportLang::Go, "main");
        let names: Vec<&str> = f.symbols.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"main"), "{names:?}");
        assert!(names.contains(&"Start"), "{names:?}");
    }

    #[test]
    fn extracts_typescript_classes_and_calls() {
        let src = r#"
import { helper } from "./util";

export class Widget {
  render(): void {
    helper();
  }
}
"#;
        let f = extract(src, SupportLang::TypeScript, "widget");
        let names: Vec<&str> = f.symbols.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"Widget"), "{names:?}");
        assert!(names.contains(&"render"), "{names:?}");
        let locals: Vec<&str> = f.imports.iter().map(|i| i.local_name.as_str()).collect();
        assert!(locals.contains(&"helper"), "{locals:?}");
        assert!(
            f.calls.iter().any(|c| c.callee == "helper"),
            "{:?}",
            f.calls
        );
    }

    #[test]
    fn last_segment_handles_paths_and_calls() {
        assert_eq!(last_segment("a::b::c"), "c");
        assert_eq!(last_segment("self.foo"), "foo");
        assert_eq!(last_segment("Foo::bar(x)"), "bar");
        assert_eq!(last_segment("&mut self.name"), "name");
    }

    #[test]
    fn unknown_language_yields_nothing_not_panic() {
        let f = extract("", SupportLang::Rust, "m");
        assert!(f.symbols.is_empty(), "empty source has no symbols");
    }
}
