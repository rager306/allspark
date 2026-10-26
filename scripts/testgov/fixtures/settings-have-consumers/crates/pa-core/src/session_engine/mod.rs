pub fn use_it(s: &crate::settings::types::Settings) { let _ = s.compaction_threshold; }

/// The false-positive shape this fixture exists for: an unrelated identifier
/// that CONTAINS `<field>`. The old rule credited `idle_timeout_ms` here,
/// because a substring match counted as a read. Keep this file's name so the
/// fixture keeps failing the field the check names.
pub fn resolve_stdin_a_setting_nobody_reads_ms() -> u64 { 250 }
