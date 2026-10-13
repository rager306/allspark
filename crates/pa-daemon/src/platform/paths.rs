//! Per-OS daemon endpoint naming: Unix socket files under
//! `<tmpdir>/allspark-<uid>/`; Windows named pipes in the `\\.\\pipe\\`
//! namespace (fixed daemon pipe, hashed worker pipes) - the TS product's
//! exact split.
use std::path::{Path, PathBuf};

use crate::paths::hash_key;

/// The fork's runtime-directory prefix under the temp dir. NOT the upstream
/// `prime-agent` name: a co-installed upstream daemon must never share this
/// directory, or the two daemons would fight over one socket.
const RUNTIME_DIR_PREFIX: &str = "allspark";


/// Default directory holding daemon socket files (Unix).
#[cfg(unix)]
pub fn socket_dir() -> PathBuf {
    let uid = pa_core::platform::process::current_user_id();
    let tmp = std::env::var_os("TMPDIR").map_or_else(|| PathBuf::from("/tmp"), PathBuf::from);
    tmp.join(format!("{RUNTIME_DIR_PREFIX}-{uid}"))
}

/// The socket-dir half of a discovery state root on Windows: TS computes
/// `<tmpdir>/allspark-user` so `DaemonStateRoot` keeps one shape.
#[cfg(not(unix))]
#[must_use]
pub fn socket_dir() -> PathBuf {
    std::env::temp_dir().join(format!("{RUNTIME_DIR_PREFIX}-user"))
}

/// Default supervisor endpoint: `daemon.sock` in the socket dir (Unix) or
/// the fixed daemon pipe name (Windows).
#[cfg(unix)]
#[must_use]
pub fn default_daemon_socket_path() -> PathBuf {
    socket_dir().join("daemon.sock")
}

#[cfg(not(unix))]
#[must_use]
pub fn default_daemon_socket_path() -> PathBuf {
    PathBuf::from(r"\\.\pipe\prime-agent-daemon")
}

/// Worker endpoint next to the supervisor's: hashed supervisor key plus the
/// worker id prefix (TS `workerSocketPath`).
#[cfg(unix)]
#[must_use]
pub fn worker_socket_path(supervisor_socket_path: &Path, worker_id: &str) -> PathBuf {
    let key = hash_key(&supervisor_socket_path.to_string_lossy(), 12);
    // Beside the supervisor's own socket, not in the shared default dir:
    // a supervisor pinned to a custom socket keeps its workers with it.
    let dir = supervisor_socket_path
        .parent()
        .map_or_else(socket_dir, Path::to_path_buf);
    dir.join(format!(
        "worker-{key}-{}.sock",
        &worker_id[..12.min(worker_id.len())]
    ))
}

#[cfg(not(unix))]
#[must_use]
pub fn worker_socket_path(supervisor_socket_path: &Path, worker_id: &str) -> PathBuf {
    let key = hash_key(&supervisor_socket_path.to_string_lossy(), 12);
    PathBuf::from(format!(
        r"\\.\pipe\prime-agent-worker-{key}-{}",
        &worker_id[..12.min(worker_id.len())]
    ))
}

// Socket-filesystem identity is the shared platform contract
// `pa_types::platform::socket_identity`: the same helper serves stale-file
// cleanup here and direct-transport ticket validation in the clients.

pub use pa_types::daemon::SocketIdentity;
pub use pa_types::platform::socket_identity;
#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn default_socket_uses_the_same_real_uid_as_the_typescript_daemon() {
        let output = std::process::Command::new("id")
            .arg("-ru")
            .output()
            .unwrap();
        assert!(output.status.success());
        let uid = String::from_utf8(output.stdout).unwrap();
        let tmp = std::env::var_os("TMPDIR").map_or_else(|| PathBuf::from("/tmp"), PathBuf::from);
        assert_eq!(
            socket_dir(),
            tmp.join(format!("{RUNTIME_DIR_PREFIX}-{}", uid.trim()))
        );
    }

    #[test]
    fn worker_socket_names_are_deterministic() {
        let supervisor = Path::new("/tmp/prime-agent-1/daemon.sock");
        let a = worker_socket_path(supervisor, "0123456789abcdef");
        let b = worker_socket_path(supervisor, "fedcba9876543210");
        assert_ne!(a, b);
        // Only the first 12 id characters key the name.
        assert_eq!(a, worker_socket_path(supervisor, "0123456789abffff"));
        // The worker socket sits BESIDE the supervisor's own socket, so a
        // supervisor pinned to a custom path keeps its workers with it.
        assert_eq!(a.parent(), supervisor.parent());
    }

    /// The reported regression: the fork's runtime dir must never collide
    /// with a co-installed upstream `prime-agent`.
    #[cfg(unix)]
    #[test]
    fn the_runtime_dir_is_not_the_upstream_dir() {
        let dir = socket_dir();
        assert_eq!(
            dir.file_name().and_then(|name| name.to_str()),
            Some(&*format!("{RUNTIME_DIR_PREFIX}-{}", {
                use std::fmt::Write;
                let uid = pa_core::platform::process::current_user_id();
                let mut out = String::new();
                let _ = write!(out, "{uid}");
                out
            }))
        );
        assert!(
            !dir.to_string_lossy().contains("prime-agent-"),
            "the fork's socket dir must not live in the upstream namespace: {}",
            dir.display()
        );
    }

    /// A worker socket under a custom supervisor socket stays in that
    /// custom directory, never the default one.
    #[cfg(unix)]
    #[test]
    fn a_custom_supervisor_keeps_its_workers_beside_it() {
        let custom = Path::new("/run/user/1000/custom/daemon.sock");
        let worker = worker_socket_path(custom, "0123456789abcdef");
        assert_eq!(worker.parent(), Some(Path::new("/run/user/1000/custom")));
    }

    /// The Windows endpoint names (TS win32 arms): the fixed daemon pipe
    /// name and the hashed worker pipe name in the `\\.\\pipe\\` namespace.
    #[test]
    #[cfg(windows)]
    fn windows_endpoints_are_the_ts_pipe_names() {
        assert_eq!(
            default_daemon_socket_path(),
            PathBuf::from(r"\\.\pipe\prime-agent-daemon")
        );
        let supervisor = Path::new(r"\\.\pipe\prime-agent-daemon");
        let a = worker_socket_path(supervisor, "0123456789abcdef");
        let rendered = a.to_string_lossy();
        assert!(
            rendered.starts_with(r"\\.\pipe\prime-agent-worker-"),
            "the worker pipe namespace: {rendered}"
        );
        assert!(
            rendered.ends_with("-0123456789ab"),
            "the 12-char id suffix: {rendered}"
        );
    }
}
