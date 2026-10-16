//! S5: the tool auto-registry — a name-indexed table of tool factories with
//! tombstones, parameter aliases, and legacy name mapping.
//!
//! Serena registers tools by reflecting over `Tool` subclasses, keeps a
//! `_deleted_tools` tombstone list so stale configs degrade softly, and maps
//! legacy tool/param names forward. This module ports those contracts onto an
//! explicit registry.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use pa_agent::types::AgentTool;

/// A tool factory: builds a fresh tool instance on demand.
pub type ToolFactory = Arc<dyn Fn() -> Arc<dyn AgentTool> + Send + Sync>;

/// A registration failure.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RegistryError {
    /// A live tool with this name is already registered.
    #[error("tool `{0}` is already registered")]
    Duplicate(String),
    /// The name is tombstoned (removed); re-registration is refused until
    /// the tombstone is lifted.
    #[error("tool `{0}` is tombstoned")]
    Tombstoned(String),
    /// No tool is registered under this name.
    #[error("tool `{0}` is not registered")]
    Unknown(String),
}

/// S5: a name-indexed tool registry.
///
/// The registry owns factories, not instances, so a context can build the
/// exact surface it needs. `_deleted_tools` tombstones keep a stale config
/// (naming a removed tool) from failing loudly; `resolve_alias` and
/// `legacy_name` translate old names forward.
#[derive(Clone, Default)]
pub struct ToolRegistry {
    tools: BTreeMap<String, ToolFactory>,
    tombstones: BTreeSet<String>,
    aliases: BTreeMap<String, String>,
    legacy: BTreeMap<String, String>,
    param_aliases: BTreeMap<String, BTreeMap<String, String>>,
}

impl std::fmt::Debug for ToolRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ToolRegistry")
            .field("tools", &self.tools.keys().collect::<Vec<_>>())
            .field("tombstones", &self.tombstones)
            .field("aliases", &self.aliases)
            .field("legacy", &self.legacy)
            .finish_non_exhaustive()
    }
}

impl ToolRegistry {
    /// An empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The registry pre-seeded with Serena's legacy name mapping
    /// (`replace_regex -> replace_content`) and the `name_path` param alias.
    #[must_use]
    pub fn with_serena_defaults() -> Self {
        let mut registry = Self::new();
        registry.add_legacy_name("replace_regex", "replace_content");
        registry.add_param_alias("find_symbol", "name_path", "name_path_pattern");
        registry
    }

    /// Register a live tool factory.
    ///
    /// # Errors
    ///
    /// Fails on a duplicate name or a tombstoned name.
    pub fn register(
        &mut self,
        name: impl Into<String>,
        factory: ToolFactory,
    ) -> Result<(), RegistryError> {
        let name = name.into();
        if self.tombstones.contains(&name) {
            return Err(RegistryError::Tombstoned(name));
        }
        if self.tools.contains_key(&name) {
            return Err(RegistryError::Duplicate(name));
        }
        self.tools.insert(name, factory);
        Ok(())
    }

    /// Register a tool instance by value (a convenience over [`Self::register`]).
    ///
    /// # Errors
    ///
    /// Fails on a duplicate name or a tombstoned name.
    pub fn register_tool(&mut self, tool: Arc<dyn AgentTool>) -> Result<(), RegistryError> {
        let name = tool.name().to_string();
        self.register(name, Arc::new(move || Arc::clone(&tool)))
    }

    /// Unregister a tool and leave a tombstone so stale configs naming it
    /// degrade softly (Serena `_deleted_tools`).
    ///
    /// # Errors
    ///
    /// Fails if no live tool is registered under this name.
    pub fn unregister(&mut self, name: &str) -> Result<(), RegistryError> {
        if self.tools.remove(name).is_none() {
            return Err(RegistryError::Unknown(name.to_string()));
        }
        self.tombstones.insert(name.to_string());
        Ok(())
    }

    /// Lift a tombstone so the name can be registered again.
    pub fn lift_tombstone(&mut self, name: &str) -> bool {
        self.tombstones.remove(name)
    }

    /// Whether a name is tombstoned.
    #[must_use]
    pub fn is_tombstoned(&self, name: &str) -> bool {
        self.tombstones.contains(name)
    }

    /// The tombstoned names (`_deleted_tools`).
    #[must_use]
    pub fn tombstones(&self) -> Vec<&str> {
        self.tombstones.iter().map(String::as_str).collect()
    }

    /// The live tool names, sorted.
    #[must_use]
    pub fn names(&self) -> Vec<&str> {
        self.tools.keys().map(String::as_str).collect()
    }

    /// Whether a live tool is registered under this name.
    #[must_use]
    pub fn contains(&self, name: &str) -> bool {
        self.tools.contains_key(name)
    }

    /// Build a fresh instance of the named tool.
    ///
    /// # Errors
    ///
    /// Fails if no live tool is registered under this name.
    pub fn instantiate(&self, name: &str) -> Result<Arc<dyn AgentTool>, RegistryError> {
        self.tools
            .get(name)
            .map(|factory| factory())
            .ok_or_else(|| RegistryError::Unknown(name.to_string()))
    }

    /// Register a name alias (`from -> to`).
    pub fn add_alias(&mut self, from: impl Into<String>, to: impl Into<String>) {
        self.aliases.insert(from.into(), to.into());
    }

    /// Resolve an alias chain to its canonical target, returning the input
    /// unchanged when no alias applies. Cycles are broken at the first
    /// repeat.
    #[must_use]
    pub fn resolve_alias<'a>(&'a self, name: &'a str) -> &'a str {
        let mut current = name;
        let mut seen: BTreeSet<&str> = BTreeSet::new();
        while let Some(next) = self.aliases.get(current) {
            if !seen.insert(current) {
                break;
            }
            current = next;
        }
        current
    }

    /// Register a legacy name mapping (`old -> new`).
    pub fn add_legacy_name(&mut self, old: impl Into<String>, new: impl Into<String>) {
        self.legacy.insert(old.into(), new.into());
    }

    /// Map a legacy tool name forward to its current name.
    #[must_use]
    pub fn legacy_name<'a>(&'a self, name: &'a str) -> &'a str {
        self.legacy.get(name).map_or(name, String::as_str)
    }

    /// Register a parameter alias for a tool (`old_param -> new_param`).
    pub fn add_param_alias(
        &mut self,
        tool: impl Into<String>,
        old_param: impl Into<String>,
        new_param: impl Into<String>,
    ) {
        self.param_aliases
            .entry(tool.into())
            .or_default()
            .insert(old_param.into(), new_param.into());
    }

    /// Rewrite legacy parameter keys in a tool-call argument object.
    ///
    /// A key already present under its current name is left untouched.
    #[must_use]
    pub fn apply_param_aliases(&self, tool: &str, args: &serde_json::Value) -> serde_json::Value {
        let Some(aliases) = self.param_aliases.get(tool) else {
            return args.clone();
        };
        let Some(object) = args.as_object() else {
            return args.clone();
        };
        let mut rewritten = serde_json::Map::new();
        for (key, value) in object {
            let canonical = aliases.get(key).map_or(key.as_str(), String::as_str);
            if !object.contains_key(canonical) || canonical == key {
                rewritten.insert(canonical.to_string(), value.clone());
            }
        }
        serde_json::Value::Object(rewritten)
    }
}
