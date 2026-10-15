//! Rotating debug log subscriber (fork-only): installs a process-global
//! `tracing` subscriber that writes formatted events to
//! `<agentDir>/logs/debug-<component>.log`, rotating by size through a
//! `debug.log -> debug.log.1 -> ... -> debug.log.<maxFiles-1>` rename ladder.
//!
//! The subscriber is opt-in: [`init`] is a no-op unless
//! `pa_core::settings::SettingsManager::get_debug_log()` reports
//! `enabled` (the `debugLog.enabled` setting or the `ALLSPARK_DEBUG`
//! env flag). The daemon supervisor, the session worker, and the CLI each
//! install their own `component` subscriber at process start.
//!
//! SECURITY: this module only ever sees the `tracing` events the codebase
//! emits. The provider HTTP transport deliberately logs method/url/status
//! and never headers, bodies, or tokens (see `pa_ai::oauth::provider_http`).

use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

use pa_core::settings::DebugLogConfig;
use tracing::Subscriber;
use tracing_subscriber::filter::{EnvFilter, LevelFilter};
use tracing_subscriber::fmt::MakeWriter;

/// Env override for the filter directives (module-level control, e.g.
/// `ALLSPARK_DEBUG_FILTER=pa_ai=debug,pa_daemon=trace`). Unset falls back to
/// the configured level as a global directive.
const DEBUG_LOG_FILTER_ENV: &str = "ALLSPARK_DEBUG_FILTER";

/// The installed-subscriber guard: keeps the process-global subscriber alive
/// for as long as the holder lives (the caller stores it for the process
/// lifetime).
#[must_use = "dropping the guard is fine (the subscriber is global) but the token marks the install"]
pub struct DebugLogGuard {
    // The global default already owns its own handle; this Arc keeps the
    // concrete subscriber type alive for the guard's lifetime too.
    _subscriber: Arc<dyn Subscriber + Send + Sync>,
}

/// One component's rotation state: the open active file, its tracked byte
/// length, and the ladder bounds.
struct RotatingState {
    file: File,
    len: u64,
    base_path: PathBuf,
    max_bytes: u64,
    max_files: u32,
}

impl RotatingState {
    /// Open (or create) the active log, appending so a restart keeps history.
    fn open(path: &Path, max_bytes: u64, max_files: u32) -> io::Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let file = OpenOptions::new().create(true).append(true).open(path)?;
        let len = file.metadata()?.len();
        Ok(Self {
            file,
            len,
            base_path: path.to_path_buf(),
            max_bytes,
            max_files,
        })
    }

    /// `<base>.<index>` (e.g. `debug-daemon.log.1`), built from the OS string
    /// so a non-UTF-8 agent dir survives.
    fn rotation_path(&self, index: u32) -> PathBuf {
        let mut name = self.base_path.clone().into_os_string();
        name.push(format!(".{index}"));
        PathBuf::from(name)
    }

    /// Shift the ladder and reopen a fresh active file. Best-effort: a failed
    /// rename/unlink never aborts logging.
    fn rotate(&mut self) -> io::Result<()> {
        if self.max_files >= 2 {
            let oldest = self.rotation_path(self.max_files - 1);
            let _ = std::fs::remove_file(&oldest);
            for index in (1..self.max_files - 1).rev() {
                let from = self.rotation_path(index);
                let to = self.rotation_path(index + 1);
                let _ = std::fs::rename(&from, &to);
            }
            let _ = std::fs::rename(&self.base_path, self.rotation_path(1));
        } else {
            // maxFiles == 1: no rotation slot; the active file starts over.
            let _ = std::fs::remove_file(&self.base_path);
        }
        self.file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.base_path)?;
        self.len = 0;
        Ok(())
    }

    /// Append one formatted chunk, rotating first when it would push the
    /// active file past `max_bytes` (an oversized single chunk still lands in
    /// a fresh active file, so no content is lost).
    fn append(&mut self, buf: &[u8]) -> io::Result<usize> {
        if self.len > 0 && self.len.saturating_add(buf.len() as u64) > self.max_bytes {
            let _ = self.rotate();
        }
        let written = self.file.write(buf)?;
        self.len = self.len.saturating_add(written as u64);
        Ok(written)
    }
}

/// The `io::Write` handle `tracing_subscriber` writes each formatted event to.
struct RotatingWriter {
    state: Arc<Mutex<RotatingState>>,
}

impl Write for RotatingWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        state.append(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        state.file.flush()
    }
}

/// The `MakeWriter` the `fmt` subscriber builder takes: hands out one
/// [`RotatingWriter`] per event, all sharing the rotation state.
struct RotatingMakeWriter {
    state: Arc<Mutex<RotatingState>>,
}

impl<'a> MakeWriter<'a> for RotatingMakeWriter {
    type Writer = RotatingWriter;

    fn make_writer(&'a self) -> Self::Writer {
        RotatingWriter {
            state: Arc::clone(&self.state),
        }
    }
}

/// The configured level as a `LevelFilter`; an unknown value reads as `debug`
/// (the settings getter already normalizes, this is the second belt).
fn level_filter(level: &str) -> LevelFilter {
    match level {
        "trace" => LevelFilter::TRACE,
        "info" => LevelFilter::INFO,
        "warn" => LevelFilter::WARN,
        "error" => LevelFilter::ERROR,
        _ => LevelFilter::DEBUG,
    }
}

/// The `ALLSPARK_DEBUG_FILTER` override when set and non-empty.
fn filter_override() -> Option<String> {
    std::env::var(DEBUG_LOG_FILTER_ENV)
        .ok()
        .filter(|value| !value.trim().is_empty())
}

/// Build the formatted subscriber over `state`. With the
/// `ALLSPARK_DEBUG_FILTER` override the full directive grammar applies (e.g.
/// `pa_ai=debug,pa_daemon=trace`); otherwise the configured level is a global
/// threshold. Split out so tests can install it locally (`with_default`)
/// without touching the process-global default.
fn build_subscriber(
    state: Arc<Mutex<RotatingState>>,
    level: &str,
) -> Box<dyn Subscriber + Send + Sync> {
    let writer = RotatingMakeWriter { state };
    match filter_override() {
        Some(directives) => Box::new(
            tracing_subscriber::fmt()
                .with_writer(writer)
                .with_env_filter(EnvFilter::new(directives))
                .with_ansi(false)
                .with_target(true)
                .finish(),
        ),
        None => Box::new(
            tracing_subscriber::fmt()
                .with_writer(writer)
                .with_max_level(level_filter(level))
                .with_ansi(false)
                .with_target(true)
                .finish(),
        ),
    }
}

/// The set of components that already installed a subscriber in this process:
/// a second `init` for the same component is a no-op (never a panic, never a
/// double `set_global_default`).
fn claimed_components() -> &'static Mutex<Vec<String>> {
    static CLAIMED: OnceLock<Mutex<Vec<String>>> = OnceLock::new();
    CLAIMED.get_or_init(|| Mutex::new(Vec::new()))
}

/// Claim `component` for this process; `false` when already claimed.
fn claim(component: &str) -> bool {
    let mut claimed = claimed_components()
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    if claimed.iter().any(|name| name == component) {
        return false;
    }
    claimed.push(component.to_string());
    true
}

/// Release a claim after a failed install so a later retry can succeed.
fn unclaim(component: &str) {
    let mut claimed = claimed_components()
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    claimed.retain(|name| name != component);
}

/// Read the debug-log settings from `agent_dir` and install the subscriber
/// for `component`. The single entry point the supervisor, worker, and CLI
/// share: `None` when the gate is off (the common case — zero cost).
#[must_use]
pub fn init_from_settings(agent_dir: &Path, component: &str) -> Option<DebugLogGuard> {
    let settings = pa_core::settings::SettingsManager::create(
        std::env::current_dir().unwrap_or_default(),
        agent_dir,
    );
    init(agent_dir, component, settings.get_debug_log())
}

/// Install the process-global rotating debug subscriber for `component`
/// (`"daemon" | "worker" | "cli"`), writing to
/// `<agent_dir>/logs/debug-<component>.log`.
///
/// Returns `None` (a no-op) when `cfg.enabled` is false, when this component
/// already installed a subscriber in this process, or when the log file cannot
/// be opened. Idempotent: a repeated call never panics.
#[must_use]
// The config is taken by value deliberately: it is a small options struct the
// caller builds per process start (the documented public surface), and the
// alternative `&DebugLogConfig` buys nothing at one call site.
#[allow(clippy::needless_pass_by_value)]
pub fn init(agent_dir: &Path, component: &str, cfg: DebugLogConfig) -> Option<DebugLogGuard> {
    if !cfg.enabled {
        return None;
    }
    if !claim(component) {
        return None;
    }
    let path = agent_dir
        .join("logs")
        .join(format!("debug-{component}.log"));
    let state = match RotatingState::open(&path, cfg.max_bytes, cfg.max_files) {
        Ok(state) => Arc::new(Mutex::new(state)),
        Err(error) => {
            unclaim(component);
            tracing::warn!(
                component,
                path = %path.display(),
                %error,
                "debug log subscriber not installed"
            );
            return None;
        }
    };
    let subscriber: Arc<dyn Subscriber + Send + Sync> =
        Arc::from(build_subscriber(state, &cfg.level));
    if let Err(error) = tracing::subscriber::set_global_default(Arc::clone(&subscriber)) {
        unclaim(component);
        eprintln!("debug log subscriber not installed: {error}");
        return None;
    }
    Some(DebugLogGuard {
        _subscriber: subscriber,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state_for(path: &Path, max_bytes: u64, max_files: u32) -> Arc<Mutex<RotatingState>> {
        Arc::new(Mutex::new(
            RotatingState::open(path, max_bytes, max_files).expect("open rotating state"),
        ))
    }

    fn append(state: &Arc<Mutex<RotatingState>>, chunk: &[u8]) {
        let mut guard = state.lock().unwrap_or_else(PoisonError::into_inner);
        guard.append(chunk).expect("append chunk");
    }

    fn read(path: &Path) -> String {
        std::fs::read_to_string(path).unwrap_or_default()
    }

    /// A write past `max_bytes` rotates: the previous content moves to `.1`,
    /// the active file keeps only the new content, and the chain respects
    /// `max_files`.
    #[test]
    fn rotation_shifts_the_ladder_and_bounds_file_count() {
        let dir = tempfile::tempdir().expect("temp dir");
        let base = dir.path().join("logs/debug-daemon.log");
        // 16-byte bound, 3 files total (active + .1 + .2).
        let state = state_for(&base, 16, 3);

        append(&state, b"aaaaaaaaaa"); // 10 bytes, active=10
        append(&state, b"bbbbbbbbbb"); // would be 20 > 16 -> rotate; active=10 (.1="aaaaaaaaaa")
        assert_eq!(
            read(&base),
            "bbbbbbbbbb",
            "active holds only the newest chunk"
        );
        assert_eq!(
            read(&base.with_file_name("debug-daemon.log.1")),
            "aaaaaaaaaa"
        );

        append(&state, b"cccccccccc"); // rotate again: .1="bbbbbbbbbb", .2="aaaaaaaaaa"
        append(&state, b"dddddddddd"); // rotate: oldest (.2) is dropped, .1="cccccccccc", .2="bbbbbbbbbb"
        assert_eq!(read(&base), "dddddddddd");
        assert_eq!(
            read(&base.with_file_name("debug-daemon.log.1")),
            "cccccccccc"
        );
        assert_eq!(
            read(&base.with_file_name("debug-daemon.log.2")),
            "bbbbbbbbbb"
        );
        assert!(
            !base.with_file_name("debug-daemon.log.3").exists(),
            "the ladder never grows past max_files"
        );
    }

    /// `max_files == 1` has no rotation slot: the active file starts over.
    #[test]
    fn single_file_rotation_truncates_in_place() {
        let dir = tempfile::tempdir().expect("temp dir");
        let base = dir.path().join("logs/debug-cli.log");
        let state = state_for(&base, 8, 1);
        append(&state, b"11111111");
        append(&state, b"22222222"); // 16 > 8 -> truncate + write
        assert_eq!(read(&base), "22222222");
        assert!(
            !base.with_file_name("debug-cli.log.1").exists(),
            "max_files=1 keeps no rotation slot"
        );
    }

    /// A single event larger than `max_bytes` is still written whole (no data
    /// loss): the empty-active guard prevents an endless rotate loop.
    #[test]
    fn oversized_single_chunk_lands_intact() {
        let dir = tempfile::tempdir().expect("temp dir");
        let base = dir.path().join("logs/debug-worker.log");
        let state = state_for(&base, 4, 3);
        append(&state, b"way-too-long-for-the-bound");
        assert_eq!(read(&base), "way-too-long-for-the-bound");
    }

    /// The subscriber routes a locally-scoped event into the component file
    /// (thread-local default: the process-global subscriber is untouched).
    #[test]
    fn subscriber_writes_events_to_the_component_file() {
        let dir = tempfile::tempdir().expect("temp dir");
        let base = dir.path().join("logs/debug-cli.log");
        let subscriber = build_subscriber(state_for(&base, 1024 * 1024, 10), "debug");
        tracing::subscriber::with_default(subscriber, || {
            tracing::debug!(answer = 42, "hello from the debug log");
        });
        let contents = read(&base);
        assert!(
            contents.contains("hello from the debug log"),
            "event landed in the file: {contents}"
        );
    }

    /// The configured level filters: a `debug` event is dropped under an
    /// `info` threshold, an `info` event passes.
    #[test]
    fn level_threshold_filters_lower_events() {
        let dir = tempfile::tempdir().expect("temp dir");
        let base = dir.path().join("logs/debug-daemon.log");
        let subscriber = build_subscriber(state_for(&base, 1024 * 1024, 10), "info");
        tracing::subscriber::with_default(subscriber, || {
            tracing::debug!("dropped-below-threshold");
            tracing::info!("kept-at-threshold");
        });
        let contents = read(&base);
        assert!(
            contents.contains("kept-at-threshold"),
            "info passes: {contents}"
        );
        assert!(
            !contents.contains("dropped-below-threshold"),
            "debug is filtered under info: {contents}"
        );
    }

    /// `init` with the gate off is a no-op: no subscriber, no file.
    #[test]
    fn disabled_init_is_a_noop() {
        let dir = tempfile::tempdir().expect("temp dir");
        let cfg = DebugLogConfig::default();
        assert!(!cfg.enabled);
        assert!(init(dir.path(), "cli", cfg).is_none());
        assert!(!dir.path().join("logs/debug-cli.log").exists());
    }

    /// The level mapping normalizes unknown values to `debug` (never a panic).
    #[test]
    fn level_filter_defaults_to_debug() {
        assert_eq!(level_filter("trace"), LevelFilter::TRACE);
        assert_eq!(level_filter("error"), LevelFilter::ERROR);
        assert_eq!(level_filter("nonsense"), LevelFilter::DEBUG);
    }
}
