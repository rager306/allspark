//! R5 — schema resource cache keyed by the canonical content hash.
//!
//! Compiling a resource (a schema, a query plan, a config) is expensive, so
//! [`SchemaCache`] memoizes the result under a [`ResourceId`]: the **SHA-256**
//! of the resource's *canonical JSON*. Two documents that differ only in key
//! order (or whitespace, which JSON does not carry) canonicalize to the same
//! byte string, so they share one cache entry and compile once.
//!
//! The canonicalizer here is a local one — [`canonicalize`] — rather than a
//! dependency: objects are re-emitted with keys sorted lexicographically and a
//! fixed separator, arrays keep their order, and scalars reuse `serde_json`'s
//! stable number/string encoding. (The crate's `serde_json` is built with
//! `preserve_order`, i.e. an insertion-ordered map, which is exactly why an
//! explicit sort is required for a content-addressed key.)
//!
//! The "compiler" is a caller-supplied closure; [`default_compiler`] is a
//! deterministic placeholder that produces a stable blob from the canonical
//! text, so the cache is exercisable without a real code generator.

#![forbid(unsafe_code)]

use std::collections::HashMap;
use std::sync::Arc;

use serde_json::Value;
use sha2::{Digest, Sha256};

/// Content-addressed id of a resource: lowercase hex SHA-256.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ResourceId(pub String);

impl ResourceId {
    /// The hex string.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for ResourceId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// A compiled/processed resource blob.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Compiled {
    /// Id of the resource this was compiled from.
    pub id: ResourceId,
    /// The processed bytes (opaque to the cache).
    pub blob: Vec<u8>,
}

impl Compiled {
    /// The blob as UTF-8, if it is valid UTF-8.
    #[must_use]
    pub fn as_str(&self) -> Option<&str> {
        std::str::from_utf8(&self.blob).ok()
    }
}

/// Canonicalize a JSON value to a deterministic string: object keys sorted
/// lexicographically, no insignificant whitespace, scalars encoded by
/// `serde_json`'s stable form.
#[must_use]
pub fn canonicalize(value: &Value) -> String {
    let mut out = String::new();
    write_canonical(value, &mut out);
    out
}

fn write_canonical(value: &Value, out: &mut String) {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) => out.push_str(&n.to_string()),
        Value::String(s) => {
            // `serde_json::to_string` on a string is a stable, escaped JSON
            // string literal (no surrounding whitespace).
            out.push_str(&serde_json::to_string(s).unwrap_or_else(|_| "\"\"".to_string()));
        }
        Value::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_canonical(item, out);
            }
            out.push(']');
        }
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort_unstable();
            out.push('{');
            for (i, key) in keys.into_iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push_str(&serde_json::to_string(key).unwrap_or_else(|_| "\"\"".to_string()));
                out.push(':');
                // `map.get(key)` is always `Some` here (key came from the map).
                if let Some(child) = map.get(key) {
                    write_canonical(child, out);
                }
            }
            out.push('}');
        }
    }
}

/// SHA-256 of the canonical form of `value`, hex-encoded.
#[must_use]
pub fn resource_id(value: &Value) -> ResourceId {
    ResourceId(sha256_hex(canonicalize(value).as_bytes()))
}

fn sha256_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(digest.len() * 2);
    for &b in &digest {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0x0f) as usize] as char);
    }
    out
}

/// A deterministic placeholder "compiler": prefix the canonical text with a
/// version tag and return it as the compiled blob.
#[must_use]
pub fn default_compiler(value: &Value) -> Compiled {
    let canonical = canonicalize(value);
    let blob = format!("pa-graph-compiled:v1\n{canonical}").into_bytes();
    Compiled {
        id: resource_id(value),
        blob,
    }
}

/// Content-addressed compilation cache.
#[derive(Debug, Default)]
pub struct SchemaCache {
    entries: HashMap<ResourceId, Arc<Compiled>>,
    hits: usize,
    misses: usize,
}

impl SchemaCache {
    /// An empty cache.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Number of cached resources.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the cache is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Cache hits since creation.
    #[must_use]
    pub fn hits(&self) -> usize {
        self.hits
    }

    /// Cache misses (compilations) since creation.
    #[must_use]
    pub fn misses(&self) -> usize {
        self.misses
    }

    /// Whether `value` is already cached.
    #[must_use]
    pub fn contains(&self, value: &Value) -> bool {
        self.entries.contains_key(&resource_id(value))
    }

    /// Return the compiled resource for `value`, compiling it with `compiler`
    /// on a miss. The returned [`Arc`] is shared, so callers clone the handle
    /// rather than the blob.
    pub fn get_or_compile<F>(&mut self, value: &Value, compiler: F) -> (ResourceId, Arc<Compiled>)
    where
        F: FnOnce(&Value) -> Compiled,
    {
        let id = resource_id(value);
        if let Some(hit) = self.entries.get(&id) {
            self.hits += 1;
            return (id, Arc::clone(hit));
        }
        self.misses += 1;
        let mut out = compiler(value);
        // The id is content-addressed; a compiler that mislabels its output
        // must not poison the key space, so stamp the true id.
        out.id = id.clone();
        let arc = Arc::new(out);
        self.entries.insert(id.clone(), Arc::clone(&arc));
        (id, arc)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn key_order_does_not_change_the_id() {
        let a = json!({"b": 1, "a": 2, "c": [3, {"z": 0, "y": 1}]});
        let b = json!({"c": [3, {"y": 1, "z": 0}], "a": 2, "b": 1});
        assert_eq!(canonicalize(&a), canonicalize(&b));
        assert_eq!(resource_id(&a), resource_id(&b));
    }

    #[test]
    fn different_content_differs() {
        let a = json!({"a": 1});
        let b = json!({"a": 2});
        let c = json!({"a": "1"});
        assert_ne!(resource_id(&a), resource_id(&b));
        assert_ne!(resource_id(&a), resource_id(&c));
        // Array order is significant.
        assert_ne!(resource_id(&json!([1, 2])), resource_id(&json!([2, 1])));
    }

    #[test]
    fn id_is_lowercase_hex_sha256() {
        let id = resource_id(&json!({"x": true}));
        assert_eq!(id.as_str().len(), 64);
        assert!(id
            .as_str()
            .chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
        assert_eq!(id.to_string(), id.as_str());
    }

    #[test]
    fn cache_compiles_once_per_content() {
        let mut cache = SchemaCache::new();
        let first = json!({"k1": 1, "k2": 2});
        let reordered = json!({"k2": 2, "k1": 1});

        let (id1, compiled1) = cache.get_or_compile(&first, default_compiler);
        assert_eq!(cache.misses(), 1);
        assert_eq!(cache.hits(), 0);

        // Same content, different key order → same id, no recompilation.
        let (id2, compiled2) = cache.get_or_compile(&reordered, default_compiler);
        assert_eq!(id1, id2);
        assert_eq!(cache.misses(), 1);
        assert_eq!(cache.hits(), 1);
        assert!(Arc::ptr_eq(&compiled1, &compiled2));
        assert_eq!(cache.len(), 1);
        assert!(cache.contains(&reordered));
        assert_eq!(compiled2.as_str(), compiled1.as_str());

        // Different content → new entry.
        let (id3, _) = cache.get_or_compile(&json!({"k1": 9}), default_compiler);
        assert_ne!(id3, id1);
        assert_eq!(cache.misses(), 2);
        assert_eq!(cache.len(), 2);
        assert!(!cache.is_empty());
    }

    #[test]
    fn compiler_closure_is_used_and_id_stamped() {
        let mut cache = SchemaCache::new();
        let value = json!({"a": 1});
        let (id, compiled) = cache.get_or_compile(&value, |v| Compiled {
            id: ResourceId("bogus".into()),
            blob: format!("custom:{}", canonicalize(v)).into_bytes(),
        });
        // The cache overrides a mislabelled compiler id.
        assert_eq!(compiled.id, id);
        assert_eq!(
            compiled.as_str(),
            Some(format!("custom:{}", canonicalize(&value)).as_str())
        );
    }
}
