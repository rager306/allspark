//! Language-server table: which binary to spawn for which file extension, and
//! where the project root sits.
//!
//! FORK-ONLY (allspark): the shape mirrors oh-my-pi's `lsp/defaults.json`
//! (command / args / fileTypes / rootMarkers) but stays a small Rust const set
//! so the fork carries no JSON asset and no schema surface upstream must merge.
//! Extend the table rather than branching the spawn path.

/// One language-server entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ServerConfig {
    /// Stable name used in `lsp(action="status")` and error text.
    pub name: &'static str,
    /// Executable to spawn (resolved on `PATH`).
    pub command: &'static str,
    /// Fixed argv passed before the transport arguments.
    pub args: &'static [&'static str],
    /// File extensions this server owns, each with a leading dot.
    pub extensions: &'static [&'static str],
    /// Marker files that identify a project root, nearest-ancestor wins.
    pub root_markers: &'static [&'static str],
    /// The `languageId` sent in `textDocument/didOpen`.
    pub language_id: &'static str,
}

/// The built-in server table. Order is the tie-break for ambiguous extensions
/// (first match wins), so keep the more specific entries first.
pub const SERVERS: &[ServerConfig] = &[
    ServerConfig {
        name: "rust-analyzer",
        command: "rust-analyzer",
        args: &[],
        extensions: &[".rs"],
        root_markers: &["Cargo.toml"],
        language_id: "rust",
    },
    ServerConfig {
        name: "typescript-language-server",
        command: "typescript-language-server",
        args: &["--stdio"],
        extensions: &[".ts", ".tsx", ".js", ".jsx", ".mjs", ".cjs"],
        root_markers: &["package.json", "tsconfig.json", "jsconfig.json"],
        language_id: "typescript",
    },
    ServerConfig {
        name: "gopls",
        command: "gopls",
        args: &["serve"],
        extensions: &[".go"],
        root_markers: &["go.mod", "go.work"],
        language_id: "go",
    },
    ServerConfig {
        name: "pyright",
        command: "pyright-langserver",
        args: &["--stdio"],
        extensions: &[".py", ".pyi"],
        root_markers: &[
            "pyproject.toml",
            "setup.py",
            "setup.cfg",
            "requirements.txt",
        ],
        language_id: "python",
    },
    ServerConfig {
        name: "clangd",
        command: "clangd",
        args: &["--background-index"],
        extensions: &[".c", ".cc", ".cpp", ".cxx", ".h", ".hpp", ".hxx"],
        root_markers: &["compile_commands.json", "CMakeLists.txt", ".clangd"],
        language_id: "cpp",
    },
    ServerConfig {
        name: "bash-language-server",
        command: "bash-language-server",
        args: &["start"],
        extensions: &[".sh", ".bash"],
        root_markers: &[".git"],
        language_id: "shellscript",
    },
];

/// The server owning `path`, or `None` when no table entry matches.
#[must_use]
pub fn server_for_path(path: &std::path::Path) -> Option<&'static ServerConfig> {
    let ext = path.extension().and_then(std::ffi::OsStr::to_str)?;
    let dotted = format!(".{ext}");
    SERVERS
        .iter()
        .find(|server| server.extensions.contains(&dotted.as_str()))
}

/// Walk from `file`'s directory upward to the nearest ancestor holding one of
/// `server.root_markers`; fall back to `file`'s own directory.
#[must_use]
pub fn project_root(file: &std::path::Path, server: &ServerConfig) -> std::path::PathBuf {
    let start = if file.is_dir() {
        file.to_path_buf()
    } else {
        file.parent().map_or_else(
            || std::path::PathBuf::from("."),
            std::path::Path::to_path_buf,
        )
    };
    for dir in start.ancestors() {
        if server
            .root_markers
            .iter()
            .any(|marker| dir.join(marker).exists())
        {
            return dir.to_path_buf();
        }
    }
    start
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn extension_picks_the_owning_server() {
        assert_eq!(
            server_for_path(Path::new("src/lib.rs")).map(|s| s.name),
            Some("rust-analyzer")
        );
        assert_eq!(
            server_for_path(Path::new("a/b.tsx")).map(|s| s.name),
            Some("typescript-language-server")
        );
        assert_eq!(server_for_path(Path::new("README.md")), None);
    }

    #[test]
    fn project_root_walks_to_the_nearest_marker() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::write(root.join("Cargo.toml"), "[workspace]").unwrap();
        let nested = root.join("crates/pa-lsp/src");
        std::fs::create_dir_all(&nested).unwrap();
        let file = nested.join("lib.rs");
        std::fs::write(&file, "").unwrap();

        let server = server_for_path(&file).unwrap();
        let found = project_root(&file, server);
        assert_eq!(found, root, "nearest Cargo.toml ancestor wins");
    }
}
