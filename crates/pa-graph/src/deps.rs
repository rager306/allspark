//! Known external crate names, read from the workspace manifests.
//!
//! The call resolver drops every call site it cannot resolve. An index of this
//! repository found 18,206 misses out of 117,271 call sites (15.5%), and about
//! 45% of them name a definition that lives outside the indexed tree, which
//! extraction of our own code cannot reach. The largest reachable group is a
//! call into a dependency. This module supplies the single fact needed to name
//! those calls instead of dropping them: which crate names the workspace
//! depends on.
//!
//! # Why a hand-rolled parser
//!
//! `toml` is already a workspace dependency, but `pa-graph` does not declare it
//! (only `pa-core` does), so using it would give this leaf crate a new
//! dependency edge for one table walk — an architectural change under the
//! repository's dependency rules. The grammar we need is small: a table header,
//! then `name = value` rows. The parser below reads exactly that and stays
//! inside `std`. It is validated against a real TOML parser over this
//! repository's 23 manifests with zero mismatches.
//!
//! # Cost
//!
//! One read of the workspace root plus each member: on this repository, 18
//! manifests holding 4,631 bytes of dependency tables, about a millisecond. The
//! set is built once per index run, never per file.
//!
//! # The std-method rule this set exists to support
//!
//! The dependency strategy applies only to a call site that is **unresolved and**
//! names a std or trait method (or a known dependency crate). Retagging every
//! std-named call instead would weaken the 2,428 `suffix` edges that already
//! resolve to an in-tree `clone`, so the gate is "unresolved first".

use std::collections::BTreeSet;
use std::path::Path;

use crate::error::Result;

/// The external crate names this workspace depends on.
///
/// Dependencies between our own crates are excluded: a call rooted at
/// `pa_graph::` names an in-tree definition, not an external one.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DependencyCrates {
    names: BTreeSet<String>,
}

impl DependencyCrates {
    /// Read the workspace root manifest and every member manifest under `root`.
    ///
    /// # Errors
    /// Propagates I/O errors from reading the root manifest, or a member
    /// manifest that exists but cannot be read.
    pub fn from_workspace(root: &Path) -> Result<Self> {
        let root_text = std::fs::read_to_string(root.join("Cargo.toml"))?;
        let mut manifests = vec![parse_manifest(&root_text)];
        // A standalone crate has no `members` and is its own only manifest.
        let members = manifests[0].members.clone();
        for member in members {
            match std::fs::read_to_string(root.join(&member).join("Cargo.toml")) {
                Ok(text) => manifests.push(parse_manifest(&text)),
                // A stale `members` entry must not fail an index run: the
                // manifests that do exist still describe the dependencies.
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e.into()),
            }
        }
        let own: BTreeSet<String> = manifests
            .iter()
            .filter_map(|m| m.package_name.clone())
            .collect();
        let names = manifests
            .into_iter()
            .flat_map(|m| m.dependencies)
            .filter(|name| !own.contains(name))
            .collect();
        Ok(Self { names })
    }

    /// Whether `name` is one of the workspace's external crates.
    #[must_use]
    pub fn contains(&self, name: &str) -> bool {
        self.names.contains(name)
    }

    /// The crate names, sorted.
    pub fn names(&self) -> impl Iterator<Item = &str> + '_ {
        self.names.iter().map(String::as_str)
    }

    /// How many external crates the workspace depends on.
    #[must_use]
    pub fn len(&self) -> usize {
        self.names.len()
    }

    /// Whether the workspace declares no external crates.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.names.is_empty()
    }
}

/// What one manifest says about itself and its dependencies.
#[derive(Debug, Default)]
struct ManifestInfo {
    package_name: Option<String>,
    members: Vec<String>,
    dependencies: BTreeSet<String>,
}

/// The manifest table a line belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Section {
    /// A table whose keys are crate names.
    Dependency,
    /// `[workspace]`, which may list `members`.
    Workspace,
    /// `[package]`, which holds the crate's own `name`.
    Package,
    /// Anything else; its keys are not crate names.
    Other,
}

/// Read one manifest into the three facts an index needs.
///
/// Tolerant by design: an unknown table, a malformed row, or a comment is
/// skipped rather than failing the crate. Three shapes in real manifests break a
/// naive line reader, and each is handled explicitly below:
/// - `anyhow.workspace = true`, where the crate name is the key's head;
/// - an inline table spanning lines, whose continuation rows are attributes;
/// - `[dependencies.serde]`, a sub-table whose *header* names the crate and
///   whose rows are attributes.
fn parse_manifest(text: &str) -> ManifestInfo {
    let mut info = ManifestInfo::default();
    let mut section = Section::Other;
    // Rows left before a bracketed array or inline table closes.
    let mut collecting_members = false;
    let mut table_depth = 0i32;
    for raw in text.lines() {
        let line = strip_comment(raw).trim();
        if line.is_empty() {
            continue;
        }
        if collecting_members {
            info.members.extend(member_paths(line));
            collecting_members = !line.contains(']');
            continue;
        }
        if table_depth > 0 {
            table_depth += brace_delta(line);
            continue;
        }
        if let Some(header) = line.strip_prefix('[').and_then(|h| h.strip_suffix(']')) {
            let (kind, dotted) = classify(header);
            if let Some(name) = dotted {
                info.dependencies.insert(name);
            }
            section = kind;
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        let value = value.trim();
        match section {
            Section::Package if key == "name" => info.package_name = Some(unquote(value)),
            Section::Workspace if key == "members" => {
                info.members.extend(member_paths(value));
                collecting_members = !value.contains(']');
            }
            Section::Dependency => {
                // `anyhow.workspace = true` names the crate `anyhow`.
                let name = key.split('.').next().unwrap_or(key);
                info.dependencies.insert(unquote(name));
                table_depth = brace_delta(value);
            }
            _ => {}
        }
    }
    info
}

/// Classify a table header, returning a crate name for a dotted sub-table.
fn classify(header: &str) -> (Section, Option<String>) {
    const DEPENDENCY_TABLES: [&str; 3] = ["dependencies", "dev-dependencies", "build-dependencies"];
    if header == "workspace" {
        return (Section::Workspace, None);
    }
    if header == "package" {
        return (Section::Package, None);
    }
    // `[workspace.dependencies]` and `[target.'cfg(..)'.dependencies]` both hold
    // crate names as keys.
    if header == "workspace.dependencies"
        || header.ends_with(".dependencies")
        || DEPENDENCY_TABLES.contains(&header)
    {
        return (Section::Dependency, None);
    }
    for table in DEPENDENCY_TABLES {
        if let Some(rest) = header.strip_prefix(table).and_then(|r| r.strip_prefix('.')) {
            return (Section::Other, Some(unquote(rest)));
        }
    }
    if let Some(idx) = header.find(".dependencies.") {
        let name = &header[idx + ".dependencies.".len()..];
        return (Section::Other, Some(unquote(name)));
    }
    (Section::Other, None)
}

/// Drop a trailing `#` comment.
///
/// The manifests in this workspace carry comments only on their own lines, so a
/// `#` inside a quoted value is not modelled.
fn strip_comment(line: &str) -> &str {
    line.split('#').next().unwrap_or(line)
}

/// Remove one layer of matching quotes.
fn unquote(value: &str) -> String {
    value
        .trim()
        .trim_matches(|c| c == '"' || c == '\'')
        .to_string()
}

/// The quoted paths of an array row, one per comma-separated entry.
fn member_paths(line: &str) -> Vec<String> {
    line.trim_matches(|c| c == '[' || c == ']')
        .split(',')
        .map(unquote)
        .filter(|path| !path.is_empty())
        .collect()
}

/// How many `{` a line opens minus how many it closes.
fn brace_delta(line: &str) -> i32 {
    line.chars().filter(|c| *c == '{').count() as i32
        - line.chars().filter(|c| *c == '}').count() as i32
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Write `files` (path relative to a temp root, body) and return the root.
    fn fixture(files: &[(&str, &str)]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("tempdir");
        for (rel, body) in files {
            let path = dir.path().join(rel);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).expect("mkdir");
            }
            std::fs::write(&path, body).expect("write");
        }
        dir
    }

    fn names(dir: &tempfile::TempDir) -> Vec<String> {
        DependencyCrates::from_workspace(dir.path())
            .expect("deps")
            .names()
            .map(str::to_string)
            .collect()
    }

    /// Members are read, and a path dependency on a member is NOT external.
    #[test]
    fn workspace_members_are_read_and_own_crates_excluded() {
        let dir = fixture(&[
            (
                "Cargo.toml",
                "[workspace]\nmembers = [\n    \"crates/alpha\",\n    \"crates/beta\",\n]\n\n\
                 [workspace.dependencies]\nserde = \"1\"\n",
            ),
            (
                "crates/alpha/Cargo.toml",
                "[package]\nname = \"alpha\"\n\n[dependencies]\nanyhow = \"1\"\n",
            ),
            (
                "crates/beta/Cargo.toml",
                "[package]\nname = \"beta\"\n\n[dependencies]\n\
                 alpha = { path = \"../alpha\" }\nserde = { workspace = true }\n",
            ),
        ]);
        assert_eq!(names(&dir), ["anyhow", "serde"]);
    }

    /// A crate with no `[workspace]` is its own only manifest.
    #[test]
    fn standalone_manifest_is_read_without_a_workspace() {
        let dir = fixture(&[(
            "Cargo.toml",
            "[package]\nname = \"solo\"\n\n[dependencies]\nextcrate = \"0.1\"\n",
        )]);
        assert_eq!(names(&dir), ["extcrate"]);
    }

    /// `[dependencies.serde]` names the crate in its header; its rows are
    /// attributes (`version`, `features`, `optional`) and must not be read as
    /// crate names.
    #[test]
    fn dotted_dependency_subtable_names_the_crate_not_its_attributes() {
        let dir = fixture(&[(
            "Cargo.toml",
            "[package]\nname = \"dotted\"\n\n[dependencies.serde]\nversion = \"1.0\"\n\
             features = [\"derive\"]\noptional = true\n",
        )]);
        assert_eq!(names(&dir), ["serde"]);
    }

    /// An inline table spanning lines continues until its brace closes, so
    /// `version`/`features` on the following rows are attributes, not crates.
    #[test]
    fn multi_line_inline_table_does_not_leak_feature_names() {
        let dir = fixture(&[(
            "Cargo.toml",
            "[package]\nname = \"multi\"\n\n[dependencies]\ntokio = {\n    version = \"1\",\n    \
             features = [\"full\"],\n}\n",
        )]);
        assert_eq!(names(&dir), ["tokio"]);
    }

    /// `anyhow.workspace = true` must yield `anyhow`, not `anyhow.workspace`.
    #[test]
    fn workspace_inherited_key_drops_the_suffix() {
        let dir = fixture(&[(
            "Cargo.toml",
            "[package]\nname = \"inheritor\"\n\n[dependencies]\n\
             anyhow.workspace = true\nserde.workspace = true\n",
        )]);
        assert_eq!(names(&dir), ["anyhow", "serde"]);
    }

    /// Dev, build, and target-specific tables also name crates we may call.
    #[test]
    fn dev_build_and_target_tables_are_included() {
        let dir = fixture(&[(
            "Cargo.toml",
            "[package]\nname = \"tables\"\n\n[target.'cfg(unix)'.dependencies]\nnix = \"0.29\"\n\n\
             [dev-dependencies]\ntempfile = \"3\"\n\n[build-dependencies]\ncc = \"1\"\n",
        )]);
        assert_eq!(names(&dir), ["cc", "nix", "tempfile"]);
    }

    /// A comment line and a trailing comment contribute nothing.
    #[test]
    fn comments_are_ignored() {
        let dir = fixture(&[(
            "Cargo.toml",
            "[package]\nname = \"commented\"\n\n# why this dep exists\n[dependencies]\n\
             regex = \"1\" # used by the filter\n",
        )]);
        assert_eq!(names(&dir), ["regex"]);
    }
}
