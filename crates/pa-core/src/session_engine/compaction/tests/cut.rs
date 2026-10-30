//! The cut: legal positions, the tool-pair predicate, and the retention walk.
//!
//! The tool-pair invariant is the contract here — a cut may never leave a
//! result without the call it answers, at any shape and any budget.

use super::super::*;
use super::fixtures::*;

#[test]
fn cut_points_never_tool_results() {
    let entries = vec![
        user_entry("u1", "root", "hello there"),
        assistant_entry("a1", "u1"),
        message_entry(
            "t1",
            "a1",
            AgentMessage::ToolResult(pa_types::ai::ToolResultMessage {
                tool_call_id: "c".to_string(),
                tool_name: "bash".to_string(),
                content: vec![pa_types::ai::UserContentBlock::Text(
                    pa_types::ai::TextContent {
                        text: "result".to_string(),
                        text_signature: None,
                        rest: serde_json::Map::default(),
                    },
                )],
                details: None,
                is_error: false,
                timestamp: 0,
                rest: serde_json::Map::default(),
            }),
        ),
    ];
    let points = find_valid_cut_points(&entries, 0, 3);
    assert_eq!(points, vec![0, 1]);
}

#[test]
fn cut_point_keeps_recent_tokens() {
    let entries = vec![
        user_entry("u1", "root", "turn one"),
        assistant_entry("a1", "u1"),
        user_entry("u2", "a1", "turn two"),
        assistant_entry("a2", "u2"),
        user_entry("u3", "a2", "turn three"),
    ];
    let keep_all = find_cut_point(&entries, 0, 5, 10_000);
    assert_eq!(keep_all.first_kept_entry_index, 0);
    assert!(!keep_all.is_split_turn);
    let keep_little = find_cut_point(&entries, 0, 5, 1);
    assert!(keep_little.first_kept_entry_index > 0);
    assert!(!keep_little.is_split_turn);

    // An assistant-final session cuts mid-turn.
    let assistant_final = vec![
        user_entry("u1", "root", "turn one"),
        assistant_entry("a1", "u1"),
        user_entry("u2", "a1", "turn two"),
        assistant_entry("a2", "u2"),
    ];
    let keep_small = find_cut_point(&assistant_final, 0, 4, 1);
    assert!(keep_small.is_split_turn);
    assert_eq!(keep_small.turn_start_index, Some(2));
}

/// S2 (the donor's `projectUnit` lesson): a cut must never separate a tool
/// RESULT from the ASSISTANT message that requested it.
///
/// `find_valid_cut_points` refuses `ToolResult` entries as cut positions,
/// which covers the adjacent shape. The shape it cannot see is a result
/// whose matching call is on the CUT side: the provider then rejects the
/// request (`tool_call_id` not found), so the plan advertised a split it
/// could not apply — the donor's "Range would split N turn(s)" symptom.
#[test]
fn a_cut_never_separates_a_tool_result_from_its_call() {
    let entries = vec![
        user_entry("m1", "", "start"),
        message_entry("m2", "m1", tool_call_assistant("call-1", "bash")),
        message_entry("m3", "m2", tool_result_message("call-1", "done")),
        user_entry("m4", "m3", "next"),
        user_entry("m5", "m4", "last"),
    ];
    let cut = find_cut_point(&entries, 0, entries.len(), 0);
    assert!(
        !orphans_a_tool_result(&entries, cut.first_kept_entry_index),
        "cut {} separated a result from its call",
        cut.first_kept_entry_index
    );

    // The shape that CAN happen: the result survives while its call is cut.
    let split = vec![
        user_entry("m1", "", "start"),
        message_entry("m2", "m1", tool_call_assistant("call-1", "bash")),
        user_entry("m3", "m2", "interrupt"),
        message_entry("m4", "m3", tool_result_message("call-1", "done")),
    ];
    assert!(
        orphans_a_tool_result(&split, 2),
        "the predicate must FIRE on the split shape, or it proves nothing"
    );
    assert!(
        !orphans_a_tool_result(&split, 1),
        "keeping the call with its result is legal"
    );
    assert!(
        orphans_a_tool_result(&split, 3),
        "a result with no call before it is orphaned wherever the cut lands"
    );
}

/// S2 exhaustive probe: is there ANY transcript shape and budget where the cut
/// orphans a result? If none, the walk-back loop in `prepare_compaction` is dead
/// code and the invariant belongs in a test, not in a runtime loop.
///
/// Enumerates every arrangement of the three shapes that can interleave a call
/// with its result: assistant-with-call, tool-result, and user/assistant text.
#[test]
fn probe_every_shape_and_budget_for_a_stranded_result() {
    // Shapes: C = assistant with a tool call, R = its result, T = plain text turn.
    let shapes: &[&str] = &["C", "R", "T"];
    let mut checked = 0usize;
    let mut stranded: Vec<String> = Vec::new();
    for len in 1..=6usize {
        let combos = 3usize.pow(len as u32);
        for combo in 0..combos {
            let mut pattern = String::new();
            let mut n = combo;
            for _ in 0..len {
                pattern.push_str(shapes[n % 3]);
                n /= 3;
            }
            // A result with no preceding call is malformed input, not a cut bug.
            let entries = entries_for(&pattern);
            for keep in 0..200u64 {
                let Ok(prep) =
                    crate::session_engine::compact_session::prepare_compaction(&entries, keep)
                else {
                    continue;
                };
                checked += 1;
                if orphans_a_tool_result(&entries, prep.cut.first_kept_entry_index) {
                    stranded.push(format!("{pattern} @ keep={keep}"));
                }
            }
        }
    }
    assert!(
        stranded.is_empty(),
        "{} shape(s) strand a result: {:?}",
        stranded.len(),
        &stranded[..stranded.len().min(5)]
    );
    assert!(checked > 1_000, "the probe must actually run: {checked}");
    eprintln!("checked {checked} (pattern, budget) pairs, none stranded");
}

/// The defect the exhaustive probe found: when the whole history FITS the
/// retention budget, the cut still lands on the first valid message, so
/// everything before it is summarized away and — when that message is a
/// tool result's position — a result is stranded from its call.
///
/// A cut is only needed when something must be dropped. A budget larger than the
/// history means "keep it all", and the honest cut then is `start_index`.
#[test]
fn a_history_that_fits_the_budget_is_kept_whole() {
    let entries = vec![
        user_entry("m1", "", "start"),
        message_entry("m2", "m1", tool_call_assistant("call-1", "bash")),
        message_entry("m3", "m2", tool_result_message("call-1", "done")),
        user_entry("m4", "m3", "next"),
    ];
    let total: u64 = entries.iter().map(estimate_entry_tokens).sum();
    // A budget past the whole history: nothing needs to be dropped.
    let cut = find_cut_point(&entries, 0, entries.len(), total + 1_000);
    assert_eq!(
        cut.first_kept_entry_index, 0,
        "a history that fits must be kept whole, not cut at its first message"
    );
    assert!(
        !orphans_a_tool_result(&entries, cut.first_kept_entry_index),
        "and nothing is stranded"
    );
}

/// Diagnostic: with a real budget, does the cut still advance?
#[test]
fn probe_cut_with_a_real_budget() {
    let mut entries = vec![user_entry("m0", "", "start")];
    for i in 0..30 {
        entries.push(message_entry(
            &format!("a{i}"),
            "m0",
            assistant_message(100, 0),
        ));
        entries.push(user_entry(&format!("u{i}"), "m0", &"x".repeat(12_000)));
    }
    let cut = find_cut_point(&entries, 0, entries.len(), 10_000);
    eprintln!(
        "entries={} cut={} split={}",
        entries.len(),
        cut.first_kept_entry_index,
        cut.is_split_turn
    );
    assert!(
        cut.first_kept_entry_index > 0,
        "a 10k budget on a long history must cut"
    );
}
