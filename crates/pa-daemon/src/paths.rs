//! Filesystem layout: agent dir, sessions dir, logs, worker descriptors.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use anyhow::{anyhow, Result};
use sha2::{Digest, Sha256};

pub const AGENT_DIR_ENV: &str = "PRIME_AGENT_CODING_AGENT_DIR";
pub const SESSION_DIR_ENV: &str = "PRIME_AGENT_SESSION_DIR";
pub const CONFIG_DIR_NAME: &str = ".prime/agent";

/// The home directory for state layout. Unresolvable home is an explicit
/// error, not a degraded `/tmp` default: the daemon must refuse to start
/// rather than write durable state outside the user profile.
///
/// # Errors
///
/// Returns an error when the home directory cannot be resolved.
pub fn home_dir() -> Result<PathBuf> {
    pa_types::platform::home_dir()
        .ok_or_else(|| anyhow!("home directory not found: set HOME (or USERPROFILE on Windows)"))
}

/// Expand a leading `~`/`~/` against [`home_dir`]; other paths pass through.
///
/// # Errors
///
/// Returns an error when expanding `~`/`~/` needs the home directory and
/// [`home_dir`] cannot resolve it.
pub fn expand_tilde(path: &str) -> Result<PathBuf> {
    if let Some(rest) = path.strip_prefix("~/") {
        Ok(home_dir()?.join(rest))
    } else if path == "~" {
        home_dir()
    } else {
        Ok(PathBuf::from(path))
    }
}

/// The agent state root: the `PRIME_AGENT_CODING_AGENT_DIR` override when
/// set (tilde expanded), else `.prime/agent` under the home directory.
///
/// # Errors
///
/// Returns an error when the override cannot be tilde-expanded, or when the
/// fallback needs the home directory and [`home_dir`] cannot resolve it.
pub fn agent_dir() -> Result<PathBuf> {
    match std::env::var_os(AGENT_DIR_ENV) {
        Some(dir) if !dir.is_empty() => expand_tilde(&dir.to_string_lossy()),
        _ => Ok(home_dir()?.join(CONFIG_DIR_NAME)),
    }
}

/// The sessions root: the `PRIME_AGENT_SESSION_DIR` override when set
/// (tilde expanded), else `<agent-dir>/sessions`.
///
/// # Errors
///
/// Returns an error when the override cannot be tilde-expanded.
pub fn sessions_dir(agent_dir: &Path) -> Result<PathBuf> {
    match std::env::var_os(SESSION_DIR_ENV) {
        Some(dir) if !dir.is_empty() => expand_tilde(&dir.to_string_lossy()),
        _ => Ok(agent_dir.join("sessions")),
    }
}

#[must_use]
pub fn logs_dir(agent_dir: &Path) -> PathBuf {
    agent_dir.join("logs")
}

/// Create the directory (and any missing parents), then restrict its
/// permissions to the current user.
///
/// # Errors
///
/// Returns an error when directory creation fails; the permission
/// restriction is best effort.
pub fn ensure_dir(path: &Path) -> Result<()> {
    // ONLY a directory this call CREATED gets the 0700 restriction. Asking
    // `create_dir_all` whether the leaf was missing is not enough: the answer is
    // the error kind, and a racing process creating the same path in between makes
    // our mkdir fail with `AlreadyExists` — which reads the same as "someone else
    // owns it" (review #9). `create_dir` is the atomic claim: it succeeds for
    // exactly one caller per path, and only that caller restricts.
    match std::fs::create_dir(path) {
        Ok(()) => {
            let _ = pa_core::platform::perms::restrict_dir(path);
            Ok(())
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            // A parent is missing: build the chain, then claim the leaf.
            if let Some(parent) = path.parent() {
                ensure_dir(parent)?;
            }
            match std::fs::create_dir(path) {
                Ok(()) => {
                    let _ = pa_core::platform::perms::restrict_dir(path);
                    Ok(())
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
                Err(error) => Err(error.into()),
            }
        }
        Err(error) => Err(error.into()),
    }
}

/// sha256 hex, first `chars` characters.
#[must_use]
pub fn hash_key(input: &str, chars: usize) -> String {
    let digest = Sha256::digest(input.as_bytes());
    digest.iter().fold(String::new(), |mut key, b| {
        use std::fmt::Write;
        write!(key, "{b:02x}").expect("write to String");
        key
    })[..chars]
        .to_string()
}

/// Log path for a daemon socket: readable basename plus an 8-char hash of
/// the normalized socket path (TS `getDaemonLogPath`).
#[must_use]
pub fn daemon_log_path(socket_path: &Path, agent_dir: &Path) -> PathBuf {
    let normalized = socket_path.to_string_lossy().to_string();
    let base = socket_path.file_name().map_or_else(
        || "daemon.sock".to_string(),
        |n| n.to_string_lossy().to_string(),
    );
    logs_dir(agent_dir).join(format!("{base}.{}.log", hash_key(&normalized, 8)))
}

/// Rotating log appender: keep the file bounded with one generation
/// rotation (TS `appendRotatingLog`).
pub struct RotatingLog {
    path: PathBuf,
    max_bytes: u64,
}

impl RotatingLog {
    #[must_use]
    pub fn new(path: PathBuf) -> Self {
        RotatingLog {
            path,
            max_bytes: 5 * 1024 * 1024,
        }
    }

    pub fn append(&self, line: &str) {
        // The log lives in `<agent-dir>/logs`; create it if this is the first write (a fresh
        // agent dir has no logs directory yet). The file carries provider-failure detail and
        // worker identities, so it is created owner-only (review #40, CWE-532) — the log may be
        // world-readable FOREVER after creation, so the mode must be right at creation, not
        // "also fixed later".
        if let Some(parent) = self.path.parent() {
            let _ = std::fs::create_dir_all(parent);
            let _ = pa_core::platform::perms::restrict_dir(parent);
        }
        let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
        else {
            return;
        };
        let _ = pa_core::platform::perms::restrict_file(&self.path);
        let payload = format!("{line}\n");
        if let Ok(metadata) = file.metadata() {
            if metadata.len() + payload.len() as u64 > self.max_bytes {
                drop(file);
                let rotated = self.path.with_extension("log.1");
                let _ = std::fs::rename(&self.path, &rotated);
                match std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&self.path)
                {
                    Ok(reopened) => {
                        // The rename moved the RESTRICTED file to `log.1`, and this reopen created
                        // a fresh one with the default mode — so the live log was world-readable
                        // from the first post-rotation line while its predecessor stayed 0600.
                        // That is #40's leak one rename later; restrict the reopened file too, so
                        // every creation path (first write and rotation) sets the mode.
                        let _ = pa_core::platform::perms::restrict_file(&self.path);
                        file = reopened;
                    }
                    Err(_) => return,
                }
            }
        }
        let _ = file.write_all(payload.as_bytes());
    }
}

use std::io::Write as _;

/// The upstream TypeScript product's per-user daemon socket dir name
/// (`prime-agent-<uid>`), beside this product's own (`allspark-<uid>`).
#[cfg(unix)]
const UPSTREAM_TS_SOCKET_DIR_PREFIX: &str = "prime-agent";

/// Whether `socket_path` names the UPSTREAM TypeScript product's daemon —
/// under `<tmpdir>/prime-agent-<uid>/`. Discriminated by PATH, never by argv
/// (both products' payloads are named `prime-agent`, so argv cannot tell them
/// apart; known bug #22). This binary's own daemon never lives there: its
/// default dir is `allspark-<uid>` ([`socket_dir`]), the launcher pins
/// `allspark-daemon.sock`, and the pre-takeover Rust dir is
/// `prime-agent-rust-<uid>` (which this predicate does NOT match). So the
/// upstream TS dir is ALWAYS a foreign product's daemon — probing or stopping
/// it is exactly the cross-product kill the finding names.
///
/// Pure over the path, so the rule is unit-testable without touching the
/// ambient environment.
#[cfg(unix)]
#[must_use]
pub fn is_upstream_ts_daemon_socket(socket_path: &Path) -> bool {
    let Some(parent) = socket_path.parent() else {
        return false;
    };
    let Some(name) = parent.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    // `prime-agent-<digits>` exactly: the `-rust-<uid>` and bare forms never
    // match (a `-` must follow the prefix, then only digits).
    name.strip_prefix(UPSTREAM_TS_SOCKET_DIR_PREFIX)
        .and_then(|rest| rest.strip_prefix('-'))
        .is_some_and(|uid| !uid.is_empty() && uid.bytes().all(|byte| byte.is_ascii_digit()))
}

/// Windows daemons share one named-pipe namespace (the known, undecided
/// Windows model): there is no upstream TS dir to separate, so nothing is
/// refused by path here.
#[cfg(not(unix))]
#[must_use]
pub fn is_upstream_ts_daemon_socket(_socket_path: &Path) -> bool {
    false
}

/// Age of a file's mtime, for stale-lease detection.
#[must_use]
pub fn mtime_age(path: &Path) -> Option<Duration> {
    let modified = std::fs::metadata(path).and_then(|m| m.modified()).ok()?;
    SystemTime::now().duration_since(modified).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_path_hashes_socket() {
        let path = daemon_log_path(
            Path::new("/tmp/prime-agent-1/daemon.sock"),
            Path::new("/ad"),
        );
        assert!(path.starts_with("/ad/logs"));
        assert!(path
            .file_name()
            .unwrap()
            .to_string_lossy()
            .contains("daemon.sock."));
        let other = daemon_log_path(
            Path::new("/tmp/prime-agent-2/daemon.sock"),
            Path::new("/ad"),
        );
        assert_ne!(path, other);
    }

    /// The cross-product guard (known bug #22): the upstream TypeScript
    /// product's daemon socket is recognized by PATH — under
    /// `<tmpdir>/prime-agent-<uid>` — never by argv. This binary's own daemon
    /// never lives there (its default dir is `allspark-<uid>`, the launcher
    /// pins `allspark-daemon.sock`, and the pre-takeover dir is
    /// `prime-agent-rust-<uid>`), so the upstream dir is always a foreign
    /// product's daemon and the guard always fires. This is the rule that keeps
    /// a fork discovery/shutdown from ever probing the co-installed upstream
    /// daemon.
    #[cfg(unix)]
    #[test]
    fn the_upstream_ts_daemon_socket_is_recognized_by_path_only() {
        let tmp = Path::new("/tmp");
        assert!(is_upstream_ts_daemon_socket(
            &tmp.join("prime-agent-501/daemon.sock")
        ));
        // A worker socket in the upstream dir is the upstream daemon's too.
        assert!(is_upstream_ts_daemon_socket(
            &tmp.join("prime-agent-501/worker-abc-0123456789ab.sock")
        ));
        // The fork's own socket, the pre-takeover rust socket, and an
        // unrelated path are NOT the upstream product's.
        assert!(!is_upstream_ts_daemon_socket(
            &tmp.join("allspark-501/daemon.sock")
        ));
        assert!(!is_upstream_ts_daemon_socket(
            &tmp.join("prime-agent-rust-501/daemon.sock")
        ));
        assert!(!is_upstream_ts_daemon_socket(
            &tmp.join("other/daemon.sock")
        ));
        // A near-miss name (a non-numeric suffix, or no separator) never reads
        // as the upstream dir.
        assert!(!is_upstream_ts_daemon_socket(
            &tmp.join("prime-agent-socket/daemon.sock")
        ));
        assert!(!is_upstream_ts_daemon_socket(
            &tmp.join("prime-agent/daemon.sock")
        ));
        // A file directly under the dir (no parent) and a bare filename are
        // not the upstream dir either.
        assert!(!is_upstream_ts_daemon_socket(Path::new("/tmp")));
        assert!(!is_upstream_ts_daemon_socket(Path::new("daemon.sock")));
    }

    /// Review #40's rotation hole: the file created by the ROTATION reopen
    /// carried the default mode, not the restricted one.
    ///
    /// `append` restricted the file it opened, then renamed it away and opened a
    /// NEW file for the post-rotation line — and that second open had no
    /// `restrict_file`, so the live log came back world-readable (0644) for the
    /// rest of its life while its rotated predecessor stayed 0600. The leak is
    /// exactly the one the first fix was written for, reintroduced one rename
    /// later, so the mode must be set on the reopened handle, not only on the
    /// creation path.
    #[cfg(unix)]
    #[test]
    fn the_log_file_created_by_rotation_is_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::TempDir::new().expect("dir");
        let path = dir.path().join("daemon.log");
        // A ceiling under one line's length, so the first append rotates: it
        // creates the file, renames it to the generation, then reopens a fresh
        // file for the line it is holding.
        let log = RotatingLog {
            path: path.clone(),
            max_bytes: 16,
        };
        log.append("provider admission: settle_stream");
        assert!(
            path.with_extension("log.1").exists(),
            "the append rotated the log"
        );
        let mode = std::fs::metadata(&path)
            .expect("log file")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(
            mode, 0o600,
            "the post-rotation log file must be owner-read/write only, got {mode:o}"
        );
    }

    /// An existing directory keeps its mode; only a directory this call CREATES
    /// is restricted (review #9).
    ///
    /// The check-then-create shape asked `!parent.exists()` and then
    /// `create_dir_all`. Two daemons racing on the same fresh parent both saw
    /// "missing"; the loser's `create_dir_all` returned `AlreadyExists`, and its
    /// 0700 restriction then landed on a directory it did not own. The atomic
    /// claim is `create_dir`, which succeeds for exactly one caller.
    #[cfg(unix)]
    #[test]
    fn a_directory_this_call_did_not_create_keeps_its_mode() {
        use std::os::unix::fs::PermissionsExt;

        let root = tempfile::tempdir().expect("tempdir");
        let shared = root.path().join("shared");
        std::fs::create_dir(&shared).expect("create the shared dir");
        // A deliberately permissive mode, as the shared temp dir has.
        std::fs::set_permissions(&shared, std::fs::Permissions::from_mode(0o1777)).expect("chmod");
        let before = std::fs::metadata(&shared)
            .expect("stat")
            .permissions()
            .mode();

        ensure_dir(&shared).expect("an existing directory is not an error");

        let after = std::fs::metadata(&shared)
            .expect("stat")
            .permissions()
            .mode();
        assert_eq!(
            before, after,
            "ensure_dir must not re-mode a directory another owner created"
        );
    }

    /// And a directory it DOES create is owner-only.
    #[cfg(unix)]
    #[test]
    fn a_directory_this_call_created_is_restricted() {
        use std::os::unix::fs::PermissionsExt;

        let root = tempfile::tempdir().expect("tempdir");
        let fresh = root.path().join("fresh/nested");
        ensure_dir(&fresh).expect("create the chain");
        let mode = std::fs::metadata(&fresh)
            .expect("stat")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(
            mode, 0o700,
            "a created directory is owner-only, got {mode:o}"
        );
    }
}
