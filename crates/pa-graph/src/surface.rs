//! Per-file *surface* and *body* hashing (G7 closure-repair, L11 surface cache).
//!
//! The surface of a file is everything another file may consume from it: the
//! signatures of its definitions — name, kind, parameter list, and return type
//! — **without** any function body. Two revisions of a file whose surfaces hash
//! equal are interchangeable as far as every other file is concerned, which is
//! what makes a depth-1 closure sound:
//!
//! * a change confined to a body (surface hash unchanged) can only affect the
//!   file itself — reindex that one file;
//! * a change to the surface can change how *dependents* resolve calls and
//!   imports — reindex the file plus its direct dependents (closure depth 1).
//!
//! The surface is derived from `pa-ast`'s tree-sitter parse alone (a pure
//! function of the file bytes), so an untouched dependent can never change its
//! own surface — no fixpoint iteration is needed.
//!
//! Canonical form: a hand-written JSON object (field order fixed, symbol array
//! sorted by `(kind, name, start_line)`) so byte equality of the serialization
//! is equality of the surface, independent of `serde_json` map ordering.

use pa_ast::SupportLang;
use sha2::{Digest, Sha256};
use tree_sitter::Node;

/// One signature-bearing symbol of a file's surface.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SurfaceSymbol {
    /// Node kind wire name (e.g. `Function`, `Struct`).
    pub kind: String,
    /// Short symbol name.
    pub name: String,
    /// Normalized parameter/field list (empty when the kind has none).
    pub params: String,
    /// Normalized return type (empty when absent).
    pub ret: String,
    /// 1-based start line (disambiguates same-named siblings).
    pub start_line: u32,
}

/// The surface of one file plus its two hashes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileSurface {
    /// SHA-256 of the canonical surface JSON.
    pub surface_sha: String,
    /// SHA-256 of the raw file bytes (the "body" hash).
    pub body_sha: String,
    /// The canonical surface JSON that was hashed.
    pub surface_json: String,
    /// The surface symbols, sorted canonically.
    pub symbols: Vec<SurfaceSymbol>,
}

/// Compute the canonical surface of `source`.
///
/// `module` is the qualified-name prefix derived from the file path (see
/// `crate::indexer`); it participates in the surface so a file move is a
/// surface change.
#[must_use]
pub fn surface_of(source: &str, lang: SupportLang, module: &str) -> FileSurface {
    let mut symbols = Vec::new();
    if let Ok(Some(tree)) = pa_ast::parse_cache::parse_cached(source, lang) {
        collect(tree.root_node(), source.as_bytes(), &mut symbols);
    }
    // The surface is line-independent: a body edit that shifts lines must not
    // change the surface. Sort by the signature fields themselves, with the
    // line only as a tie-breaker (excluded from the hashed JSON below).
    symbols.sort_by(|a, b| {
        a.kind
            .cmp(&b.kind)
            .then_with(|| a.name.cmp(&b.name))
            .then_with(|| a.params.cmp(&b.params))
            .then_with(|| a.ret.cmp(&b.ret))
            .then_with(|| a.start_line.cmp(&b.start_line))
    });
    let surface_json = canonical_json(lang, module, &symbols);
    let surface_sha = sha256_hex(surface_json.as_bytes());
    FileSurface {
        surface_sha,
        body_sha: sha256_hex(source.as_bytes()),
        surface_json,
        symbols,
    }
}

/// Hash of the file's *surface* only (signatures, no bodies).
#[must_use]
pub fn surface_hash(source: &str, lang: SupportLang, module: &str) -> String {
    surface_of(source, lang, module).surface_sha
}

/// Hash of the whole file content (the "body" side of the split).
#[must_use]
pub fn body_hash(source: &str) -> String {
    sha256_hex(source.as_bytes())
}

/// Read `path` and compute its surface; `None` for an unsupported language.
#[must_use]
pub fn surface_hash_file(path: &std::path::Path, module: &str) -> Option<String> {
    let lang = SupportLang::from_path(path)?;
    let source = std::fs::read_to_string(path).ok()?;
    Some(surface_hash(&source, lang, module))
}

/// Read `path` and compute its body (content) hash.
#[must_use]
pub fn body_hash_file(path: &std::path::Path) -> Option<String> {
    let bytes = std::fs::read(path).ok()?;
    Some(sha256_hex(&bytes))
}

/// Render the canonical surface JSON: fixed key order, sorted symbols.
fn canonical_json(lang: SupportLang, module: &str, symbols: &[SurfaceSymbol]) -> String {
    let mut out = String::with_capacity(64 + symbols.len() * 64);
    out.push_str("{\"v\":2,\"lang\":\"");
    out.push_str(&escape(&format!("{lang:?}")));
    out.push_str("\",\"module\":\"");
    out.push_str(&escape(module));
    out.push_str("\",\"symbols\":[");
    for (i, s) in symbols.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push_str("{\"k\":\"");
        out.push_str(&escape(&s.kind));
        out.push_str("\",\"n\":\"");
        out.push_str(&escape(&s.name));
        out.push_str("\",\"p\":\"");
        out.push_str(&escape(&s.params));
        out.push_str("\",\"r\":\"");
        out.push_str(&escape(&s.ret));
        // No line number: the surface is a signature set, not a layout. The
        // return-type string must be closed before the object: an unclosed
        // `"r":"` made 82.7% of stored rows invalid JSON (1,683 of 2,034).
        out.push_str("\"}");
    }
    out.push_str("]}");
    out
}

/// Minimal JSON string escaping (control chars, `"`, `\`).
fn escape(value: &str) -> String {
    use std::fmt::Write as _;
    let mut out = String::with_capacity(value.len());
    for c in value.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out
}

const HEX: &[u8; 16] = b"0123456789abcdef";

/// Lowercase hex of a SHA-256 digest.
pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(digest.len() * 2);
    for &b in &digest {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0x0f) as usize] as char);
    }
    out
}

/// Whether a tree-sitter kind is a signature-bearing definition, and its
/// surface label.
fn surface_kind(kind: &str) -> Option<&'static str> {
    Some(match kind {
        "function_item" | "function_definition" | "function_declaration" => "Function",
        "method_definition" | "method_declaration" | "constructor_declaration" => "Method",
        "struct_item" | "struct_specifier" | "type_spec" | "type_alias_declaration" => "Struct",
        "enum_item" | "enum_specifier" | "enum_declaration" => "Enum",
        "trait_item" | "interface_declaration" | "protocol_declaration" => "Interface",
        "class_definition" | "class_declaration" | "class_specifier" => "Class",
        "mod_item" | "namespace_definition" => "Module",
        _ => return None,
    })
}

fn collect(node: Node<'_>, src: &[u8], out: &mut Vec<SurfaceSymbol>) {
    if let Some(label) = surface_kind(node.kind()) {
        if let Some(name) = node_name(node, src) {
            out.push(SurfaceSymbol {
                kind: label.to_string(),
                name,
                params: signature_text(node, src, params_fields(node.kind())),
                ret: signature_text(node, src, ret_fields(node.kind())),
                start_line: line(node.start_position().row),
            });
        }
    }
    // Nested definitions (methods inside impl/class) are part of the surface
    // too: reordering or renaming them is an API change for dependents.
    for child in named_children(node) {
        collect(child, src, out);
    }
}

fn params_fields(kind: &str) -> &'static [&'static str] {
    match kind {
        "function_item"
        | "function_definition"
        | "function_declaration"
        | "method_definition"
        | "method_declaration"
        | "constructor_declaration" => &["parameters", "parameter_list"],
        _ => &[],
    }
}

fn ret_fields(kind: &str) -> &'static [&'static str] {
    match kind {
        "function_item"
        | "function_definition"
        | "function_declaration"
        | "method_definition"
        | "method_declaration" => &["return_type", "result"],
        _ => &[],
    }
}

/// The text of the first present field among `fields`, whitespace-normalized.
fn signature_text(node: Node<'_>, src: &[u8], fields: &[&str]) -> String {
    for field in fields {
        if let Some(n) = node.child_by_field_name(field) {
            let normalized = normalize_ws(n.utf8_text(src).unwrap_or(""));
            if !normalized.is_empty() {
                return normalized;
            }
        }
    }
    String::new()
}

/// Collapse every run of whitespace to a single space and trim.
fn normalize_ws(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut pending_space = false;
    for c in raw.chars() {
        if c.is_whitespace() {
            pending_space = !out.is_empty();
        } else {
            if pending_space {
                out.push(' ');
                pending_space = false;
            }
            out.push(c);
        }
    }
    out
}

fn node_name(node: Node<'_>, src: &[u8]) -> Option<String> {
    if let Some(n) = node.child_by_field_name("name") {
        return Some(n.utf8_text(src).unwrap_or("").trim().to_string());
    }
    if let Some(n) = node.child_by_field_name("type") {
        let text = n.utf8_text(src).unwrap_or("").trim().to_string();
        return text.rsplit([':', '.', '>']).next().map(str::to_string);
    }
    None
}

fn named_children(node: Node<'_>) -> Vec<Node<'_>> {
    let mut cursor = node.walk();
    node.named_children(&mut cursor).collect()
}

fn line(row: usize) -> u32 {
    u32::try_from(row).unwrap_or(u32::MAX).saturating_add(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SRC: &str = r"
pub fn parse_config(path: &str, retries: u32) -> Config {
    let raw = read_file(path);
    Config { raw }
}

pub struct Config {
    pub raw: String,
}

impl Config {
    pub fn len(&self) -> usize {
        self.raw.len()
    }
}
";

    /// The canonical surface JSON must actually parse. Every previous test
    /// compared hashes, so an unclosed string survived: 1,683 of 2,034 stored
    /// rows were invalid JSON and nothing noticed because no reader existed yet.
    #[test]
    fn the_surface_json_parses() {
        let surface = surface_of(SRC, SupportLang::Rust, "a::b");
        let parsed: serde_json::Value =
            serde_json::from_str(&surface.surface_json).expect("surface JSON must parse");
        assert_eq!(parsed["v"], 2);
        assert_eq!(parsed["module"], "a::b");
        let symbols = parsed["symbols"].as_array().expect("symbols array");
        assert!(!symbols.is_empty(), "expected symbols: {parsed}");
        for symbol in symbols {
            for key in ["k", "n", "p", "r"] {
                assert!(
                    symbol.get(key).is_some_and(serde_json::Value::is_string),
                    "symbol field {key} must be a string: {symbol}"
                );
            }
        }
    }

    #[test]
    fn body_edit_leaves_surface_unchanged() {
        let edited = SRC.replace(
            "let raw = read_file(path);",
            "let raw = read_file(path); // a body-only comment\n    let _ = retries;",
        );
        assert_ne!(body_hash(SRC), body_hash(&edited));
        assert_eq!(
            surface_hash(SRC, SupportLang::Rust, "m"),
            surface_hash(&edited, SupportLang::Rust, "m"),
            "a body-only edit must not change the surface"
        );
    }

    #[test]
    fn signature_edit_changes_surface() {
        let edited = SRC.replace("retries: u32", "retries: u64");
        assert_ne!(
            surface_hash(SRC, SupportLang::Rust, "m"),
            surface_hash(&edited, SupportLang::Rust, "m"),
            "a parameter-type change is a surface change"
        );
    }

    #[test]
    fn renamed_symbol_changes_surface() {
        let edited = SRC.replace("fn parse_config", "fn load_config");
        assert_ne!(
            surface_hash(SRC, SupportLang::Rust, "m"),
            surface_hash(&edited, SupportLang::Rust, "m")
        );
    }

    #[test]
    fn module_move_changes_surface() {
        assert_ne!(
            surface_hash(SRC, SupportLang::Rust, "a::b"),
            surface_hash(SRC, SupportLang::Rust, "a::c")
        );
    }

    #[test]
    fn surface_is_deterministic() {
        let a = surface_of(SRC, SupportLang::Rust, "m");
        let b = surface_of(SRC, SupportLang::Rust, "m");
        assert_eq!(a.surface_sha, b.surface_sha);
        assert_eq!(a.surface_json, b.surface_json);
        assert_eq!(a.surface_sha.len(), 64);
        assert_eq!(a.body_sha.len(), 64);
    }

    #[test]
    fn surface_includes_methods_and_types() {
        let s = surface_of(SRC, SupportLang::Rust, "m");
        let names: Vec<&str> = s.symbols.iter().map(|x| x.name.as_str()).collect();
        assert!(names.contains(&"parse_config"), "{names:?}");
        assert!(names.contains(&"Config"), "{names:?}");
        assert!(names.contains(&"len"), "{names:?}");
        let parse = s
            .symbols
            .iter()
            .find(|x| x.name == "parse_config")
            .expect("parse_config");
        assert!(parse.params.contains("path"), "params: {:?}", parse.params);
        assert!(parse.ret.contains("Config"), "ret: {:?}", parse.ret);
    }

    #[test]
    fn comment_change_is_not_a_surface_change() {
        let edited = SRC.replace("pub struct Config {", "// a comment\npub struct Config {");
        assert_eq!(
            surface_hash(SRC, SupportLang::Rust, "m"),
            surface_hash(&edited, SupportLang::Rust, "m")
        );
    }
}
