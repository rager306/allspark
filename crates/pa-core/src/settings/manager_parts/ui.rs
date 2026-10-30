//! Presentation settings: theme, tree filter, detail level, session dir.

use anyhow::Result;

use crate::settings::manager::{DebugLogConfig, SettingsManager};
use crate::settings::types::{QueueModeSetting, ThinkingLevelSetting, UpdateChannel};

impl SettingsManager {
    /// `markdown.codeBlockIndent`: the string the chat markdown renderer
    /// indents fenced code blocks by (TS default: two spaces).
    #[must_use]
    pub fn get_code_block_indent(&self) -> String {
        self.settings()
            .markdown
            .as_ref()
            .and_then(|markdown| markdown.code_block_indent.clone())
            .unwrap_or_else(|| "  ".to_string())
    }

    /// `terminal.fullscreenMouse`: whether the fullscreen transcript
    /// surface enables mouse tracking and wheel scrolling.
    #[must_use]
    pub fn get_fullscreen_mouse(&self) -> bool {
        self.settings()
            .terminal
            .as_ref()
            .and_then(|terminal| terminal.fullscreen_mouse)
            .unwrap_or(true)
    }

    /// `terminal.showImages`: whether image blocks in tool results render
    /// their type/dimension metadata rows (TS default: true).
    #[must_use]
    pub fn get_show_images(&self) -> bool {
        self.settings()
            .terminal
            .as_ref()
            .and_then(|terminal| terminal.show_images)
            .unwrap_or(true)
    }

    /// `treeFilterMode`: the `/tree` selector's initial filter; an unset
    /// or invalid value falls back to `user-only` (the TS default).
    #[must_use]
    pub fn get_tree_filter_mode(&self) -> String {
        let mode = self.settings().tree_filter_mode.clone().unwrap_or_default();
        let valid = ["default", "no-tools", "user-only", "labeled-only", "all"];
        if valid.contains(&mode.as_str()) && !mode.is_empty() {
            mode
        } else {
            "user-only".to_string()
        }
    }

    /// `chatDetail`: the conversation-detail level the chat starts at; an unset or
    /// invalid value falls back to `overview` (operator directive 2026-09-28).
    #[must_use]
    pub fn get_chat_detail(&self) -> String {
        match self.settings().chat_detail.as_deref() {
            Some("details") => "details",
            Some("all") => "all",
            _ => "overview",
        }
        .to_string()
    }

    #[must_use]
    pub fn get_branch_summary_skip_prompt(&self) -> bool {
        self.settings()
            .branch_summary
            .as_ref()
            .and_then(|branch_summary| branch_summary.skip_prompt)
            .unwrap_or(false)
    }

    /// # Errors
    ///
    /// Returns an error when the global settings file cannot be written.
    pub fn set_steering_mode(&mut self, mode: QueueModeSetting) -> Result<()> {
        self.global.steering_mode = Some(mode);
        self.save_global(&["steeringMode"])
    }

    /// # Errors
    ///
    /// Returns an error when the global settings file cannot be written.
    pub fn set_follow_up_mode(&mut self, mode: QueueModeSetting) -> Result<()> {
        self.global.follow_up_mode = Some(mode);
        self.save_global(&["followUpMode"])
    }

    /// # Errors
    ///
    /// Returns an error when the global settings file cannot be written.
    pub fn set_theme(&mut self, theme: String) -> Result<()> {
        self.global.theme = Some(theme);
        self.save_global(&["theme"])
    }

    /// # Errors
    ///
    /// Returns an error when the global settings file cannot be written.
    pub fn set_update_channel(&mut self, channel: UpdateChannel) -> Result<()> {
        self.global.update_channel = Some(channel);
        self.save_global(&["updateChannel"])
    }

    /// # Errors
    ///
    /// Returns an error when the global settings file cannot be written.
    pub fn set_default_thinking_level(&mut self, level: ThinkingLevelSetting) -> Result<()> {
        self.global.default_thinking_level = Some(level);
        self.save_global(&["defaultThinkingLevel"])
    }

    /// The steering queue's delivery mode: `all` co-delivers every queued message at
    /// the next turn boundary; `one-at-a-time` delivers one per turn.
    #[must_use]
    pub fn get_steering_mode(&self) -> QueueModeSetting {
        self.merged.steering_mode.unwrap_or(QueueModeSetting::All)
    }

    #[must_use]
    pub fn get_follow_up_mode(&self) -> QueueModeSetting {
        self.merged
            .follow_up_mode
            .unwrap_or(QueueModeSetting::OneAtATime)
    }

    #[must_use]
    pub fn get_theme(&self) -> Option<&str> {
        self.merged.theme.as_deref()
    }

    /// Global-only read of the two known values; anything else is unset.
    #[must_use]
    pub fn get_update_channel(&self) -> Option<UpdateChannel> {
        self.global.update_channel
    }

    #[must_use]
    pub fn get_default_thinking_level(&self) -> Option<ThinkingLevelSetting> {
        self.merged.default_thinking_level
    }

    #[must_use]
    pub fn get_session_dir(&self) -> Option<std::path::PathBuf> {
        let session_dir = self.merged.session_dir.as_ref()?;
        let home = pa_types::platform::home_dir()?;
        Some(if session_dir == "~" {
            home
        } else if let Some(rest) = session_dir.strip_prefix("~/") {
            home.join(rest)
        } else {
            session_dir.into()
        })
    }

    // -- persistence ---------------------------------------------------------

    /// FORK-ONLY: the resolved rotating debug-log configuration. `enabled`
    /// is the `debugLog.enabled` setting or the `ALLSPARK_DEBUG=1` env; an
    /// unknown `level` reads as `debug`.
    #[must_use]
    pub fn get_debug_log(&self) -> DebugLogConfig {
        let cfg = self.merged.debug_log.as_ref();
        let enabled = cfg.and_then(|d| d.enabled).unwrap_or(false)
            || std::env::var("ALLSPARK_DEBUG").is_ok_and(|v| v == "1");
        let raw_level = cfg
            .and_then(|d| d.level.clone())
            .unwrap_or_else(|| "debug".to_string());
        let level = matches!(
            raw_level.as_str(),
            "trace" | "debug" | "info" | "warn" | "error"
        )
        .then_some(raw_level)
        .unwrap_or_else(|| "debug".to_string());
        DebugLogConfig {
            enabled,
            level,
            max_files: cfg.and_then(|d| d.max_files).unwrap_or(10).max(1),
            max_bytes: cfg
                .and_then(|d| d.max_bytes)
                .unwrap_or(10 * 1024 * 1024)
                .max(1024),
        }
    }
}
