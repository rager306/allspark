//! The telemetry gate and its one-time notice.

use anyhow::Result;

use crate::settings::manager::SettingsManager;

impl SettingsManager {
    /// # Errors
    ///
    /// Returns an error when the global settings file cannot be written.
    pub fn set_telemetry_enabled(&mut self, enabled: bool) -> Result<()> {
        let telemetry = self.global.telemetry.get_or_insert_with(Default::default);
        telemetry.enabled = Some(enabled);
        self.save_global(&["telemetry.enabled"])
    }

    /// # Errors
    ///
    /// Returns an error when the global settings file cannot be written.
    pub fn set_telemetry_notice_shown(&mut self, shown: bool) -> Result<()> {
        let telemetry = self.global.telemetry.get_or_insert_with(Default::default);
        telemetry.notice_shown = Some(shown);
        self.save_global(&["telemetry.noticeShown"])
    }

    /// `telemetry.enabled` when any scope sets it (`None`: nothing set, the
    /// default-on posture), resolved like [`Self::get_telemetry_enabled`].
    #[must_use]
    pub fn telemetry_enabled_setting(&self) -> Option<bool> {
        let set = [
            self.global.telemetry.as_ref(),
            self.project.telemetry.as_ref(),
            self.runtime_overrides.telemetry.as_ref(),
        ]
        .iter()
        .any(|scope| scope.and_then(|t| t.enabled).is_some());
        set.then(|| self.get_telemetry_enabled())
    }

    /// Telemetry is enabled only when every scope says so (default true).
    #[must_use]
    pub fn get_telemetry_enabled(&self) -> bool {
        [
            self.global.telemetry.as_ref(),
            self.project.telemetry.as_ref(),
            self.runtime_overrides.telemetry.as_ref(),
        ]
        .iter()
        .all(|scope| scope.and_then(|t| t.enabled).unwrap_or(true))
    }

    #[must_use]
    pub fn get_telemetry_notice_shown(&self) -> bool {
        self.runtime_overrides
            .telemetry
            .as_ref()
            .and_then(|t| t.notice_shown)
            .or_else(|| self.global.telemetry.as_ref().and_then(|t| t.notice_shown))
            .unwrap_or(false)
    }
}
