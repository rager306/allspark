//! Content hashing for incremental file classification.

use std::fs::Metadata;
use std::io::Read;
use std::path::Path;

use sha2::{Digest, Sha256};

use crate::error::Result;

/// A file's content hashes plus cheap metadata, as stored in `file_hashes`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FileHash {
    /// SHA-256 of the file bytes, lowercase hex.
    pub sha256: String,
    /// BLAKE3 of the file bytes, lowercase hex (fast re-check).
    pub blake3: String,
    /// Modification time in nanoseconds since the Unix epoch.
    pub mtime_ns: i64,
    /// File size in bytes.
    pub size: i64,
}

/// Compute both content hashes for `bytes` and merge in `metadata`.
///
/// SHA-256 is the classification key (stable across toolchains); BLAKE3 is the
/// fast parallel hash the watcher can re-check without reading the file twice.
#[must_use]
pub fn hash_bytes(bytes: &[u8], meta: &Metadata) -> FileHash {
    let sha256 = hex(&Sha256::digest(bytes));
    let blake3 = blake3::hash(bytes).to_hex().to_string();
    FileHash {
        sha256,
        blake3,
        mtime_ns: mtime_ns(meta),
        size: meta.len() as i64,
    }
}

/// Read `path` and hash it.
///
/// # Errors
/// Propagates I/O errors from `std::fs`.
pub fn hash_file(path: &Path) -> Result<FileHash> {
    let meta = std::fs::metadata(path)?;
    let mut file = std::fs::File::open(path)?;
    let mut buf = Vec::with_capacity(meta.len().min(4 << 20) as usize);
    file.read_to_end(&mut buf)?;
    Ok(hash_bytes(&buf, &meta))
}

fn hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0x0f) as usize] as char);
    }
    out
}

fn mtime_ns(meta: &Metadata) -> i64 {
    use std::time::UNIX_EPOCH;
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .and_then(|d| i64::try_from(d.as_nanos()).ok())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hashes_are_stable_and_differing() {
        let dir = tempfile::tempdir().expect("tempdir");
        let a = dir.path().join("a.txt");
        let b = dir.path().join("b.txt");
        std::fs::write(&a, b"hello world").expect("write a");
        std::fs::write(&b, b"hello worlds").expect("write b");
        let ha = hash_file(&a).expect("hash a");
        let hb = hash_file(&b).expect("hash b");
        assert_eq!(ha.sha256.len(), 64);
        assert_eq!(ha.blake3.len(), 64);
        assert_eq!(ha.size, 11);
        assert_ne!(ha.sha256, hb.sha256);
        // Re-hashing the same file is deterministic.
        assert_eq!(ha.sha256, hash_file(&a).expect("rehash").sha256);
    }
}
