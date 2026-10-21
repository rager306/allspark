//! The streaming main-screen flush (TS `exitFullscreen`'s inline
//! repaint): the exit path that renders the inline frame section by
//! section and writes the changed rows into the user's native
//! scrollback in bounded chunks.

use super::AgentView;
use crate::Line;

impl AgentView {
    /// Stream the changed rows of the inline layout to `out` as the main-screen flush: the one
    /// output path that writes into the user's native scrollback, so its byte stream is
    /// parity-frozen. `self.flushed_frame` is the diff base.
    ///
    /// - rows extending the flushed frame append into scrollback;
    /// - a change above the flushed tail repaints the last screenful
    ///   (scrollback is immutable);
    /// - an identical frame writes nothing.
    ///
    /// # Errors
    ///
    /// Propagates the write error when `out` rejects a chunk: the rows already written have
    /// scrolled, so the flush is not retried.
    pub fn stream_flush_to(
        &mut self,
        out: &mut dyn std::io::Write,
        width: usize,
        screen_height: usize,
    ) -> std::io::Result<()> {
        let layout = self.layout_pass(width);
        let mut sink = FlushSink {
            flushed: std::mem::take(&mut self.flushed_frame),
            texts: Vec::new(),
            ring: std::collections::VecDeque::new(),
            chunk: String::new(),
            screen_height,
            appending: false,
            repaint: false,
        };
        sink.feed(out, &layout.splash)?;
        let mut preceded_by_tool_activity = false;
        for (index, entry) in self.chat.iter().enumerate() {
            let rows =
                self.render_entry(index, entry, width, index == 0, preceded_by_tool_activity);
            sink.feed(out, &rows)?;
            preceded_by_tool_activity = Self::is_compact_neighbor(entry);
        }
        sink.feed(out, &layout.tail)?;
        let dock = self.render_dock(width);
        sink.feed(out, &dock)?;
        sink.finish(out)?;
        self.flushed_frame = std::mem::take(&mut sink.texts);
        Ok(())
    }
}

/// The encoded flush rows leave the process in chunks of at most this many bytes: each PTY
/// write stays one syscall.
const CHUNK_BYTES: usize = 32 * 1024;

/// The unit of exit-path progress one flush write reports.
///
/// A write to the pty blocks until the terminal drains its buffer, so a completed write is the
/// only proof the flush is moving — and the granularity of that proof is the exit guard's
/// heartbeat. Stamping once per [`CHUNK_BYTES`] tied the heartbeat to a whole chunk's drain:
/// at the gate's 96 KiB/s slow-drain harness a 32 KiB chunk lands every ~350ms, so scheduler
/// jitter alone pushed the gap past the guard's 500ms grace and force-quit a healthy flush
/// mid-transcript (the `slow_drain_exit_guard_e2e` failure: one alt-screen leave, no `row 0`).
/// A quarter of the grace at that rate is 4 KiB; a stalled terminal accepts nothing at all,
/// which is the state the guard exists to catch.
pub(super) const PROGRESS_WRITE_BYTES: usize = 4 * 1024;

/// The streaming main-screen flush state: feeds rows section by section, routes them between
/// the append stream and the repaint ring, writes in bounded chunks.
struct FlushSink {
    /// The last flush's row texts — the diff base.
    flushed: Vec<String>,
    /// The new frame's row texts (the NEXT flush's diff base).
    texts: Vec<String>,
    /// The most recent `screen_height` rows, for the repaint write.
    ring: std::collections::VecDeque<crate::Line>,
    /// The encoded append rows not yet handed to `out`.
    chunk: String,
    screen_height: usize,
    /// Set once a row extends the flushed frame: every later row appends.
    appending: bool,
    /// Set when a row inside the flushed frame changed: every row lands
    /// in the repaint ring instead.
    repaint: bool,
}

impl FlushSink {
    /// Feed one section of the inline frame.
    fn feed(&mut self, out: &mut dyn std::io::Write, rows: &[crate::Line]) -> std::io::Result<()> {
        for row in rows {
            let index = self.texts.len();
            let text = row_text_of(row);
            if self.appending {
                crate::interactive::write_flush_rows(&mut self.chunk, std::slice::from_ref(row));
                self.texts.push(text);
                if self.chunk.len() >= CHUNK_BYTES {
                    write_with_progress(
                        out,
                        self.chunk.as_bytes(),
                        crate::exit_guard::note_exit_progress,
                    )?;
                    self.chunk.clear();
                }
            } else if self.repaint || index >= self.flushed.len() {
                // A changed row turns the write into a repaint; a row
                // past the flushed frame turns it into an append.
                if self.repaint {
                    self.ring_push(row);
                } else {
                    self.appending = true;
                    crate::interactive::write_flush_rows(
                        &mut self.chunk,
                        std::slice::from_ref(row),
                    );
                }
                self.texts.push(text);
            } else {
                if self.flushed[index].as_str() != text.as_str() {
                    self.repaint = true;
                }
                self.ring_push(row);
                self.texts.push(text);
            }
        }
        Ok(())
    }

    /// Keep the repaint ring at one screenful.
    fn ring_push(&mut self, row: &crate::Line) {
        self.ring.push_back(row.clone());
        while self.ring.len() > self.screen_height {
            self.ring.pop_front();
        }
    }

    /// Write what the decided mode owes: the append tail, the repaint
    /// erase plus the ring, or nothing for an identical frame.
    fn finish(&mut self, out: &mut dyn std::io::Write) -> std::io::Result<()> {
        if self.appending {
            if !self.chunk.is_empty() {
                write_with_progress(
                    out,
                    self.chunk.as_bytes(),
                    crate::exit_guard::note_exit_progress,
                )?;
                self.chunk.clear();
            }
        } else if self.repaint || self.texts.len() < self.flushed.len() {
            // A frame that shrank never rewinds into a rewrite of
            // scrollback: the changed region repaints the visible window.
            let mut buffer = String::from("\x1b[2J\x1b[H");
            let ring: Vec<crate::Line> = std::mem::take(&mut self.ring).into_iter().collect();
            crate::interactive::write_flush_rows(&mut buffer, &ring);
            write_with_progress(
                out,
                buffer.as_bytes(),
                crate::exit_guard::note_exit_progress,
            )?;
            self.chunk.clear();
        }
        Ok(())
    }
}

/// Write one encoded flush buffer, stamping exit-path progress after every
/// [`PROGRESS_WRITE_BYTES`] the terminal accepts: the exit guard's watchdog reads
/// a completed write as proof the drain is moving, so its heartbeat must be
/// finer than the guard's grace window at any drain rate it is meant to survive.
pub(super) fn write_with_progress(
    out: &mut dyn std::io::Write,
    bytes: &[u8],
    mut progress: impl FnMut(),
) -> std::io::Result<()> {
    for slice in bytes.chunks(PROGRESS_WRITE_BYTES) {
        out.write_all(slice)?;
        progress();
    }
    Ok(())
}

/// Concatenated span contents of a row (includes zero-width OSC zone
/// markers, which must persist into scrollback).
fn row_text_of(line: &Line) -> String {
    line.iter().map(|span| span.content.as_str()).collect()
}

/// Split a string at a char boundary.
pub(super) fn split_at_chars(text: &str, at: usize) -> (&str, &str) {
    let mut end = text.len();
    let mut count = 0;
    for (index, _) in text.char_indices() {
        if count == at {
            end = index;
            break;
        }
        count += 1;
    }
    if count < at {
        return (text, "");
    }
    (&text[..end], &text[end..])
}
