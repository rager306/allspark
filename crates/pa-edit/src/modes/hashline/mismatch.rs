//! Snapshot-tag mismatch diagnostics.

use super::messages::format_anchored_context;
use crate::error::EditError;

/// Context needed to explain a stale or unrecognized snapshot tag.
pub struct MismatchDetails {
    pub path: Option<String>,
    pub expected_file_hash: String,
    pub actual_file_hash: String,
    pub file_lines: Vec<String>,
    pub anchor_lines: Vec<u32>,
    pub hash_recognized: bool,
    /// Absolute paths this session has already issued `expected_file_hash` for.
    /// Non-empty when `hash_recognized` is false and the tag belongs to a
    /// different file in the same session — e.g. a worktree lane that pasted a
    /// parent-checkout tag into its own (correctly) relative header.
    pub tag_origin_paths: Vec<String>,
}

/// Format the complete model-facing snapshot mismatch diagnostic.
pub fn format_mismatch_message(details: &MismatchDetails) -> String {
    let path = details
        .path
        .as_ref()
        .map_or_else(String::new, |path| format!(" for {path}"));
    let mut lines = if details.hash_recognized {
        vec![
            format!("Edit rejected{path}: file changed between read and edit."),
            format!(
                "Section is bound to #{}, but the current file hashes to #{}. If a prior edit in this \
				 session modified this file, copy the [path#newhash] header from that edit's \
				 response; otherwise read the file with `bash` and take the tag from the header \
				 this rejection names (the current file hashes to it).",
                details.expected_file_hash, details.actual_file_hash
            ),
        ]
    } else {
        let mut lines = vec![format!(
            "Edit rejected{path}: hash #{} is not from this session.",
            details.expected_file_hash
        )];
        // Name the absolute paths this session has already issued the tag for,
        // so a lane that pasted a tag from a sibling file (typically the parent
        // checkout) doesn't follow the suggestion into a wrong-tree write.
        // One path is the common case; show all so the model can pick the
        // right one when more than one file shares the tag.
        for origin in &details.tag_origin_paths {
            lines.push(format!(
                "Hash #{} was issued in this session for {origin}.",
                details.expected_file_hash
            ));
        }
        lines.push(format!(
            "The current file hashes to #{}. Use that tag in a [path#tag] header — never \
			 invent the tag and never reuse one from a prior session; a header with the current \
			 tag is accepted without any further read.",
            details.actual_file_hash
        ));
        lines
    };
    let context = format_anchored_context(&details.anchor_lines, &details.file_lines);
    if !context.is_empty() {
        lines.push(String::new());
        lines.extend(context);
    }
    lines.join("\n")
}

/// Construct a match failure from mismatch details.
pub fn mismatch_error(details: &MismatchDetails) -> EditError {
    EditError::matched(format_mismatch_message(details))
}
