use super::*;

#[test]
fn default_percentage_preserves_the_current_threshold_and_strict_boundary() {
    let settings = CompactionSettings::default();
    for (window, output, expected) in [
        (1_048_576, 32_000, 996_147),
        (131_072, 32_768, 81_920),
        (128_000, 0, 111_616),
        (0, 0, 0),
    ] {
        assert_eq!(compaction_threshold(window, output, &settings), expected);
        assert!(!should_compact(expected, window, output, &settings));
        assert_eq!(
            should_compact(expected + 1, window, output, &settings),
            expected > 0
        );
    }
}

#[test]
fn custom_percentages_obey_both_ceilings_as_model_limits_change() {
    for (percent, expected_by_model) in [
        (1.0, [52_769, 52_769, 17_152]),
        (50.0, [524_288, 65_536, 17_152]),
        (62.5, [655_360, 81_920, 17_152]),
        (80.0, [838_860, 81_920, 17_152]),
        (95.0, [996_147, 81_920, 17_152]),
        (100.0, [1_000_192, 81_920, 17_152]),
    ] {
        let settings = CompactionSettings {
            threshold_percent: percent,
            ..Default::default()
        };
        for ((window, output), expected) in
            [(1_048_576, 32_000), (131_072, 32_768), (65_536, 32_000)]
                .into_iter()
                .zip(expected_by_model)
        {
            assert_eq!(compaction_threshold(window, output, &settings), expected);
            assert!(!should_compact(expected, window, output, &settings));
            assert!(should_compact(expected + 1, window, output, &settings));
        }
    }
}

#[test]
fn invalid_runtime_percentages_fall_back_without_losing_headroom() {
    for percent in [
        0.0,
        -1.0,
        100.01,
        f64::NAN,
        f64::INFINITY,
        f64::NEG_INFINITY,
    ] {
        let settings = CompactionSettings {
            threshold_percent: percent,
            ..Default::default()
        };
        assert_eq!(compaction_threshold(1_048_576, 32_000, &settings), 996_147);
        assert_eq!(compaction_threshold(131_072, 32_768, &settings), 81_920);
    }
    let settings = CompactionSettings {
        threshold_percent: 100.0,
        reserve_tokens: 1,
        ..Default::default()
    };
    assert_eq!(compaction_threshold(131_072, 32_768, &settings), 94_208);
    // A window smaller than the headroom floor keeps a LIVE trigger: the
    // difference saturates to 0, and 0 means disabled — see
    // `a_declared_output_budget_at_the_window_never_disables_the_trigger`.
    assert_eq!(compaction_threshold(4_000, 32_000, &settings), 2_000);
}

#[tokio::test]
async fn saved_percentage_applies_to_existing_sessions_without_changing_manual_budgets() {
    let dir = tempfile::tempdir().unwrap();
    let agent_dir = dir.path().join("agent");
    let agent = std::sync::Arc::new(pa_agent::agent::Agent::new(
        pa_agent::agent::AgentOptions::default(),
    ));
    let mut session = super::super::AgentSession::new(
        agent,
        crate::session::manager::SessionManager::in_memory(dir.path()),
        Vec::new(),
    )
    .await
    .unwrap();
    session.set_compaction_settings(CompactionSettings {
        reserve_tokens: 12_345,
        keep_recent_tokens: 567,
        ..Default::default()
    });
    session.set_auxiliary_model_context(super::super::auxiliary_model::AuxiliaryModelContext {
        cwd: dir.path().to_path_buf(),
        agent_dir: agent_dir.clone(),
    });
    let mut settings = crate::settings::SettingsManager::create(dir.path(), &agent_dir);
    assert_eq!(session.compaction_settings().threshold_percent, 95.0);
    settings.set_compaction_threshold_percent(80.5).unwrap();
    assert_eq!(
        session.compaction_settings(),
        CompactionSettings {
            threshold_percent: 80.5,
            reserve_tokens: 12_345,
            keep_recent_tokens: 567,
            ..Default::default()
        }
    );
    session.set_auto_compaction_enabled(false);
    assert!(!should_compact(
        1_000_000,
        1_048_576,
        32_000,
        &session.compaction_settings()
    ));
}

#[test]
fn low_percentages_leave_room_for_retained_history_and_both_summaries() {
    for percent in [1.0, 5.0, 10.0] {
        let settings = CompactionSettings {
            threshold_percent: percent,
            ..Default::default()
        };
        assert_eq!(compaction_threshold(128_000, 16_384, &settings), 52_769);
        assert!(!should_compact(52_769, 128_000, 16_384, &settings));
        assert!(should_compact(52_770, 128_000, 16_384, &settings));
        // The hard combined model ceiling wins even if retention cannot fit.
        assert_eq!(compaction_threshold(65_536, 32_000, &settings), 17_152);
    }
}

/// Review 5 (#3793): a declared output budget that cannot fit alongside any
/// input must not disable the trigger.
///
/// Eight models in the compiled catalog declare `maxTokens == contextWindow`
/// (`gpt-4` 8192/8192, `llama-3.1-8b-instant` 131072/131072,
/// `moonshotai/kimi-k3` 1048576/1048576, `moonshotai/kimi-k2.6`,
/// `meta-llama/Llama-3.2-3B-Instruct`, `openai/gpt-oss-20b`,
/// `moonshotai/kimi-k2-0905`). Subtracting the whole budget gave a threshold of
/// 0, which this module reads as DISABLED — so compaction never fired and the
/// session grew until the provider rejected it.
#[test]
fn a_declared_output_budget_at_the_window_never_disables_the_trigger() {
    let settings = CompactionSettings::default();
    for (window, output) in [
        (8_192u64, 8_192u64),
        (131_072, 131_072),
        (98_304, 98_304),
        (1_048_576, 1_048_576),
        // A budget ABOVE the window is the same shape.
        (131_072, 200_000),
    ] {
        let threshold = compaction_threshold(window, output, &settings);
        assert!(
            threshold > 0,
            "window {window} with declared output {output} must keep a live trigger"
        );
        assert!(
            !should_compact(1, window, output, &settings),
            "an empty session is never due"
        );
        assert!(
            should_compact(threshold + 1, window, output, &settings),
            "crossing the threshold must fire for window {window}"
        );
    }
}

/// A budget BELOW the window is a real simultaneous claim and is reserved in
/// full: the clamp above must not loosen the ordinary cases.
#[test]
fn a_real_output_budget_is_still_reserved_in_full() {
    let settings = CompactionSettings::default();
    // 131_072 - 32_768 - 16_384 = 81_920 (the 95% ceiling is higher).
    assert_eq!(compaction_threshold(131_072, 32_768, &settings), 81_920);
    // 128_000 - 0 - 16_384 = 111_616 < 95% of 128_000.
    assert_eq!(compaction_threshold(128_000, 0, &settings), 111_616);
}
