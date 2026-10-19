//! The sparse walk's end-section laziness: a frame whose window cannot
//! reach the splash or the tail renders them zero times, and a frame that
//! reaches them renders them once.
use super::*;
use crate::chat::{ChatEntry, StatusKind, WorkingState};
use crate::theme::{ColorMode, Theme};

fn view() -> AgentView {
    AgentView::new(Theme::builtin("prime", ColorMode::TrueColor))
}

/// A view with a long transcript and tail content an eager compose would
/// render on every frame: the working loader.
fn filled(turns: usize) -> AgentView {
    let mut view = view();
    for index in 0..turns {
        view.push_entry(ChatEntry::Status {
            text: format!("row {index}"),
            kind: StatusKind::Info,
        });
    }
    view.working = Some(WorkingState {
        activity: "Thinking",
        message: None,
        download: true,
        tokens: 3,
        elapsed_secs: 0,
    });
    view
}

fn text_of(frame: &[Line]) -> Vec<String> {
    frame
        .iter()
        .map(|line| line.iter().map(|span| span.content.as_str()).collect())
        .collect()
}

fn reset_section_counters() {
    SPLASH_RENDERS.with(|count| count.set(0));
    TAIL_RENDERS.with(|count| count.set(0));
}

/// A scrolled frame in the middle of a long transcript renders neither
/// end section, and its rows are the exact-geometry render at the same
/// transcript position.
#[test]
fn mid_transcript_scroll_frames_render_neither_end_section() {
    let mut sparse = filled(400);
    let mut full = filled(400);
    sparse.render_frame(37, 24);
    // The exact-geometry reference holds `sparse_enabled` off, so its
    // frames render through the full layout pass; it learns the
    // transcript's absolute geometry once.
    full.sparse_enabled = false;
    full.render_frame(37, 24);
    sparse.scroll_by(-60);
    // The first scrolled frame establishes the walked cursor from the
    // cursorless tail anchor; the counters below cover the frames after.
    sparse.render_frame(37, 24);
    full.resolve_sparse_geometry();
    full.scroll_by(-63);
    full.resolve_sparse_geometry();
    full.sparse_enabled = false;
    sparse.scroll_by(-3);
    reset_section_counters();
    let frame = sparse.render_frame(37, 24);
    assert_eq!(SPLASH_RENDERS.with(std::cell::Cell::get), 0);
    assert_eq!(TAIL_RENDERS.with(std::cell::Cell::get), 0);
    assert_eq!(frame, full.render_frame(37, 24));
}

/// A frame at the transcript start renders the splash (not the tail), a
/// following frame renders the tail (not the splash), and the reached
/// section's content is in the frame the walk returned.
#[test]
fn frames_render_an_end_section_only_when_the_window_reaches_it() {
    let mut view = filled(400);
    view.render_frame(37, 24);
    view.scroll_to_top();
    reset_section_counters();
    let top = view.render_frame(37, 24);
    assert_eq!(SPLASH_RENDERS.with(std::cell::Cell::get), 1);
    assert_eq!(TAIL_RENDERS.with(std::cell::Cell::get), 0);
    let rows = text_of(&top);
    assert!(rows.iter().any(|row| row.contains("prime agent")));
    assert!(!rows.iter().any(|row| row.contains("Thinking")));
    view.scroll_to_bottom();
    reset_section_counters();
    let tail = view.render_frame(37, 24);
    assert_eq!(SPLASH_RENDERS.with(std::cell::Cell::get), 0);
    assert_eq!(TAIL_RENDERS.with(std::cell::Cell::get), 1);
    let rows = text_of(&tail);
    assert!(rows.iter().any(|row| row.contains("Thinking")));
}

#[test]
fn streaming_entry_renders_once_per_frame_without_reusing_stale_rows() {
    use crate::chat::{AssistantMessage, MessageBlock};
    use crate::view::layout::ENTRY_RENDERS;

    let mut sparse = view();
    let mut dense = view();
    for view in [&mut sparse, &mut dense] {
        view.push_entry(ChatEntry::Assistant(Box::new(AssistantMessage {
            blocks: vec![MessageBlock::Text("history ".repeat(300))],
            has_tool_calls: false,
            streaming: true,
            error: None,
            aborted: false,
        })));
        view.render_frame(40, 20);
    }
    for suffix in ["", "\n\n**a new bold tail**", "\n\nUnicode café 界 tail"] {
        for view in [&mut sparse, &mut dense] {
            view.prepare_entry_mutation(0);
            let ChatEntry::Assistant(message) = &mut view.chat[0] else {
                panic!("streaming assistant fixture");
            };
            let MessageBlock::Text(text) = &mut message.blocks[0] else {
                panic!("streaming text fixture");
            };
            text.push_str(suffix);
            view.mark_entry_stale(0);
        }
        ENTRY_RENDERS.with(|count| count.set(0));
        let frame = sparse.render_frame(40, 20);
        assert_eq!(ENTRY_RENDERS.with(std::cell::Cell::get), 1);
        dense.sparse_enabled = false;
        dense.sparse_window = None;
        assert_eq!(frame, dense.render_frame(40, 20));
    }
}
