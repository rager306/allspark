//! The shared PTY harness: one reader (the raw stream + the rendered grid),
//! the quiescence oracle, and the failure-artifact capture. The per-test
//! `PtyReader` copies converge here — migrate a test by deleting its local
//! copy and declaring `mod support;`. The copies drifted (11 variants, no
//! two identical), which is exactly how a fix in one place misses the other
//! ten.
//!
//! The quiescence oracle (`wait_for_stable`): a surface is settled when the
//! child has produced no new output for `stable_ms`. Every wait's failure
//! path dumps the artifacts (the raw stream and the rendered grid) under
//! `.tmp/e2e-artifacts/`, so a red run reconstructs without a re-run.
#![cfg(unix)]
// Each test crate consumes only the parts it uses; the union stays whole.
#![allow(dead_code)]

use std::io::{Read, Write};
use std::os::fd::{AsRawFd, OwnedFd};
use std::time::{Duration, Instant};

use nix::fcntl::{fcntl, FcntlArg::F_SETFL, OFlag};
use nix::pty::{openpty, Winsize};

/// The harness pty geometry (the per-test copies' sizes varied; the union
/// pins one).
pub const PTY_ROWS: usize = 24;
pub const PTY_COLS: usize = 80;

/// Where the failure artifacts land (the repo scratch, never the OS temp).
pub fn artifacts_dir() -> std::path::PathBuf {
    let manifest = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest
        .ancestors()
        .nth(2)
        .map_or_else(std::env::temp_dir, |root| {
            root.join(".tmp").join("e2e-artifacts")
        })
}

/// A fresh pty pair with the harness geometry.
pub fn open_harness_pty() -> (OwnedFd, OwnedFd) {
    #[allow(clippy::cast_possible_truncation)] // the harness geometry is a compile-time constant
    let winsize = Winsize {
        ws_row: PTY_ROWS as u16,
        ws_col: PTY_COLS as u16,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    openpty(Some(&winsize), None)
        .expect("openpty")
        .into_fdmaster_fdslave()
}

trait IntoFdPair {
    fn into_fdmaster_fdslave(self) -> (OwnedFd, OwnedFd);
}

impl IntoFdPair for nix::pty::OpenptyResult {
    fn into_fdmaster_fdslave(self) -> (OwnedFd, OwnedFd) {
        (self.master, self.slave)
    }
}

/// The rendered screen (a minimal ANSI grid renderer): the TUI draws
/// char-by-char with cursor moves, so a needle must be matched against the
/// RENDERED grid — a raw byte search false-negatives whenever a word is
/// painted glyph-by-glyph with cursor hops between them.
pub struct PtyReader {
    pub file: std::fs::File,
    /// The raw byte stream (kept for the artifact dump).
    pub output: Vec<u8>,
    rows: usize,
    cols: usize,
    grid: Vec<Vec<char>>,
    row: usize,
    col: usize,
}

impl PtyReader {
    pub fn new(master: OwnedFd) -> PtyReader {
        Self::with_size(master, PTY_ROWS, PTY_COLS)
    }

    /// A reader whose grid matches the pty's actual geometry (the tests that
    /// match rendered needles open their pty at their own size).
    pub fn with_size(master: OwnedFd, rows: usize, cols: usize) -> PtyReader {
        let fd = master.as_raw_fd();
        fcntl(fd, F_SETFL(OFlag::O_NONBLOCK)).expect("pty master non-blocking");
        PtyReader {
            file: master.into(),
            output: Vec::new(),
            rows,
            cols,
            grid: vec![vec![' '; cols]; rows],
            row: 0,
            col: 0,
        }
    }

    /// Write to the child (typing, control sequences).
    pub fn write_typed(&mut self, text: &str) {
        self.file.write_all(text.as_bytes()).expect("type text");
    }

    /// Write raw bytes to the child (control sequences the tests build by hand).
    pub fn write(&mut self, payload: &[u8]) {
        self.file.write_all(payload).expect("write to the pty");
    }

    /// A write that tolerates the child being gone: a closed master fails
    /// with EIO — no answer, not a failure.
    pub fn try_write(&mut self, payload: &[u8]) {
        let _ = self.file.write_all(payload);
        let _ = self.file.flush();
    }

    pub fn put(&mut self, ch: char) {
        if self.row < self.rows && self.col < self.cols {
            self.grid[self.row][self.col] = ch;
        }
        self.col = (self.col + 1).min(self.cols - 1);
    }

    /// Feed the raw bytes through the grid: text lands at the cursor, CSI
    /// cursor/erase sequences move or clear it, and every other escape
    /// (SGR, private modes, OSC) is consumed.
    pub fn feed(&mut self, data: &[u8]) {
        let bytes = data;
        let mut i = 0;
        while i < bytes.len() {
            let byte = bytes[i];
            if byte == 0x1b {
                match bytes.get(i + 1) {
                    Some(b'[') => {
                        let mut j = i + 2;
                        let start = j;
                        while j < bytes.len() && !(0x40..=0x7e).contains(&bytes[j]) {
                            j += 1;
                        }
                        if j >= bytes.len() {
                            break;
                        }
                        self.csi(&String::from_utf8_lossy(&bytes[start..j]), bytes[j]);
                        i = j + 1;
                    }
                    Some(b']') => {
                        let mut j = i + 2;
                        let mut consumed = false;
                        while j < bytes.len() {
                            if bytes[j] == 0x07 {
                                i = j + 1;
                                consumed = true;
                                break;
                            }
                            if bytes[j] == 0x1b {
                                let terminator = bytes.get(j + 1) == Some(&b'\\');
                                i = if terminator { j + 2 } else { j };
                                consumed = true;
                                break;
                            }
                            j += 1;
                        }
                        if !consumed {
                            i = j;
                        }
                    }
                    Some(_) => i += 2,
                    None => break,
                }
                continue;
            }
            match byte {
                b'\n' => {
                    self.row = (self.row + 1).min(self.rows - 1);
                    i += 1;
                }
                b'\r' => {
                    self.col = 0;
                    i += 1;
                }
                byte if byte < 0x20 => i += 1,
                byte if byte < 0x80 => {
                    self.put(byte as char);
                    i += 1;
                }
                _ => {
                    let len = if byte < 0xe0 {
                        2
                    } else if byte < 0xf0 {
                        3
                    } else {
                        4
                    };
                    let end = (i + len).min(bytes.len());
                    if let Ok(text) = std::str::from_utf8(&bytes[i..end]) {
                        if let Some(ch) = text.chars().next() {
                            self.put(ch);
                        }
                    }
                    i = end;
                }
            }
        }
    }

    fn csi(&mut self, params: &str, final_byte: u8) {
        let params = params.trim_start_matches(['?', '>', '<', '=']);
        let number = |index: usize, default: usize| -> usize {
            params
                .split(';')
                .nth(index)
                .and_then(|value| value.parse::<usize>().ok())
                .unwrap_or(default)
        };
        match final_byte {
            b'H' | b'f' => {
                self.row = number(0, 1).saturating_sub(1).min(self.rows - 1);
                self.col = number(1, 1).saturating_sub(1).min(self.cols - 1);
            }
            b'A' => self.row = self.row.saturating_sub(number(0, 1)),
            b'B' => self.row = (self.row + number(0, 1)).min(self.rows - 1),
            b'C' => self.col = (self.col + number(0, 1)).min(self.cols - 1),
            b'D' => self.col = self.col.saturating_sub(number(0, 1)),
            b'G' => self.col = number(0, 1).saturating_sub(1).min(self.cols - 1),
            b'J' => {
                let mode = number(0, 0);
                if mode == 2 || mode == 3 {
                    self.grid = vec![vec![' '; self.cols]; self.rows];
                }
            }
            _ => {}
        }
    }

    /// The rendered grid as text.
    pub fn rendered(&self) -> String {
        self.grid
            .iter()
            .map(|row| row.iter().collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// One non-blocking read; `Ok(0)` when nothing was pending (the caller
    /// waits) — the drive-cycle helpers poll the master directly.
    pub fn try_read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        self.file.read(buffer)
    }

    /// Read whatever the child produced since the last poll. One 8 KiB
    /// bufferful per call for the bounded-wait loops; `drain` (the callers
    /// that must empty the kernel buffer) loops it.
    /// `Ok(bytes read)`; 0 = nothing pending.
    pub fn pump(&mut self) -> usize {
        let mut buffer = [0u8; 8192];
        match self.file.read(&mut buffer) {
            Ok(0) | Err(_) => 0,
            Ok(n) => {
                self.output.extend_from_slice(&buffer[..n]);
                self.feed(&buffer[..n]);
                n
            }
        }
    }

    /// Drain every pending byte (the bounded-query race: a wait that sleeps
    /// between 8 KiB reads loses a 250 ms window against a 128 KiB burst).
    pub fn drain(&mut self) {
        loop {
            let mut buffer = [0u8; 8192];
            match self.file.read(&mut buffer) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    self.output.extend_from_slice(&buffer[..n]);
                    self.feed(&buffer[..n]);
                }
            }
        }
    }

    /// The current stream position (the mark for the `wait_from*` family).
    pub fn mark(&self) -> usize {
        self.output.len()
    }

    /// The bytes collected since the given mark.
    pub fn since(&self, mark: usize) -> &[u8] {
        &self.output[mark..]
    }

    /// The whole raw stream collected so far.
    pub fn output(&self) -> &[u8] {
        &self.output
    }

    /// Read until quiet for `quiet_polls` consecutive empty polls: a write
    /// into a full kernel buffer is dropped, so tests drain to silence
    /// before every resume write.
    pub fn drain_until_quiet(&mut self, quiet_polls: usize) {
        let mut quiet = 0;
        while quiet < quiet_polls {
            let mut buffer = [0u8; 8192];
            match self.file.read(&mut buffer) {
                Ok(0) | Err(_) => quiet += 1,
                Ok(n) => {
                    self.output.extend_from_slice(&buffer[..n]);
                    self.feed(&buffer[..n]);
                    quiet = 0;
                }
            }
            std::thread::sleep(Duration::from_millis(25));
        }
    }

    fn find_since(&self, mark: usize, needle: &[u8]) -> Option<usize> {
        find_subsequence(&self.output[mark..], needle).map(|at| mark + at)
    }

    /// Drain until the needle appears in the raw stream, bounded by the
    /// deadline; returns the raw offset where the needle first appeared
    /// (the ordering oracle's evidence). Panics with the artifact dump.
    pub fn wait_for(&mut self, needle: &[u8], what: &str, window_ms: u64) -> usize {
        let deadline = Instant::now() + Duration::from_millis(window_ms);
        loop {
            if let Some(at) = self.find_since(0, needle) {
                return at;
            }
            let read = self.pump();
            if Instant::now() > deadline {
                self.dump_artifacts(what);
                panic!("timeout waiting for {what} (needle {needle:?})");
            }
            if read < 8192 {
                std::thread::sleep(Duration::from_millis(20));
            }
        }
    }

    /// Drain until the needle appears AFTER the mark (the ordering oracle:
    /// the evidence must postdate the mark), the copies' 30s bound.
    pub fn wait_from(&mut self, mark: usize, needle: &[u8], what: &str) -> usize {
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            if let Some(at) = self.find_since(mark, needle) {
                return at;
            }
            let read = self.pump();
            if Instant::now() > deadline {
                self.dump_artifacts(what);
                panic!("timeout waiting for {what} (needle {needle:?} after mark {mark})");
            }
            if read < 8192 {
                std::thread::sleep(Duration::from_millis(20));
            }
        }
    }

    /// `wait_from` with a caller-chosen bound (a regression's starvation
    /// window can be shorter than the default bound).
    pub fn wait_from_bounded(
        &mut self,
        mark: usize,
        needle: &[u8],
        what: &str,
        bound: Duration,
    ) -> usize {
        let deadline = Instant::now() + bound;
        loop {
            if let Some(at) = self.find_since(mark, needle) {
                return at;
            }
            self.pump();
            if Instant::now() > deadline {
                self.dump_artifacts(what);
                panic!("{what} missed the {bound:?} bound (needle {needle:?})");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// `wait_from(0, ..)`: the needle must appear anywhere in the stream.
    pub fn wait_from_start(&mut self, needle: &[u8], what: &str) -> usize {
        self.wait_from(0, needle, what)
    }

    /// Drain until the needle appears in the RENDERED grid; returns the raw
    /// offset at which the needle first became visible.
    pub fn wait_for_screen(&mut self, needle: &str, what: &str, window_ms: u64) -> usize {
        let deadline = Instant::now() + Duration::from_millis(window_ms);
        let mut seen_at = None;
        loop {
            if seen_at.is_none() && self.rendered().contains(needle) {
                seen_at = Some(self.output.len());
            }
            if let Some(at) = seen_at {
                return at;
            }
            self.pump();
            if Instant::now() > deadline {
                self.dump_artifacts(what);
                panic!("timeout waiting for {what} (screen needle {needle:?})");
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// The quiescence oracle: resolve when the child has produced no new
    /// output for `stable_ms` (and has produced some), bounded by
    /// `window_ms`. This is the settle primitive the fixed sleeps replace.
    pub fn wait_for_stable(&mut self, stable_ms: u64, what: &str, window_ms: u64) {
        let deadline = Instant::now() + Duration::from_millis(window_ms);
        let mut last_growth = Instant::now();
        let mut last_len = self.output.len();
        loop {
            self.pump();
            if self.output.len() != last_len {
                last_len = self.output.len();
                last_growth = Instant::now();
            } else if last_len > 0 && last_growth.elapsed() >= Duration::from_millis(stable_ms) {
                return;
            }
            if Instant::now() > deadline {
                self.dump_artifacts(what);
                panic!(
                    "timeout waiting for {what}: the stream never settled ({stable_ms}ms stable, \
                     {last_len} bytes total)"
                );
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// Dump the raw stream and the rendered grid under
    /// `.tmp/e2e-artifacts/<label>-<unix_ms>/`; every wait's failure path
    /// calls this, so a red run reconstructs without a re-run.
    pub fn dump_artifacts(&self, label: &str) {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or_default();
        let dir = artifacts_dir().join(format!("{label}-{}-{stamp}", std::process::id()));
        if std::fs::create_dir_all(&dir).is_err() {
            return;
        }
        let _ = std::fs::write(dir.join("raw.log"), &self.output);
        let _ = std::fs::write(dir.join("screen.txt"), self.rendered());
        eprintln!("artifacts: {}", dir.display());
    }
}

/// Find `needle` in `haystack` (the copies' shared helper).
pub fn find_subsequence(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}
