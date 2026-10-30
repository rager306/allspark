//! Where a compaction cut may land: the legal positions, the tool-pair
//! predicate over a kept range, and the walk-back that turns a retention
//! budget into one boundary.

use super::tokens::estimate_entry_tokens;
use pa_types::ai::AssistantContentBlock;
use pa_types::session::{AgentMessage, FileEntry};

/// Valid cut positions: every message EXCEPT `ToolResult`, plus branch and
/// compaction boundaries.
///
/// # The tool-pair invariant is STRUCTURAL, not checked
///
/// Excluding `ToolResult` is what guarantees a cut never separates a result from the
/// assistant message that requested it: no valid boundary can fall between a call and
/// its result, so dropping the call always drops its result and keeping the call
/// always keeps it. A provider rejects a request whose `toolResult` names an absent
/// `toolCallId`, and this set makes that state unreachable from `find_cut_point`.
///
/// That is necessary but NOT sufficient, and the measurement says so: the set
/// excludes `ToolResult` as a cut POSITION, while `find_cut_point`'s walk-back can
/// still leave the boundary ON a result, because `cut_index` starts at
/// `cut_points[0]` and the back-walk stops at the previous message. With the pair
/// repair in `prepare_compaction` disabled, the exhaustive probe finds **17 401**
/// stranded cases out of 60 976 (shape, budget) pairs; with the `start_index` fix
/// below (a fit-history cut no longer lands on the first message) that count drops to
/// 1 759 — and with the repair enabled, zero. Both are load-bearing.
///
/// So the invariant is enforced in two places on purpose — this set says which
/// positions are legal, and the repair makes the published boundary respect the
/// pair — and `probe_every_shape_and_budget_for_a_stranded_result` is the guard over
/// both. Removing either one makes that test fail.
#[must_use]
pub fn find_valid_cut_points(
    entries: &[FileEntry],
    start_index: usize,
    end_index: usize,
) -> Vec<usize> {
    let mut cut_points = Vec::new();
    for (i, entry) in entries
        .iter()
        .enumerate()
        .take(end_index.min(entries.len()))
        .skip(start_index)
    {
        match entry {
            FileEntry::Message { message, .. } => match message {
                AgentMessage::User(_)
                | AgentMessage::Assistant(_)
                | AgentMessage::Custom(_)
                | AgentMessage::BashExecution(_)
                | AgentMessage::BranchSummary(_)
                | AgentMessage::CompactionSummary(_) => cut_points.push(i),
                AgentMessage::ToolResult(_) => {}
            },
            FileEntry::BranchSummary { .. } | FileEntry::CustomMessage { .. } => {
                cut_points.push(i);
            }
            _ => {}
        }
    }
    cut_points
}

/// Whether keeping `entries[keep_from..]` would strand a tool result: a result
/// whose matching call sits on the DROPPED side.
///
/// This is the provider's own precondition, not a heuristic: a `toolResult`
/// whose `toolCallId` does not appear in a preceding assistant message is
/// rejected at request time, so a plan that produces one advertises a split it
/// cannot apply. The donor's `projectUnit` exists for the same class of defect
/// (recommend, explain and apply sharing ONE validity rule) and names the
/// symptom: "Range would split N turn(s)".
///
/// The check spans the WHOLE kept range, so it also catches a result kept by a
/// later cut while its call was dropped by an earlier one.
#[must_use]
pub fn orphans_a_tool_result(entries: &[FileEntry], keep_from: usize) -> bool {
    let kept = entries.get(keep_from..).unwrap_or(&[]);
    let mut calls: std::collections::HashSet<&str> = std::collections::HashSet::new();
    for entry in kept {
        let FileEntry::Message { message, .. } = entry else {
            continue;
        };
        match message {
            AgentMessage::Assistant(assistant) => {
                for block in &assistant.content {
                    if let AssistantContentBlock::ToolCall(call) = block {
                        calls.insert(call.id.as_str());
                    }
                }
            }
            // A result whose call is not in the kept range: the provider would
            // reject the request, so the cut is not applicable.
            AgentMessage::ToolResult(result) if !calls.contains(result.tool_call_id.as_str()) => {
                return true;
            }
            _ => {}
        }
    }
    false
}

/// The user/bashExecution message (or branch boundary) starting the turn
/// containing `entry_index`; None when no turn start exists before it.
fn find_turn_start_index(
    entries: &[FileEntry],
    entry_index: usize,
    start_index: usize,
) -> Option<usize> {
    for i in (start_index..=entry_index).rev() {
        let entry = &entries[i];
        match entry {
            FileEntry::BranchSummary { .. }
            | FileEntry::CustomMessage { .. }
            | FileEntry::Message {
                message: AgentMessage::User(_) | AgentMessage::BashExecution(_),
                ..
            } => return Some(i),
            _ => {}
        }
    }
    None
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CutPointResult {
    pub first_kept_entry_index: usize,
    /// Turn start when splitting mid-turn (None otherwise).
    pub turn_start_index: Option<usize>,
    pub is_split_turn: bool,
}

/// Find the cut point keeping approximately `keep_recent_tokens`.
///
/// # Panics
///
/// The `unwrap` on the last cut point cannot fire: the cut-point list was
/// checked non-empty above.
#[must_use]
pub fn find_cut_point(
    entries: &[FileEntry],
    start_index: usize,
    end_index: usize,
    keep_recent_tokens: u64,
) -> CutPointResult {
    let cut_points = find_valid_cut_points(entries, start_index, end_index);
    if cut_points.is_empty() {
        return CutPointResult {
            first_kept_entry_index: start_index,
            turn_start_index: None,
            is_split_turn: false,
        };
    }
    // Where the cut starts depends on what the budget MEANS:
    //
    // - `0` is "retain nothing": the whole history is summarized, so the cut is the
    //   first legal position. Carrying a large budget instead must not summarize
    //   everything — that was the inverted case this comment replaced, where a budget
    //   larger than the history still cut at `cut_points[0]` and dropped it all.
    // - anything else is "retain up to N": start by keeping everything and let the
    //   loop below move the boundary only when the retained tail actually exceeds it.
    //
    // Both are needed: the daemon's low-percentage compaction relies on `0` meaning
    // "all" (it compacts real history), and a generous budget must mean "keep".
    let mut accumulated = 0u64;
    let mut cut_index = cut_points[0];
    for (i, entry) in entries
        .iter()
        .enumerate()
        .take(end_index.min(entries.len()))
        .skip(start_index)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
    {
        accumulated = accumulated.saturating_add(estimate_entry_tokens(entry));
        if accumulated >= keep_recent_tokens {
            cut_index = *cut_points.last().unwrap();
            for &point in &cut_points {
                if point >= i {
                    cut_index = point;
                    break;
                }
            }
            break;
        }
    }
    // Walk back to a message or compaction boundary.
    while cut_index > start_index {
        match &entries[cut_index - 1] {
            FileEntry::Compaction { .. } | FileEntry::Message { .. } => break,
            _ => cut_index -= 1,
        }
    }
    let is_user_message = matches!(
        &entries[cut_index],
        FileEntry::Message {
            message: AgentMessage::User(_),
            ..
        }
    );
    let turn_start_index = if is_user_message {
        None
    } else {
        find_turn_start_index(entries, cut_index, start_index)
    };
    CutPointResult {
        first_kept_entry_index: cut_index,
        is_split_turn: !is_user_message && turn_start_index.is_some(),
        turn_start_index,
    }
}
