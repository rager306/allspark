//! Language-server table: which binary to spawn for which file extension, and
//! where the project root sits.
//!
//! FORK-ONLY (allspark): the shape mirrors oh-my-pi's `lsp/defaults.json`
//! (command / args / fileTypes / rootMarkers) but stays a small Rust const set
//! so the fork carries no JSON asset and no schema surface upstream must merge.
//! Extend the table rather than branching the spawn path.

/// One language-server entry.
///
/// Owned strings rather than `&'static str`: `lsp.servers` lets an operator override a
/// built-in entry (`command`, `args`, `extensions`, `rootMarkers`), so the resolved table
/// is built at session start instead of being a compile-time constant. The built-in
/// `SERVERS` table stays static and is copied into that owned form on demand.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerConfig {
    /// Stable name used in `lsp(action="status")` and error text.
    pub name: String,
    /// Executable to spawn (resolved on `PATH`).
    pub command: String,
    /// Fixed argv passed before the transport arguments.
    pub args: Vec<String>,
    /// File extensions this server owns, each with a leading dot.
    pub extensions: Vec<String>,
    /// Marker files that identify a project root, nearest-ancestor wins.
    pub root_markers: Vec<String>,
    /// The `languageId` sent in `textDocument/didOpen`.
    pub language_id: String,
}

/// The built-in server table. Order is the tie-break for ambiguous extensions
/// (first match wins), so keep the more specific entries first.
#[must_use]
pub fn builtin_servers() -> Vec<ServerConfig> {
    vec![
        ServerConfig {
            name: "rust-analyzer".to_string(),
            command: "rust-analyzer".to_string(),
            args: vec![],
            extensions: vec![".rs".to_string()],
            root_markers: vec!["Cargo.toml".to_string()],
            language_id: "rust".to_string(),
        },
        ServerConfig {
            name: "typescript-language-server".to_string(),
            command: "typescript-language-server".to_string(),
            args: vec!["--stdio".to_string()],
            extensions: vec![
                ".ts".to_string(),
                ".tsx".to_string(),
                ".js".to_string(),
                ".jsx".to_string(),
                ".mjs".to_string(),
                ".cjs".to_string(),
            ],
            root_markers: vec![
                "package.json".to_string(),
                "tsconfig.json".to_string(),
                "jsconfig.json".to_string(),
            ],
            language_id: "typescript".to_string(),
        },
        ServerConfig {
            name: "gopls".to_string(),
            command: "gopls".to_string(),
            args: vec!["serve".to_string()],
            extensions: vec![".go".to_string()],
            root_markers: vec!["go.mod".to_string(), "go.work".to_string()],
            language_id: "go".to_string(),
        },
        ServerConfig {
            name: "pyright".to_string(),
            command: "pyright-langserver".to_string(),
            args: vec!["--stdio".to_string()],
            extensions: vec![".py".to_string(), ".pyi".to_string()],
            root_markers: vec![
                "pyproject.toml".to_string(),
                "setup.py".to_string(),
                "setup.cfg".to_string(),
                "requirements.txt".to_string(),
            ],
            language_id: "python".to_string(),
        },
        ServerConfig {
            name: "clangd".to_string(),
            command: "clangd".to_string(),
            args: vec!["--background-index".to_string()],
            extensions: vec![
                ".c".to_string(),
                ".cc".to_string(),
                ".cpp".to_string(),
                ".cxx".to_string(),
                ".h".to_string(),
                ".hpp".to_string(),
                ".hxx".to_string(),
            ],
            root_markers: vec![
                "compile_commands.json".to_string(),
                "CMakeLists.txt".to_string(),
                ".clangd".to_string(),
            ],
            language_id: "cpp".to_string(),
        },
        ServerConfig {
            name: "bash-language-server".to_string(),
            command: "bash-language-server".to_string(),
            args: vec!["start".to_string()],
            extensions: vec![".sh".to_string(), ".bash".to_string()],
            root_markers: vec![".git".to_string()],
            language_id: "shellscript".to_string(),
        },
    ]
}

/// The server owning `path` within `table`, or `None` when no entry matches.
#[must_use]
pub fn server_in<'a>(
    table: &'a [ServerConfig],
    path: &std::path::Path,
) -> Option<&'a ServerConfig> {
    let ext = path.extension().and_then(std::ffi::OsStr::to_str)?;
    let dotted = format!(".{ext}");
    table
        .iter()
        .find(|server| server.extensions.iter().any(|owned| owned == &dotted))
}

/// Resolve the operator's `lsp.servers` overrides onto the built-in table.
///
/// The setting is a name-keyed map; a key that names a built-in entry changes that
/// entry's `command`/`args`/`extensions`/`rootMarkers`/`languageId` (Serena's
/// `defaults.json` override shape), and a key that names nothing adds a new server.
/// Values are plain strings or arrays of strings; anything else is ignored rather than
/// guessed at, so a typo cannot silently spawn the wrong binary.
#[must_use]
pub fn resolve_servers(
    overrides: Option<&serde_json::Map<String, serde_json::Value>>,
) -> Vec<ServerConfig> {
    let mut table = builtin_servers();
    let Some(overrides) = overrides else {
        return table;
    };
    for (name, value) in overrides {
        let Some(object) = value.as_object() else {
            continue;
        };
        let mut entry = table
            .iter()
            .position(|server| &server.name == name)
            .map_or_else(
                || ServerConfig {
                    name: name.clone(),
                    command: name.clone(),
                    args: Vec::new(),
                    extensions: Vec::new(),
                    root_markers: Vec::new(),
                    language_id: name.clone(),
                },
                |index| table[index].clone(),
            );
        let strings = |field: &str| -> Option<Vec<String>> {
            let raw = object.get(field)?;
            if let Some(text) = raw.as_str() {
                return Some(vec![text.to_string()]);
            }
            raw.as_array().map(|items| {
                items
                    .iter()
                    .filter_map(|item| item.as_str().map(str::to_string))
                    .collect()
            })
        };
        if let Some(command) = object.get("command").and_then(|raw| raw.as_str()) {
            entry.command = command.to_string();
        }
        if let Some(args) = strings("args") {
            entry.args = args;
        }
        if let Some(extensions) = strings("extensions") {
            // A bare `ts` is meant as `.ts`; the matcher compares a leading-dot form.
            entry.extensions = extensions
                .into_iter()
                .map(|ext| {
                    if ext.starts_with('.') {
                        ext
                    } else {
                        format!(".{ext}")
                    }
                })
                .collect();
        }
        if let Some(markers) = strings("rootMarkers") {
            entry.root_markers = markers;
        }
        if let Some(language) = object.get("languageId").and_then(|raw| raw.as_str()) {
            entry.language_id = language.to_string();
        }
        match table.iter().position(|server| server.name == entry.name) {
            Some(index) => table[index] = entry,
            None => table.push(entry),
        }
    }
    table
}

/// The server owning `path` in the built-in table.
#[must_use]
pub fn server_for_path(path: &std::path::Path) -> Option<ServerConfig> {
    let table = builtin_servers();
    server_in(&table, path).cloned()
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
            Some("rust-analyzer".to_string())
        );
        assert_eq!(
            server_for_path(Path::new("a/b.tsx")).map(|s| s.name),
            Some("typescript-language-server".to_string())
        );
        assert_eq!(server_for_path(Path::new("README.md")), None);
    }

    /// `lsp.servers` (review #22) was parsed and read by nobody, so an operator who
    /// pointed a server at a different binary saw no effect. It now resolves onto the
    /// built-in table, which is what decides the spawn.
    #[test]
    fn overrides_replace_a_builtin_command_and_add_a_new_server() {
        let mut overrides = serde_json::Map::new();
        overrides.insert(
            "rust-analyzer".to_string(),
            serde_json::json!({ "command": "/opt/ra/bin/rust-analyzer", "args": ["--log-file", "/tmp/ra.log"] }),
        );
        overrides.insert(
            "zls".to_string(),
            serde_json::json!({ "command": "zls", "extensions": ["zig"], "languageId": "zig" }),
        );
        let table = resolve_servers(Some(&overrides));
        let ra = table
            .iter()
            .find(|server| server.name == "rust-analyzer")
            .expect("builtin survives");
        assert_eq!(ra.command, "/opt/ra/bin/rust-analyzer");
        assert_eq!(ra.args, ["--log-file", "/tmp/ra.log"]);
        // An untouched field keeps the built-in value: an override is a patch, not a rewrite.
        assert_eq!(ra.language_id, "rust");
        let zls = table
            .iter()
            .find(|server| server.name == "zls")
            .expect("new server added");
        // A bare `zig` is matched against the leading-dot form.
        assert_eq!(zls.extensions, [".zig"]);
        assert_eq!(
            server_in(&table, Path::new("src/main.zig")).map(|server| server.name.clone()),
            Some("zls".to_string())
        );
        // With no overrides the table is exactly the built-ins.
        assert_eq!(resolve_servers(None).len(), builtin_servers().len());
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
        let found = project_root(&file, &server);
        assert_eq!(found, root, "nearest Cargo.toml ancestor wins");
    }
}
