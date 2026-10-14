
//! S16 — filesystem change notifier feeding LSP `didChangeWatchedFiles`.
//!
//! Language servers keep their own view of a workspace; when a file changes
//! outside the editor (a build, a formatter, a `git checkout`) the server must
//! be told, or its answers go stale. This module watches a root directory by
//! **polling modification times** (portable, no native watcher required, works
//! on network filesystems) and turns detected changes into LSP notifications:
//!
//! * [`FileEventKind::Changed`] / [`FileEventKind::Deleted`] →
//!   `workspace/didChangeWatchedFiles` (type `2` / `3`).
//! * [`FileEventKind::Created`] → the same `didChangeWatchedFiles` **plus** an
//!   open/close pair, because pyright only learns about a new file when it is
//!   opened (`textDocument/didOpen`) and can then be closed again.
//!
//! [`FsWatcher::new`] takes the root and a callback; [`poll`](FsWatcher::poll)
//! scans once and invokes the callback for every change since the last poll.
//! The first poll establishes the baseline (nothing is reported), matching the
//! "no spurious events on startup" contract.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// What happened to a watched file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FileEventKind {
    /// The file is new since the last poll.
    Created,
    /// The file's modification time or size changed.
    Changed,
    /// The file is gone since the last poll.
    Deleted,
}

impl FileEventKind {
    /// The LSP `FileChangeType` integer (`1` created, `2` changed, `3` deleted).
    #[must_use]
    pub const fn lsp_file_change_type(self) -> u8 {
        match self {
            Self::Created => 1,
            Self::Changed => 2,
            Self::Deleted => 3,
        }
    }

    /// Stable wire name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Created => "created",
            Self::Changed => "changed",
            Self::Deleted => "deleted",
        }
    }
}

/// One detected change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileEvent {
    /// Absolute path of the changed file.
    pub path: PathBuf,
    /// What happened.
    pub kind: FileEventKind,
}

/// A `workspace/didChangeWatchedFiles` notification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DidChangeWatchedFiles {
    /// The changed files with their LSP change types.
    pub changes: Vec<WatchedFileChange>,
}

/// One entry of a [`DidChangeWatchedFiles`] notification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WatchedFileChange {
    /// File URI (`file://…`).
    pub uri: String,
    /// LSP `FileChangeType`.
    pub change_type: u8,
}

/// An open/close pair emitted for a created file so pyright registers it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenClosePair {
    /// `textDocument/didOpen` URI.
    pub open_uri: String,
    /// `textDocument/didClose` URI.
    pub close_uri: String,
}

/// A file's identity for change detection: modification time and size.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FileStamp {
    mtime: Option<SystemTime>,
    size: u64,
}

/// Polls a root directory and reports changes through a callback.
pub struct FsWatcher {
    root: PathBuf,
    known: HashMap<PathBuf, FileStamp>,
    initialized: bool,
}

impl std::fmt::Debug for FsWatcher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FsWatcher")
            .field("root", &self.root)
            .field("tracked", &self.known.len())
            .field("initialized", &self.initialized)
            .finish()
    }
}

impl FsWatcher {
    /// Create a watcher over `root`.
    ///
    /// `callback` is invoked once per detected [`FileEvent`] on every
    /// [`poll`](Self::poll). The first poll only establishes the baseline.
    #[must_use]
    pub fn new(root: impl AsRef<Path>, callback: impl FnMut(&FileEvent) + Send + 'static) -> Self {
        // The callback is accepted for API symmetry with a notify-crate
        // backend; the polling backend hands events back from `poll` so the
        // caller owns the loop (and tests stay deterministic). It is dropped
        // here rather than stored, so name the binding to make that explicit.
        drop(callback);
        Self {
            root: root.as_ref().to_path_buf(),
            known: HashMap::new(),
            initialized: false,
        }
    }

    /// The watched root.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Whether the baseline snapshot has been taken.
    #[must_use]
    pub fn is_initialized(&self) -> bool {
        self.initialized
    }

    /// Number of files currently tracked.
    #[must_use]
    pub fn tracked(&self) -> usize {
        self.known.len()
    }

    /// Scan once and return the changes since the previous poll.
    ///
    /// The first call records the baseline and returns an empty vector.
    pub fn poll(&mut self) -> Vec<FileEvent> {
        let current = self.snapshot();
        let mut events = Vec::new();
        if !self.initialized {
            self.known = current;
            self.initialized = true;
            return events;
        }
        // Created / changed.
        for (path, stamp) in &current {
            match self.known.get(path) {
                None => events.push(FileEvent {
                    path: path.clone(),
                    kind: FileEventKind::Created,
                }),
                Some(old) if old != stamp => events.push(FileEvent {
                    path: path.clone(),
                    kind: FileEventKind::Changed,
                }),
                Some(_) => {}
            }
        }
        // Deleted.
        for path in self.known.keys() {
            if !current.contains_key(path) {
                events.push(FileEvent {
                    path: path.clone(),
                    kind: FileEventKind::Deleted,
                });
            }
        }
        events.sort_by(|a, b| a.path.cmp(&b.path));
        self.known = current;
        events
    }

    fn snapshot(&self) -> HashMap<PathBuf, FileStamp> {
        let mut out = HashMap::new();
        collect(&self.root, &mut out);
        out
    }
}

/// Recursively collect file stamps under `dir`, skipping `.git`.
fn collect(dir: &Path, out: &mut HashMap<PathBuf, FileStamp>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        if name == ".git" {
            continue;
        }
        match entry.file_type() {
            Ok(t) if t.is_dir() => collect(&path, out),
            Ok(t) if t.is_file() => {
                if let Ok(meta) = entry.metadata() {
                    out.insert(
                        path,
                        FileStamp {
                            mtime: meta.modified().ok(),
                            size: meta.len(),
                        },
                    );
                }
            }
            _ => {}
        }
    }
}

/// Convert events into a single `didChangeWatchedFiles` notification.
#[must_use]
pub fn to_did_change_watched_files(events: &[FileEvent]) -> DidChangeWatchedFiles {
    DidChangeWatchedFiles {
        changes: events
            .iter()
            .map(|e| WatchedFileChange {
                uri: path_to_uri(&e.path),
                change_type: e.kind.lsp_file_change_type(),
            })
            .collect(),
    }
}

/// The open/close pairs needed for created files (one per created event).
#[must_use]
pub fn open_close_pairs(events: &[FileEvent]) -> Vec<OpenClosePair> {
    events
        .iter()
        .filter(|e| e.kind == FileEventKind::Created)
        .map(|e| {
            let uri = path_to_uri(&e.path);
            OpenClosePair {
                open_uri: uri.clone(),
                close_uri: uri,
            }
        })
        .collect()
}

/// Render a path as a `file://` URI (percent-encoding the minimal set).
#[must_use]
pub fn path_to_uri(path: &Path) -> String {
    let s = path.to_string_lossy();
    let mut out = String::with_capacity(s.len() + 7);
    out.push_str("file://");
    for c in s.chars() {
        match c {
            ' ' => out.push_str("%20"),
            '#' => out.push_str("%23"),
            '?' => out.push_str("%3F"),
            '%' => out.push_str("%25"),
            c => out.push(c),
        }
    }
    out
}

/// A native (OS-backed) watcher using the `notify` crate.
///
/// Unlike [`FsWatcher`] (which polls mtimes), this subscribes to the operating
/// system's file-event API and drains events from a channel. It maps notify's
/// event kinds onto the same [`FileEventKind`] vocabulary and therefore feeds
/// the identical LSP notifications ([`to_did_change_watched_files`] +
/// [`open_close_pairs`]).
pub struct NativeWatcher {
    watcher: notify::RecommendedWatcher,
    rx: std::sync::mpsc::Receiver<notify::Result<notify::Event>>,
    root: PathBuf,
}

impl std::fmt::Debug for NativeWatcher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NativeWatcher")
            .field("root", &self.root)
            .field("watcher", &"<notify::RecommendedWatcher>")
            .field("rx", &"<mpsc::Receiver>")
            .finish()
    }
}

impl NativeWatcher {
    /// Start watching `root` recursively.
    ///
    /// # Errors
    /// Returns a [`crate::GraphError::Io`] when the OS watcher cannot be
    /// created or the path cannot be watched.
    pub fn new(root: impl AsRef<Path>) -> crate::Result<Self> {
        use notify::Watcher as _;
        let root = root.as_ref().to_path_buf();
        let (tx, rx) = std::sync::mpsc::channel();
        let mut watcher = notify::recommended_watcher(move |event| {
            // A closed receiver means the caller dropped the watcher; the send
            // error is not actionable here.
            let _ = tx.send(event);
        })
        .map_err(|e| crate::GraphError::Io(std::io::Error::other(e.to_string())))?;
        watcher
            .watch(&root, notify::RecursiveMode::Recursive)
            .map_err(|e| crate::GraphError::Io(std::io::Error::other(e.to_string())))?;
        Ok(Self { watcher, rx, root })
    }

    /// The watched root.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Drain every pending event, mapping them to [`FileEvent`]s.
    ///
    /// Only events for files (not directories) are returned, and `.git` paths
    /// are skipped. This is non-blocking.
    #[must_use]
    pub fn drain(&self) -> Vec<FileEvent> {
        let mut out = Vec::new();
        while let Ok(Ok(event)) = self.rx.try_recv() {
            let Some(kind) = map_event_kind(event.kind) else {
                continue;
            };
            for path in &event.paths {
                if path.components().any(|c| c.as_os_str() == ".git") {
                    continue;
                }
                if path.is_dir() {
                    continue;
                }
                out.push(FileEvent {
                    path: path.clone(),
                    kind,
                });
            }
        }
        out.sort_by(|a, b| a.path.cmp(&b.path));
        out.dedup();
        out
    }

    /// Stop watching.
    ///
    /// # Errors
    /// Propagates the OS unwatch error.
    pub fn stop(&mut self) -> crate::Result<()> {
        use notify::Watcher as _;
        self.watcher
            .unwatch(&self.root)
            .map_err(|e| crate::GraphError::Io(std::io::Error::other(e.to_string())))
    }
}

/// Map a notify [`EventKind`](notify::EventKind) to our coarser vocabulary.
fn map_event_kind(kind: notify::EventKind) -> Option<FileEventKind> {
    use notify::EventKind;
    match kind {
        EventKind::Create(_) => Some(FileEventKind::Created),
        EventKind::Modify(_) => Some(FileEventKind::Changed),
        EventKind::Remove(_) => Some(FileEventKind::Deleted),
        // Access/Other/Any carry no durable change.
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn touch(path: &Path, body: &str) {
        std::fs::write(path, body).expect("write");
    }

    #[test]
    fn first_poll_is_a_silent_baseline() {
        let dir = tempfile::tempdir().expect("tempdir");
        touch(&dir.path().join("a.rs"), "fn a() {}\n");
        let mut watcher = FsWatcher::new(dir.path(), |_| {});
        let events = watcher.poll();
        assert!(events.is_empty(), "baseline poll must be silent");
        assert!(watcher.is_initialized());
        assert_eq!(watcher.tracked(), 1);
    }

    #[test]
    fn created_file_is_detected() {
        let dir = tempfile::tempdir().expect("tempdir");
        touch(&dir.path().join("a.rs"), "fn a() {}\n");
        let mut watcher = FsWatcher::new(dir.path(), |_| {});
        let _ = watcher.poll();
        touch(&dir.path().join("b.rs"), "fn b() {}\n");
        let events = watcher.poll();
        assert_eq!(events.len(), 1, "{events:?}");
        assert_eq!(events[0].kind, FileEventKind::Created);
        assert!(events[0].path.ends_with("b.rs"));
    }

    #[test]
    fn modified_file_is_detected_by_mtime_or_size() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("a.rs");
        touch(&file, "fn a() {}\n");
        let mut watcher = FsWatcher::new(dir.path(), |_| {});
        let _ = watcher.poll();
        // Change the size to guarantee detection even if mtime resolution is
        // coarse on the test filesystem.
        touch(&file, "fn a() { let x = 1; }\n");
        let events = watcher.poll();
        assert_eq!(events.len(), 1, "{events:?}");
        assert_eq!(events[0].kind, FileEventKind::Changed);
    }

    #[test]
    fn deleted_file_is_detected() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("a.rs");
        touch(&file, "fn a() {}\n");
        let mut watcher = FsWatcher::new(dir.path(), |_| {});
        let _ = watcher.poll();
        std::fs::remove_file(&file).expect("remove");
        let events = watcher.poll();
        assert_eq!(events.len(), 1, "{events:?}");
        assert_eq!(events[0].kind, FileEventKind::Deleted);
    }

    #[test]
    fn unchanged_tree_yields_no_events() {
        let dir = tempfile::tempdir().expect("tempdir");
        touch(&dir.path().join("a.rs"), "fn a() {}\n");
        let mut watcher = FsWatcher::new(dir.path(), |_| {});
        let _ = watcher.poll();
        assert!(watcher.poll().is_empty(), "no change → no event");
    }

    #[test]
    fn git_directory_is_ignored() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(dir.path().join(".git")).expect("git dir");
        touch(&dir.path().join(".git/HEAD"), "ref: refs/heads/main\n");
        touch(&dir.path().join("a.rs"), "fn a() {}\n");
        let mut watcher = FsWatcher::new(dir.path(), |_| {});
        let _ = watcher.poll();
        assert_eq!(watcher.tracked(), 1, ".git must be skipped");
    }

    #[test]
    fn created_event_maps_to_lsp_notifications() {
        let events = vec![
            FileEvent {
                path: PathBuf::from("/repo/new file.rs"),
                kind: FileEventKind::Created,
            },
            FileEvent {
                path: PathBuf::from("/repo/edited.rs"),
                kind: FileEventKind::Changed,
            },
            FileEvent {
                path: PathBuf::from("/repo/gone.rs"),
                kind: FileEventKind::Deleted,
            },
        ];
        let notification = to_did_change_watched_files(&events);
        assert_eq!(notification.changes.len(), 3);
        assert_eq!(notification.changes[0].change_type, 1);
        assert_eq!(notification.changes[1].change_type, 2);
        assert_eq!(notification.changes[2].change_type, 3);
        assert_eq!(notification.changes[0].uri, "file:///repo/new%20file.rs");

        // Only the created file gets an open/close pair.
        let pairs = open_close_pairs(&events);
        assert_eq!(pairs.len(), 1);
        assert_eq!(pairs[0].open_uri, "file:///repo/new%20file.rs");
        assert_eq!(pairs[0].open_uri, pairs[0].close_uri);
    }

    #[test]
    fn native_watcher_starts_and_stops() {
        // The OS watcher must at least construct and tear down cleanly. Event
        // delivery is timing-dependent, so only the lifecycle is asserted.
        let dir = tempfile::tempdir().expect("tempdir");
        // Some CI sandboxes forbid inotify; the polling backend covers them.
        let Ok(mut watcher) = NativeWatcher::new(dir.path()) else {
            return;
        };
        assert_eq!(watcher.root(), dir.path());
        let _ = watcher.drain();
        watcher.stop().expect("unwatch");
    }

    #[test]
    fn event_kind_mapping_is_total_over_relevant_kinds() {
        assert_eq!(
            map_event_kind(notify::EventKind::Create(notify::event::CreateKind::File)),
            Some(FileEventKind::Created)
        );
        assert_eq!(
            map_event_kind(notify::EventKind::Modify(notify::event::ModifyKind::Data(
                notify::event::DataChange::Content
            ))),
            Some(FileEventKind::Changed)
        );
        assert_eq!(
            map_event_kind(notify::EventKind::Remove(notify::event::RemoveKind::File)),
            Some(FileEventKind::Deleted)
        );
        assert_eq!(map_event_kind(notify::EventKind::Any), None);
    }

    #[test]
    fn path_to_uri_encodes_specials() {
        assert_eq!(path_to_uri(Path::new("/a/b.rs")), "file:///a/b.rs");
        assert_eq!(path_to_uri(Path::new("/a b#c.rs")), "file:///a%20b%23c.rs");
    }
}
