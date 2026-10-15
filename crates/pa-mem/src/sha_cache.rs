//! P8 — a build-artifact cache keyed by an input fingerprint.
//!
//! pogocache's `tools/build/run.sh` rebuilds a platform package only when the
//! source tree's SHA-1 changes:
//!
//! ```sh
//! reposha=$(tar -O -xf repo.tar.gz | sha1sum | awk '{print $1;}')
//! if [[ -f "../../packages/$name.tar.gz" ]]; then
//!     reposha2=$(cat $name/reposha)
//!     if [[ "$reposha2" == "$reposha" ]]; then build=0; fi
//! fi
//! ```
//!
//! [`ShaCache`] is that idea as a reusable, testable unit: fingerprint a set of
//! inputs (source files, an env, a toolchain id) into one SHA-256, and store or
//! look up the artifact built from exactly those inputs. It is deliberately
//! *not* wired into CI here — it is the helper the packaging script would call.
//!
//! The real cross-compile matrix (five platforms, static musl) stays in the
//! packaging pipeline; this module owns only the cache-key half.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::error::{MemError, MemResult};

/// A content-addressed build cache: inputs → one SHA-256 key → an artifact path.
///
/// The cache is a directory. Each entry is `<root>/<key>.bin` (the artifact)
/// beside `<root>/<key>.inputs` (the manifest that produced it), so a cache
/// lookup can be audited without rebuilding.
#[derive(Debug, Clone)]
pub struct ShaCache {
    root: PathBuf,
}

impl ShaCache {
    /// A cache rooted at `root` (created on first store).
    #[must_use]
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// The cache root.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The SHA-256 key for a set of inputs.
    ///
    /// The inputs are folded in a fixed order — every `(name, bytes)` sorted by
    /// name, each length-prefixed — so the key depends only on the input
    /// *set*, not on the order the caller happened to collect it in. A length
    /// prefix keeps `("ab","c")` distinct from `("a","bc")`.
    #[must_use]
    pub fn key<'a, I>(&self, inputs: I) -> String
    where
        I: IntoIterator<Item = (&'a str, &'a [u8])>,
    {
        let sorted: BTreeMap<&str, &[u8]> = inputs.into_iter().collect();
        let mut hasher = Sha256::new();
        hasher.update(b"pa-mem:sha-cache:v1\0");
        for (name, bytes) in sorted {
            hasher.update((name.len() as u64).to_le_bytes());
            hasher.update(name.as_bytes());
            hasher.update((bytes.len() as u64).to_le_bytes());
            hasher.update(bytes);
        }
        let digest = hasher.finalize();
        let mut out = String::with_capacity(64);
        for byte in digest {
            // `write!` into the String avoids the temporary `format!` alloc.
            let _ = write!(out, "{byte:02x}");
        }
        out
    }

    /// The SHA-256 key for a set of files, folding in each file's bytes.
    ///
    /// Files are identified by their path relative to `base` (so the same tree
    /// at two absolute locations fingerprints identically), which is exactly
    /// the property a reproducible build cache needs.
    ///
    /// # Errors
    ///
    /// [`MemError::Io`] when a file cannot be read.
    pub fn key_for_files(&self, base: &Path, files: &[PathBuf]) -> MemResult<String> {
        let mut inputs: Vec<(String, Vec<u8>)> = Vec::with_capacity(files.len());
        for file in files {
            let relative = file
                .strip_prefix(base)
                .unwrap_or(file)
                .to_string_lossy()
                .into_owned();
            let bytes =
                fs::read(file).map_err(|err| MemError::Io(format!("{}: {err}", file.display())))?;
            inputs.push((relative, bytes));
        }
        let borrowed: Vec<(&str, &[u8])> = inputs
            .iter()
            .map(|(name, bytes)| (name.as_str(), bytes.as_slice()))
            .collect();
        Ok(self.key(borrowed))
    }

    /// The artifact path for `key`.
    #[must_use]
    pub fn artifact_path(&self, key: &str) -> PathBuf {
        self.root.join(format!("{key}.bin"))
    }

    /// The input-manifest path for `key`.
    #[must_use]
    pub fn manifest_path(&self, key: &str) -> PathBuf {
        self.root.join(format!("{key}.inputs"))
    }

    /// Whether an artifact for `key` is cached.
    #[must_use]
    pub fn contains(&self, key: &str) -> bool {
        self.artifact_path(key).is_file()
    }

    /// Stores `artifact` under `key`, writing the manifest beside it.
    ///
    /// # Errors
    ///
    /// [`MemError::Io`] when the cache directory or files cannot be written.
    pub fn store(&self, key: &str, manifest: &[String], artifact: &[u8]) -> MemResult<PathBuf> {
        fs::create_dir_all(&self.root)
            .map_err(|err| MemError::Io(format!("{}: {err}", self.root.display())))?;
        let path = self.artifact_path(key);
        fs::write(&path, artifact)
            .map_err(|err| MemError::Io(format!("{}: {err}", path.display())))?;
        let manifest_path = self.manifest_path(key);
        let body = manifest.join("\n");
        fs::write(&manifest_path, body)
            .map_err(|err| MemError::Io(format!("{}: {err}", manifest_path.display())))?;
        Ok(path)
    }

    /// Returns the cached artifact for `key`, if present.
    ///
    /// # Errors
    ///
    /// [`MemError::Io`] when the artifact exists but cannot be read.
    pub fn get(&self, key: &str) -> MemResult<Option<Vec<u8>>> {
        let path = self.artifact_path(key);
        match fs::read(&path) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(err) => Err(MemError::Io(format!("{}: {err}", path.display()))),
        }
    }

    /// The manifest recorded for `key`, if any.
    ///
    /// # Errors
    ///
    /// [`MemError::Io`] when the manifest exists but cannot be read.
    pub fn manifest(&self, key: &str) -> MemResult<Option<String>> {
        let path = self.manifest_path(key);
        match fs::read_to_string(&path) {
            Ok(text) => Ok(Some(text)),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(err) => Err(MemError::Io(format!("{}: {err}", path.display()))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn the_key_is_order_independent() {
        let cache = ShaCache::new("/unused");
        let a = cache.key([("b", b"two".as_slice()), ("a", b"one".as_slice())]);
        let b = cache.key([("a", b"one".as_slice()), ("b", b"two".as_slice())]);
        assert_eq!(a, b);
    }

    #[test]
    fn the_key_is_length_prefixed() {
        let cache = ShaCache::new("/unused");
        // Without the length prefix, ("ab","c") and ("a","bc") would collide
        // on the concatenated byte stream; the name split keeps them apart too.
        let a = cache.key([("x", b"ab".as_slice()), ("y", b"c".as_slice())]);
        let b = cache.key([("x", b"a".as_slice()), ("y", b"bc".as_slice())]);
        assert_ne!(a, b);
    }

    #[test]
    fn a_changed_input_changes_the_key() {
        let cache = ShaCache::new("/unused");
        let before = cache.key([("src/lib.rs", b"fn main() {}".as_slice())]);
        let after = cache.key([("src/lib.rs", b"fn main() { }".as_slice())]);
        assert_ne!(before, after);
    }

    #[test]
    fn the_key_is_a_sha256_hex_string() {
        let cache = ShaCache::new("/unused");
        let key = cache.key([("k", b"v".as_slice())]);
        assert_eq!(key.len(), 64);
        assert!(key.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn store_then_hit_returns_the_artifact() {
        let dir = tempfile::tempdir().expect("tempdir");
        let cache = ShaCache::new(dir.path());
        let key = cache.key([("input", b"hello".as_slice())]);
        assert!(!cache.contains(&key));
        assert_eq!(cache.get(&key).expect("get"), None);

        let manifest = vec!["input=hello".to_string()];
        cache
            .store(&key, &manifest, b"artifact-bytes")
            .expect("store");
        assert!(cache.contains(&key));
        assert_eq!(
            cache.get(&key).expect("get").expect("present"),
            b"artifact-bytes"
        );
        assert_eq!(
            cache.manifest(&key).expect("manifest"),
            Some("input=hello".to_string())
        );
    }

    #[test]
    fn a_missing_artifact_is_a_miss_not_an_error() {
        let dir = tempfile::tempdir().expect("tempdir");
        let cache = ShaCache::new(dir.path());
        assert_eq!(cache.get("deadbeef").expect("get"), None);
        assert_eq!(cache.manifest("deadbeef").expect("manifest"), None);
    }

    #[test]
    fn files_fingerprint_by_relative_path() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("src.rs");
        fs::write(&file, b"contents").expect("write");
        let cache = ShaCache::new("/unused");
        let a = cache
            .key_for_files(dir.path(), std::slice::from_ref(&file))
            .expect("key");
        // The same file under a different base directory, but the same relative
        // name, must fingerprint identically.
        let other = tempfile::tempdir().expect("tempdir");
        let other_file = other.path().join("src.rs");
        fs::write(&other_file, b"contents").expect("write");
        let b = cache
            .key_for_files(other.path(), std::slice::from_ref(&other_file))
            .expect("key");
        assert_eq!(a, b);
    }

    #[test]
    fn a_changed_file_changes_the_file_key() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("src.rs");
        let cache = ShaCache::new("/unused");
        fs::write(&file, b"one").expect("write");
        let before = cache
            .key_for_files(dir.path(), std::slice::from_ref(&file))
            .expect("key");
        fs::write(&file, b"two").expect("write");
        let after = cache
            .key_for_files(dir.path(), std::slice::from_ref(&file))
            .expect("key");
        assert_ne!(before, after);
    }

    #[test]
    fn an_unreadable_file_is_an_error() {
        let dir = tempfile::tempdir().expect("tempdir");
        let cache = ShaCache::new("/unused");
        let missing = dir.path().join("nope.rs");
        assert!(matches!(
            cache.key_for_files(dir.path(), &[missing]),
            Err(MemError::Io(_))
        ));
    }

    #[test]
    fn the_cache_survives_a_reopen() {
        let dir = tempfile::tempdir().expect("tempdir");
        let key = {
            let cache = ShaCache::new(dir.path());
            let key = cache.key([("input", b"x".as_slice())]);
            cache.store(&key, &[], b"payload").expect("store");
            key
        };
        let reopened = ShaCache::new(dir.path());
        assert_eq!(
            reopened.get(&key).expect("get").expect("present"),
            b"payload"
        );
    }
}
