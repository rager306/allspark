//! Settings subsystem: typed settings documents, global/project storage,
//! legacy migrations, and the manager.

pub(crate) mod interactive_settings;
pub(crate) mod load;
pub(crate) mod manager;
pub(crate) mod merge;
pub(crate) mod storage;
pub(crate) mod types;

#[cfg(test)]
mod persistence_tests;

pub use manager::{
    DebugLogConfig, IdleEviction, SettingsError, SettingsManager, DEFAULT_IDLE_EVICTION_MINUTES,
    DEFAULT_PROVIDER_CONCURRENCY_LIMIT,
};
pub use storage::{
    FileSettingsStorage, InMemorySettingsStorage, SettingsScope, SettingsStorage, CONFIG_DIR_NAME,
};
pub use types::{
    AgentsSettings, AutoRefineSettings, AutonomousSettings, CompactionSettings, GraphSettings,
    JevAutoThinkingSetting, JevCacheSettings, JevEnabledSetting, JevGraphRerankSetting,
    JevSettings, JevToolArbiterSetting, LspSettings, McpServerConfig, ModelRoutingLevel,
    QueueModeSetting, RoutingSettings, Settings, ThinkingLevelSetting, ToolProfileSettings,
    TransportSetting, UpdateChannel,
};
