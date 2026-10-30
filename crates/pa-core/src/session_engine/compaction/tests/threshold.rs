//! The trigger: the threshold formula's two ceilings, its retention floor, and
//! the boundary the session asks about.
//!
//! Both questions live here because they must agree: `should_compact` prices
//! the HISTORY, `plan_compaction_due` prices the REQUEST, and a plan with no
//! prefix has to answer like the message-only form.

use super::super::*;
use super::fixtures::*;

#[test]
fn threshold_crosses_the_reserve_headroom() {
    // 126_010 tokens on a 128k window with a 127_500 reserve leaves
    // a 500-token combined ceiling — the crossing fires.
    let settings = CompactionSettings {
        threshold_percent: 95.0,
        enabled: true,
        reserve_tokens: 127_500,
        keep_recent_tokens: 10,
    };
    let messages = vec![
        user_message("f14 auto seed turn", 1),
        assistant_message(110, 2),
        user_message("f14 threshold crossing turn", 3),
        assistant_message(126_010, 4),
    ];
    assert!(threshold_compaction_due(&messages, 128_000, 0, &settings));
    let messages = vec![
        user_message("f14 auto seed turn", 1),
        assistant_message(110, 2),
    ];
    assert!(!threshold_compaction_due(&messages, 128_000, 0, &settings));
    let disabled = CompactionSettings {
        threshold_percent: 95.0,
        enabled: false,
        reserve_tokens: 127_500,
        keep_recent_tokens: 10,
    };
    assert!(!threshold_compaction_due(&messages, 128_000, 0, &disabled));
    assert!(!threshold_compaction_due(&messages, 0, 0, &settings));
}

#[test]
fn threshold_fires_before_a_combined_limit_overflow() {
    // The live 400 shape: 1_017_457 input tokens under the old trigger
    // (window - reserve = 1_032_192) but over the combined ceiling once the
    // 32_000 requested output no longer fits — the new trigger fires first.
    let settings = CompactionSettings::default();
    let messages = vec![
        user_message("live session turn", 1),
        assistant_message(1_017_457, 2),
    ];
    assert!(threshold_compaction_due(
        &messages, 1_048_576, 32_000, &settings
    ));
    let messages = vec![
        user_message("live session turn", 1),
        assistant_message(990_000, 2),
    ];
    assert!(!threshold_compaction_due(
        &messages, 1_048_576, 32_000, &settings
    ));
    // A 64k output budget pushes the combined ceiling below the percentage
    // one (1_048_576 - 65_536 - 16_384 = 966_656): 970_000 fires on the
    // combined ceiling alone — under the old reserve line AND 95% of the window.
    let messages = vec![
        user_message("live session turn", 1),
        assistant_message(970_000, 2),
    ];
    assert!(threshold_compaction_due(
        &messages, 1_048_576, 65_536, &settings
    ));
    assert!(!threshold_compaction_due(
        &messages, 1_048_576, 0, &settings
    ));
}

#[test]
fn threshold_percentage_ceiling_guards_large_windows() {
    // Past 95% of a large window the percentage ceiling fires before
    // the combined ceiling (1_048_576 * 0.95 = 996_147).
    let settings = CompactionSettings::default();
    assert!(should_compact(996_148, 1_048_576, 32_000, &settings));
    assert!(!should_compact(996_000, 1_048_576, 32_000, &settings));
}

#[test]
fn threshold_combined_ceiling_guards_small_windows() {
    // A 128k window with a 32k output budget: pure 95% (124_518) would under-reserve
    // — the combined ceiling (131_072 - 32_768 - 16_384 = 81_920) comes first.
    let settings = CompactionSettings::default();
    assert_eq!(compaction_threshold(131_072, 32_768, &settings), 81_920);
    assert!(should_compact(82_000, 131_072, 32_768, &settings));
    assert!(!should_compact(81_000, 131_072, 32_768, &settings));
}

#[test]
fn threshold_headroom_floor_covers_estimate_error() {
    // A tiny reserve still keeps the 4_096 estimate-error floor:
    // 131_072 - 32_768 - 4_096 = 94_208 < the 95% ceiling.
    let settings = CompactionSettings {
        threshold_percent: 95.0,
        enabled: true,
        reserve_tokens: 1,
        keep_recent_tokens: 10,
    };
    assert_eq!(compaction_threshold(131_072, 32_768, &settings), 94_208);
    // An output budget larger than any context still leaves a LIVE trigger
    // (see `a_declared_output_budget_at_the_window_never_disables_the_trigger`):
    // a reservation never exceeds half the window, because the other half is
    // the input the session exists to hold. Returning 0 here used to read as
    // "disabled" and stopped compaction for the model entirely.
    let wide = compaction_threshold(128_000, 124_000, &CompactionSettings::default());
    assert!(wide > 0, "a live trigger is the contract: {wide}");
    assert_eq!(wide, 128_000 / 2 - 16_384);
    assert!(should_compact(
        wide + 1,
        128_000,
        124_000,
        &CompactionSettings::default()
    ));
    assert!(!should_compact(
        wide,
        128_000,
        124_000,
        &CompactionSettings::default()
    ));
}

#[test]
fn stale_pre_compaction_usage_never_retriggers() {
    let settings = CompactionSettings {
        threshold_percent: 95.0,
        enabled: true,
        reserve_tokens: 127_500,
        keep_recent_tokens: 10,
    };
    let messages = vec![
        compaction_summary(10),
        user_message("kept turn", 5),
        assistant_message(126_010, 6),
    ];
    assert!(!threshold_compaction_due(&messages, 128_000, 0, &settings));
    let messages = vec![
        compaction_summary(10),
        user_message("new turn", 11),
        assistant_message(126_010, 12),
    ];
    assert!(threshold_compaction_due(&messages, 128_000, 0, &settings));
}

#[test]
fn should_compact_threshold() {
    let settings = CompactionSettings::default();
    assert!(should_compact(120_000, 128_000, 0, &settings)); // > 128k - 16k
    assert!(!should_compact(100_000, 128_000, 0, &settings));
    let disabled = CompactionSettings {
        enabled: false,
        ..settings
    };
    assert!(!should_compact(200_000, 128_000, 0, &disabled));
    assert!(!should_compact(1, 0, 0, &settings));
}
