//! The fork's native-tool gates (lsp, graph, edit-hybrid) and routing.

use anyhow::Result;

use crate::settings::manager::SettingsManager;

impl SettingsManager {
    /// The native `lsp` tool's gate: unset means ON (the fork wires the tool
    /// by default; `lsp.enabled = false` opts out).
    #[must_use]
    pub fn get_lsp_enabled(&self) -> bool {
        self.merged
            .lsp
            .as_ref()
            .and_then(|lsp| lsp.enabled)
            .unwrap_or(true)
    }

    /// The native `lsp` tool's idle bound for a live language-server client
    /// (`lsp.idleTimeoutMs`).
    ///
    /// `None` — the default — keeps a client for the whole session, which is
    /// what every build did before this setting had a reader. `Some(0)` is
    /// treated as unset rather than "evict immediately", so a zero cannot
    /// silently turn a server into a spawn-per-request loop.
    #[must_use]
    pub fn get_lsp_idle_timeout(&self) -> Option<std::time::Duration> {
        let millis = self.merged.lsp.as_ref()?.idle_timeout_ms?;
        (millis > 0).then(|| std::time::Duration::from_millis(millis))
    }

    /// Per-server LSP overrides (`lsp.servers`), keyed by server name.
    ///
    /// The setting was documented and parsed but read by nobody (review #22), so an operator
    /// who overrode a server's command saw no effect at all. It is surfaced as an owned map
    /// rather than wired into a launch path this crate does not own: `pa-lsp` builds its server
    /// table, and `None` (unset) must stay distinguishable from an empty map — the first means
    /// "use the built-in table", the second means "the operator overrode everything".
    #[must_use]
    pub fn get_lsp_servers(&self) -> Option<serde_json::Map<String, serde_json::Value>> {
        self.merged.lsp.as_ref()?.servers.clone()
    }

    /// The native `graph` tool's gate: unset means ON.
    #[must_use]
    pub fn get_graph_enabled(&self) -> bool {
        self.merged
            .graph
            .as_ref()
            .and_then(|graph| graph.enabled)
            .unwrap_or(true)
    }

    /// [FORK-ONLY] The model router's level (`routing.mode`).
    ///
    /// Unset and unrecognized spellings read as [`ModelRoutingLevel::Off`],
    /// the pre-router behaviour byte for byte: a typo disables the router
    /// instead of silently enabling a selector nobody asked for.
    #[must_use]
    pub fn get_routing_level(&self) -> crate::settings::types::ModelRoutingLevel {
        self.merged
            .routing
            .as_ref()
            .and_then(|routing| routing.mode.as_deref())
            .map_or(
                crate::settings::types::ModelRoutingLevel::Off,
                crate::settings::types::ModelRoutingLevel::parse,
            )
    }

    /// Persist the router's level to the GLOBAL scope: the policy is the
    /// user's, never a project's (an untrusted checkout must not reorder
    /// which provider serves its prompts).
    ///
    /// # Errors
    ///
    /// Returns an error when the global settings scope cannot be saved.
    pub fn set_routing_level(
        &mut self,
        level: crate::settings::types::ModelRoutingLevel,
    ) -> Result<()> {
        self.global_mut()
            .routing
            .get_or_insert_with(Default::default)
            .mode = Some(level.as_str().to_string());
        self.save_global_scope(&["routing.mode"])
    }

    /// The graph's background build/refresh policy (`graph.auto`).
    ///
    /// Unset reads as [`GraphAuto::Auto`]: the first query builds on demand and
    /// a background task refreshes while the session works. `watch` is accepted and
    /// behaves like `auto` (its filesystem watcher is not wired into a session);
    /// `off` leaves only the explicit `index` action.
    #[must_use]
    pub fn get_graph_auto(&self) -> crate::settings::types::GraphAuto {
        self.merged
            .graph
            .as_ref()
            .and_then(|graph| graph.auto)
            .unwrap_or_default()
    }

    /// The code-graph store path (`graph.dbPath`), resolved against `~/`
    /// when prefixed; `None` lets the caller default to `<agentDir>/graph.db`.
    #[must_use]
    pub fn get_graph_db_path(&self) -> Option<std::path::PathBuf> {
        let raw = self.merged.graph.as_ref()?.db_path.as_ref()?;
        let raw = raw.trim();
        if raw.is_empty() {
            return None;
        }
        if raw == "~" {
            return pa_types::platform::home_dir();
        }
        if let Some(rest) = raw.strip_prefix("~/") {
            return pa_types::platform::home_dir().map(|home| home.join(rest));
        }
        Some(std::path::PathBuf::from(raw))
    }

    /// The agent-facing `schedule` tool's gate: unset means OFF.
    ///
    /// The other fork tools default ON because they only read or edit the
    /// working tree. This one is the only tool that writes to the user's
    /// DURABLE cron catalog — the same store `schedule add` writes — so an
    /// agent may create work that outlives its session without being asked.
    /// Opt-in keeps that decision with the operator.
    #[must_use]
    pub fn get_schedule_enabled(&self) -> bool {
        self.merged
            .schedule
            .as_ref()
            .and_then(|schedule| schedule.enabled)
            .unwrap_or(false)
    }

    /// The native `edit` tool's gate: unset means ON.
    #[must_use]
    pub fn get_edit_hybrid_enabled(&self) -> bool {
        self.merged
            .edit_hybrid
            .as_ref()
            .and_then(|edit| edit.enabled)
            .unwrap_or(true)
    }

    /// The native `edit` tool's default mode (`editHybrid.defaultMode`),
    /// unset meaning `replace` (the caller applies the fallback).
    #[must_use]
    pub fn get_edit_hybrid_default_mode(&self) -> Option<String> {
        self.merged
            .edit_hybrid
            .as_ref()
            .and_then(|edit| edit.default_mode.clone())
            .map(|mode| mode.trim().to_ascii_lowercase())
            .filter(|mode| !mode.is_empty())
    }

    /// The native `edit` tool's requestable modes
    /// (`editHybrid.allowModes`); `None`/empty means all modes.
    #[must_use]
    pub fn get_edit_hybrid_allow_modes(&self) -> Vec<String> {
        self.merged
            .edit_hybrid
            .as_ref()
            .and_then(|edit| edit.allow_modes.clone())
            .unwrap_or_default()
            .into_iter()
            .map(|mode| mode.trim().to_ascii_lowercase())
            .filter(|mode| !mode.is_empty())
            .collect()
    }
}
