//! The summarization prompts: the initial and update templates the summarizer
//! answers in, the split-turn prefix template, and the retry instruction for a
//! summary the output cap cut off.

use std::fmt::Write as _;

const SUMMARIZATION_PROMPT: &str = "The messages above are a conversation to summarize. Create a structured context checkpoint summary that another LLM will use to continue the work.\n\nUse this EXACT format:\n\n## Goal\n[What is the user trying to accomplish? Can be multiple items if the session covers different tasks.]\n\n## Constraints & Preferences\n- [Any constraints, preferences, or requirements mentioned by user]\n- [Or \"(none)\" if none were mentioned]\n\n## Progress\n### Done\n- [x] [Completed tasks/changes]\n\n### In Progress\n- [ ] [Current work]\n\n### Blocked\n- [Issues preventing progress, if any]\n\n## Outstanding Work\n- [Running subagents: name / task / status (recovery pointer: rlm.list_subagents())]\n- [Background bash handles: command / status (recovery: the [bash-done] notification; a lost handle cannot be re-awaited)]\n- [Active heartbeats and the goal: schedule / objective (recovery pointers: rlm_heartbeat.list() and goal.get())]\n- [Or \"(none)\" if nothing is in flight]\n\n## Key Decisions\n- **[Decision]**: [Brief rationale]\n\n## Next Steps\n1. [Ordered list of what should happen next]\n\n## Critical Context\n- [Any data, examples, or references needed to continue]\n- [Or \"(none)\" if not applicable]\n\nKeep each section concise. Preserve exact file paths, function names, and error messages.";

const KERNEL_PERSIST_SUMMARY_NOTE: &str = "Note: the Python kernel keeps running after this summary — every Python variable, import, and helper you defined stays available. The cells that defined them won't appear above, so record in the summary any names worth remembering so you reuse them instead of redefining them.";

/// A split turn keeps its suffix; this prompt summarizes the cut turn's
/// prefix for the retained recent work.
pub const TURN_PREFIX_SUMMARIZATION_PROMPT: &str = "This is the PREFIX of a turn that was too large to keep. The SUFFIX (recent work) is retained.\n\nSummarize the prefix to provide context for the retained suffix:\n\n## Original Request\n[What did the user ask for in this turn?]\n\n## Early Progress\n- [Key decisions and work done in the prefix]\n\n## Outstanding Work\n- [Running subagents, background bash handles, active heartbeats/goal started in the prefix and still in flight; or \"(none)\"]\n\n## Context for Suffix\n- [Information needed to understand the retained recent work]\n\nBe concise. Focus on what's needed to understand the kept suffix.";

const UPDATE_SUMMARIZATION_PROMPT: &str = "The messages above are NEW conversation messages to incorporate into the existing summary provided in <previous-summary> tags.\n\nUpdate the existing structured summary with new information. RULES:\n- PRESERVE all existing information from the previous summary\n- ADD new progress, decisions, and context from the new messages\n- UPDATE the Progress section: move items from \"In Progress\" to \"Done\" when completed\n- UPDATE \"Next Steps\" based on what was accomplished\n- PRESERVE exact file paths, function names, and error messages\n- PRESERVE the Outstanding Work section: update in-flight subagents, background bash handles, heartbeats, and the goal from the new messages\n- If something is no longer relevant, you may remove it\n\nUse this EXACT format:\n\n## Goal\n[Preserve existing goals, add new ones if the task expanded]\n\n## Constraints & Preferences\n- [Preserve existing, add new ones discovered]\n\n## Progress\n### Done\n- [x] [Include previously done items AND newly completed items]\n\n### In Progress\n- [ ] [Current work - update based on progress]\n\n### Blocked\n- [Current blockers - remove if resolved]\n\n## Outstanding Work\n- [Preserve in-flight subagents, background bash handles, heartbeats, and the goal; update statuses from the new messages]\n\n## Key Decisions\n- **[Decision]**: [Brief rationale] (preserve all previous, add new)\n\n## Next Steps\n1. [Update based on current state]\n\n## Critical Context\n- [Preserve important context, add new if needed]\n\nKeep each section concise. Preserve exact file paths, function names, and error messages.";

/// The retry instruction for a summary the output cap cut off: the retry
/// wire call appends it as one more text block on the summarization request.
pub(crate) const SHORTER_SUMMARY_INSTRUCTION: &str = "Your previous summary was cut off at the output limit. Write a shorter summary that fits: keep every required section and compress the detail.";

/// The line a twice-capped summary commits behind: the retry's text still
/// replaces the transcript, so it rides in front of this disclosure.
pub(crate) const SUMMARY_TRUNCATED_MARKER: &str =
    "[summary truncated: earlier detail may be missing]";

/// The instruction part of the summarization prompt (initial or update
/// template, user instructions, kernel persistence note).
#[must_use]
pub fn build_summarization_prompt(
    custom_instructions: Option<&str>,
    previous_summary: Option<&str>,
) -> String {
    let mut base = if previous_summary.is_some() {
        UPDATE_SUMMARIZATION_PROMPT
    } else {
        SUMMARIZATION_PROMPT
    }
    .to_string();
    if let Some(custom_instructions) = custom_instructions {
        let _ = write!(base,
            "\n\n<user-instructions>\nThe user provided these instructions for this summary. Follow them with high priority while keeping the section format above: emphasize what they ask to focus on, and preserve verbatim anything they ask to remember.\n{custom_instructions}\n</user-instructions>"
        );
    }
    format!("{base}\n\n{KERNEL_PERSIST_SUMMARY_NOTE}")
}
