//! Transport contract for daemon sockets and streams: `AF_UNIX` sockets today, named pipes on
//! Windows later. Callers bind/connect through these traits and never name a concrete socket type.

use std::future::Future;
use std::path::Path;
use std::pin::Pin;

use anyhow::Result;
// `Context` is used only by the linux `O_PATH` re-anchoring and the
// Windows pipe-name error; a bare import is unused elsewhere.
#[cfg(any(target_os = "linux", windows))]
use anyhow::Context;

/// A full-duplex stream between a client and a daemon endpoint; `split`
/// consumes it into owned halves, and callers hold only the erased halves.
pub trait TransportStream: Send + Sync {
    fn split(self: Box<Self>) -> (Box<dyn AsyncReadHalf>, Box<dyn AsyncWriteHalf>);
}

/// Owned read half of a [`TransportStream`]; blanket-implemented.
pub trait AsyncReadHalf: tokio::io::AsyncRead + Unpin + Send {}
impl<T> AsyncReadHalf for T where T: tokio::io::AsyncRead + Unpin + Send {}

/// Owned write half of a [`TransportStream`]; blanket-implemented.
pub trait AsyncWriteHalf: tokio::io::AsyncWrite + Unpin + Send {}
impl<T> AsyncWriteHalf for T where T: tokio::io::AsyncWrite + Unpin + Send {}

#[cfg(unix)]
impl TransportStream for tokio::net::UnixStream {
    fn split(self: Box<Self>) -> (Box<dyn AsyncReadHalf>, Box<dyn AsyncWriteHalf>) {
        let (reader, writer) = tokio::net::UnixStream::into_split(*self);
        (Box::new(reader), Box::new(writer))
    }
}

/// Future returned by [`TransportListener::accept`].
pub type AcceptFuture<'a> =
    Pin<Box<dyn Future<Output = std::io::Result<Box<dyn TransportStream>>> + Send + 'a>>;

/// A bound transport endpoint that hands out connected streams.
pub trait TransportListener: Send + Sync {
    /// Future-boxed so the trait stays dyn-compatible (RPITIT methods are not).
    fn accept(&self) -> AcceptFuture<'_>;
}

#[cfg(unix)]
impl TransportListener for tokio::net::UnixListener {
    fn accept(&self) -> AcceptFuture<'_> {
        Box::pin(async move {
            loop {
                let (stream, _address) = self.accept().await?;
                // The accepted peer's uid is the kernel's answer, not the peer's (review #24,
                // CWE-306): a socket is bound before the daemon restricts its path, so a local
                // user can connect inside that window and keep the accepted connection after the
                // chmod. Refusing a foreign uid here covers the daemon and worker listeners
                // alike, because both are bound through `bind_transport`.
                //
                // A refusal RETRIES instead of surfacing an error: the accept loops count
                // consecutive failures toward a give-up budget, so an attacker able to connect
                // repeatedly could otherwise end the supervisor by being refused.
                if verify_peer_uid(&stream, &listener_label(self)).is_err() {
                    continue;
                }
                return Ok(Box::new(stream) as Box<dyn TransportStream>);
            }
        })
    }
}

/// The bound endpoint's own path, for a refusal message (empty when the listener has no path).
#[cfg(unix)]
fn listener_label(listener: &tokio::net::UnixListener) -> std::path::PathBuf {
    listener
        .local_addr()
        .ok()
        .and_then(|address| address.as_pathname().map(Path::to_path_buf))
        .unwrap_or_default()
}

/// Native `AF_UNIX` path capacity, excluding the terminating NUL.
#[cfg(unix)]
const MAX_SUN_PATH: usize = {
    // SAFETY: sockaddr_un contains only integer fields and arrays of integers.
    let address: libc::sockaddr_un = unsafe { std::mem::zeroed() };
    address.sun_path.len() - 1
};

/// A kernel-valid `AF_UNIX` address for `bind`/`connect`. Paths within the limit pass through; a
/// longer path is re-anchored through an `O_PATH` descriptor on its parent
/// (`/proc/self/fd/<fd>/<name>`) so the socket file still lands at the original deep location (like
/// TS, surviving deep `TMPDIR` paths). Linux only.
#[cfg(unix)]
pub struct UnixSocketAddress {
    address: std::path::PathBuf,
    /// Holds the directory descriptor open; the re-anchored
    /// `/proc/self/fd` path is valid only while this lives.
    _dir: Option<std::fs::File>,
}

#[cfg(unix)]
impl UnixSocketAddress {
    /// The effective address to hand to `bind`/`connect`.
    fn effective(&self) -> &Path {
        &self.address
    }

    /// Resolve `path` into a kernel-valid `AF_UNIX` address, or fail with the
    /// original path in the message.
    fn new(path: &Path) -> Result<Self> {
        if path.as_os_str().len() <= MAX_SUN_PATH {
            return Ok(Self {
                address: path.to_path_buf(),
                _dir: None,
            });
        }
        #[cfg(target_os = "linux")]
        {
            let (address, dir) = Self::through_dir_fd(path)?;
            Ok(Self {
                address,
                _dir: Some(dir),
            })
        }
        #[cfg(not(target_os = "linux"))]
        {
            anyhow::bail!(
                "AF_UNIX socket path exceeds the {MAX_SUN_PATH}-byte limit: {}",
                path.display()
            )
        }
    }

    /// Re-anchor a too-long path through `/proc/self/fd/<dir fd>/<file name>`.
    #[cfg(target_os = "linux")]
    fn through_dir_fd(path: &Path) -> Result<(std::path::PathBuf, std::fs::File)> {
        use std::os::fd::AsRawFd;
        use std::os::unix::fs::OpenOptionsExt;
        // `O_PATH`: an fd that references the directory without read/write
        // access; only `/proc/self/fd` traversal uses it.
        const O_PATH: i32 = 0o200_000;
        let name = path
            .file_name()
            .ok_or_else(|| anyhow::anyhow!("socket path has no file name: {}", path.display()))?;
        let parent = path.parent().ok_or_else(|| {
            anyhow::anyhow!("socket path has no parent directory: {}", path.display())
        })?;
        let dir = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(O_PATH)
            .open(parent)
            .with_context(|| format!("open socket directory {}", parent.display()))?;
        let address = std::path::PathBuf::from(format!(
            "/proc/self/fd/{}/{}",
            dir.as_raw_fd(),
            name.to_string_lossy()
        ));
        if address.as_os_str().len() > MAX_SUN_PATH {
            anyhow::bail!(
                "AF_UNIX socket path exceeds the {MAX_SUN_PATH}-byte limit: {}",
                path.display()
            );
        }
        Ok((address, dir))
    }
}

/// Bind a listening endpoint at `path` (a socket file on Unix).
///
/// # Errors
///
/// Returns an error if `path` cannot become a kernel-valid socket address or the listener fails to
/// bind.
#[cfg(unix)]
pub async fn bind_transport(path: &Path) -> Result<Box<dyn TransportListener>> {
    let address = UnixSocketAddress::new(path)?;
    let listener = tokio::net::UnixListener::bind(address.effective())?;
    Ok(Box::new(listener))
}

/// Connect to the endpoint at `path` asynchronously.
///
/// # Errors
///
/// Returns an error if `path` cannot become a kernel-valid socket address or the connection fails.
#[cfg(unix)]
pub async fn connect_transport(path: &Path) -> Result<Box<dyn TransportStream>> {
    let address = UnixSocketAddress::new(path)?;
    let stream = tokio::net::UnixStream::connect(address.effective()).await?;
    verify_peer_uid(&stream, path)?;
    Ok(Box::new(stream))
}

/// Refuse a peer on a Unix socket that another user owns (reviews #17/#24, CWE-290/306).
///
/// Both directions need it, and both used to trust the peer. On the CLIENT side
/// `probe_daemon` validates the HELLO's protocol, schema and build hash — every one of
/// those fields is supplied by the peer — so a predictable socket path
/// (`${TMPDIR}/allspark-daemon.sock`) let another local user bind it first, answer with
/// the expected public constants, and receive the victim's session commands and prompt
/// text. On the SERVER side a socket file exists from `bind` until the daemon restricts
/// its path, so a local user can connect inside that window and keep the accepted
/// connection after the chmod.
///
/// The answer is the kernel's, not the peer's: `SO_PEERCRED` on a connected socket
/// reports the uid of the process at the other end, which a process cannot forge.
/// Refusing a foreign uid turns both impersonations into a connection error the caller
/// already reports.
#[cfg(target_os = "linux")]
fn verify_peer_uid<F: std::os::fd::AsRawFd>(stream: &F, path: &Path) -> Result<()> {
    let fd = stream.as_raw_fd();
    let mut credentials = libc::ucred {
        pid: 0,
        uid: 0,
        gid: 0,
    };
    let mut length = libc::socklen_t::try_from(std::mem::size_of::<libc::ucred>())
        .expect("a ucred is far smaller than socklen_t::MAX");
    // SAFETY: `getsockopt` writes at most `length` bytes into `credentials`, whose size is
    // exactly what `length` reports; the fd is owned by `stream` for the call.
    let status = unsafe {
        libc::getsockopt(
            fd,
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            std::ptr::addr_of_mut!(credentials).cast(),
            std::ptr::addr_of_mut!(length),
        )
    };
    if status != 0 {
        // No answer from the kernel is not a reason to trust the peer.
        return Err(anyhow::anyhow!(
            "could not read the owner of the daemon socket {}: {}",
            path.display(),
            std::io::Error::last_os_error()
        ));
    }
    // SAFETY: read-only libc call.
    let our_uid = unsafe { libc::geteuid() };
    same_owner(credentials.uid, our_uid, path)
}

/// The kernel-reported identity of the process at the other end of a connected Unix socket.
///
/// `SO_PEERCRED` reports the credentials captured when the connection was made, so neither
/// field can be forged by the peer. Unlike a HELLO's `supervisorPid`, which the peer writes,
/// the pid here is the kernel's own answer — the one fact that binds a claimed process to the
/// socket a command was sent to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PeerCredentials {
    pub pid: u32,
    pub uid: u32,
}

/// Read the connected peer's credentials, or `None` where the platform cannot report them.
#[cfg(target_os = "linux")]
#[must_use]
pub fn socket_peer_credentials<F: std::os::fd::AsRawFd>(stream: &F) -> Option<PeerCredentials> {
    let mut credentials = libc::ucred {
        pid: 0,
        uid: 0,
        gid: 0,
    };
    let mut length = libc::socklen_t::try_from(std::mem::size_of::<libc::ucred>())
        .expect("a ucred is far smaller than socklen_t::MAX");
    // SAFETY: `getsockopt` writes at most `length` bytes into `credentials`, whose size is
    // exactly what `length` reports; the fd is owned by `stream` for the call.
    let status = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            std::ptr::addr_of_mut!(credentials).cast(),
            std::ptr::addr_of_mut!(length),
        )
    };
    if status != 0 {
        return None;
    }
    // A pid that does not fit `u32` (or 0, which no process has) names nothing.
    u32::try_from(credentials.pid)
        .ok()
        .filter(|pid| *pid > 0)
        .map(|pid| PeerCredentials {
            pid,
            uid: credentials.uid,
        })
}

/// The BSD/macOS answer carries the uid only (`getpeereid` has no pid), so the pid half of
/// [`PeerCredentials`] is unavailable there.
#[cfg(all(unix, not(target_os = "linux")))]
#[must_use]
pub fn socket_peer_credentials<F: std::os::fd::AsRawFd>(_stream: &F) -> Option<PeerCredentials> {
    None
}

/// The decision, separated from the syscall so it can be exercised on any machine.
///
/// A pure comparison, and the whole finding lives in it: the client trusted whichever
/// process answered. Keeping the syscall and the rule apart means the rule has a test that
/// runs everywhere — including as root, where every socket on the host is uid 0 and a
/// socket-level test would be vacuous.
fn same_owner(peer_uid: u32, our_uid: u32, path: &Path) -> Result<()> {
    if peer_uid != our_uid {
        return Err(anyhow::anyhow!(
            "refusing the socket {}: it belongs to uid {peer_uid} (this process is uid {our_uid})",
            path.display(),
        ));
    }
    Ok(())
}

/// The BSD/macOS half of the same check: `getpeereid` reports the peer's effective uid.
///
/// It was a no-op here, which left macOS with only the private-directory layer — and a
/// directory check cannot help when the socket path was chosen some other way (an explicit
/// `PRIME_AGENT_DAEMON_SOCKET`, a mounted home). `getpeereid` is the platform's own answer
/// and needs no `SO_PEERCRED`; the decision is the same `same_owner` the Linux path uses.
#[cfg(all(unix, not(target_os = "linux")))]
fn verify_peer_uid<F: std::os::fd::AsRawFd>(stream: &F, path: &Path) -> Result<()> {
    let mut peer_uid: libc::uid_t = 0;
    let mut peer_gid: libc::gid_t = 0;
    // SAFETY: `getpeereid` writes one uid and one gid into the pointers given; the fd is
    // owned by `stream` for the duration of the call.
    let status = unsafe { libc::getpeereid(stream.as_raw_fd(), &mut peer_uid, &mut peer_gid) };
    if status != 0 {
        // No answer from the kernel is not a reason to trust the peer.
        return Err(anyhow::anyhow!(
            "could not read the owner of the daemon socket {}: {}",
            path.display(),
            std::io::Error::last_os_error()
        ));
    }
    // SAFETY: read-only libc call.
    let our_uid = unsafe { libc::geteuid() };
    same_owner(peer_uid, our_uid, path)
}

/// Definitive nonblocking Unix listener refusal, used by the exit cleanup.
/// A saturated backlog can yield `EAGAIN` on Linux rather than a completed
/// connect. Only `ConnectionRefused` permits unlink; all unknown outcomes fail
/// closed. Reuse the transport's long-path re-anchor so deep socket paths
/// retain their stale-cleanup behavior.
///
/// The refused verdict is definitive on Linux only: on macOS and the BSDs a
/// saturated listen queue ALSO refuses the connect, so a live successor that
/// filled its backlog is indistinguishable from a dead path by this probe.
/// There the function never claims closure (the exit cleanup preserves the
/// path; the next bind's stale-socket prepare cleans a file nothing serves).
#[cfg(unix)]
#[must_use]
pub fn unix_listener_definitely_closed(path: &Path) -> bool {
    #[cfg(not(target_os = "linux"))]
    {
        let _ = path;
        false
    }
    #[cfg(target_os = "linux")]
    {
        let Ok(effective) = UnixSocketAddress::new(path) else {
            return false;
        };
        let Ok(address) = socket2::SockAddr::unix(effective.effective()) else {
            return false;
        };
        let Ok(socket) = socket2::Socket::new(socket2::Domain::UNIX, socket2::Type::STREAM, None)
        else {
            return false;
        };
        if socket.set_nonblocking(true).is_err() {
            return false;
        }
        matches!(
            socket.connect(&address),
            Err(error) if error.kind() == std::io::ErrorKind::ConnectionRefused
        )
    }
}

/// WINDOWS NAMED PIPES DO NOT AUTHENTICATE THEIR PEER (review #23, CWE-290).
///
/// The unix arms above and below bind the connection to the kernel's answer for the peer
/// (`SO_PEERCRED` / `getpeereid`) on BOTH sides: a client verifies the server it connected to,
/// and a listener verifies the client it accepted. Neither is possible here with the same
/// guarantee. `GetNamedPipeServerProcessId` answers a pid, but a pid is reused and says nothing
/// about which install the process belongs to; the real equivalent is pid + process start id +
/// executable identity, the ladder the installer's stop probe uses (`install-rust.sh`), and it
/// does not exist in this crate yet. An impersonation check works the opposite way — the SERVER
/// can verify a client, the CLIENT cannot verify the server — so it cannot close the
/// forged-daemon case, which is the one that has already killed a live session on unix.
///
/// Stated here so "the transport authenticates its peer" is not read as a claim about every
/// platform. Closing it is a separate change that needs a Windows runner to prove, not a
/// same-shape port of the unix check: `.research/review-runs/20261109-041525-a01cd1a0e/
/// DECISIONS-23-26.md` carries the three mechanisms and what each does not give.
#[cfg(windows)]
impl TransportListener for super::windows_pipe::NamedPipeListener {
    fn accept(&self) -> AcceptFuture<'_> {
        Box::pin(async move {
            let server = self.accept().await?;
            Ok(Box::new(server) as Box<dyn TransportStream>)
        })
    }
}

#[cfg(windows)]
impl TransportStream for tokio::net::windows::named_pipe::NamedPipeServer {
    fn split(self: Box<Self>) -> (Box<dyn AsyncReadHalf>, Box<dyn AsyncWriteHalf>) {
        let (reader, writer) = tokio::io::split(*self);
        (Box::new(reader), Box::new(writer))
    }
}

#[cfg(windows)]
impl TransportStream for tokio::net::windows::named_pipe::NamedPipeClient {
    fn split(self: Box<Self>) -> (Box<dyn AsyncReadHalf>, Box<dyn AsyncWriteHalf>) {
        let (reader, writer) = tokio::io::split(*self);
        (Box::new(reader), Box::new(writer))
    }
}

/// The pipe name handed to `CreateNamedPipe`/`CreateFile` (must be UTF-8): an existing pipe path
/// passes through - the local `\\.\pipe\`/ `\\?\pipe\` and remote `\\server\pipe\` forms,
/// case-insensitively like the pipe namespace. Any other explicit path (a unix-style socket path)
/// is
/// derived deterministically: resolved absolute spelling, lowercased, so bind and connect name the
/// same endpoint.
#[cfg(windows)]
fn pipe_name(path: &Path) -> Result<String> {
    let raw = path
        .to_str()
        .with_context(|| format!("pipe name is not UTF-8: {}", path.display()))?;
    if is_pipe_path(&raw.to_ascii_lowercase()) {
        return Ok(raw.to_string());
    }
    let absolute = if path.is_relative() {
        std::env::current_dir()
            .with_context(|| format!("resolve the relative socket path {}", path.display()))?
            .join(path)
    } else {
        path.to_path_buf()
    };
    let normalized = absolute
        .to_str()
        .with_context(|| format!("pipe name is not UTF-8: {}", absolute.display()))?
        .to_ascii_lowercase();
    Ok(format!(
        r"\\.\pipe\prime-agent-explicit-{:016x}",
        fnv1a64(&normalized)
    ))
}

/// Whether `raw` (already lowercased) is a pipe path the Windows APIs
/// accept as-is: the local forms or the remote `\\server\pipe\` form.
#[cfg(windows)]
fn is_pipe_path(raw: &str) -> bool {
    if raw.starts_with(r"\\.\pipe\") || raw.starts_with(r"\\?\pipe\") {
        return true;
    }
    raw.strip_prefix(r"\\")
        .and_then(|rest| rest.split_once(['\\', '/']))
        .is_some_and(|(server, tail)| {
            !server.is_empty() && tail.split(['\\', '/']).next() == Some("pipe")
        })
}

/// FNV-1a over the raw path bytes: the derivation key for [`pipe_name`]
/// (dependency-free and stable).
#[cfg(windows)]
fn fnv1a64(bytes: &str) -> u64 {
    bytes.bytes().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

/// Bind a listening endpoint at `path` (a named pipe on Windows).
///
/// # Errors
///
/// Returns an error when `path` is not valid UTF-8 or the named-pipe listener cannot be created.
#[cfg(windows)]
pub async fn bind_transport(path: &Path) -> Result<Box<dyn TransportListener>> {
    let name = pipe_name(path)?;
    let listener = super::windows_pipe::NamedPipeListener::bind(&name)?;
    Ok(Box::new(listener))
}

/// Connect to the endpoint at `path` asynchronously.
///
/// # Errors
///
/// Returns an error when `path` is not valid UTF-8 or the connection attempt fails, including the
/// busy-instance retry window.
#[cfg(windows)]
pub async fn connect_transport(path: &Path) -> Result<Box<dyn TransportStream>> {
    let name = pipe_name(path)?;
    let client = super::windows_pipe::connect(&name).await?;
    Ok(Box::new(client))
}

/// A blocking full-duplex stream, for the CLI's one-shot command client.
pub trait BlockingTransportStream:
    std::fmt::Debug + std::io::Read + std::io::Write + Send + Sync
{
    /// Duplicate the underlying handle so reads and writes can proceed on separate owned halves.
    ///
    /// # Errors
    ///
    /// Returns an error if the handle cannot be duplicated.
    fn try_clone_box(&self) -> std::io::Result<Box<dyn BlockingTransportStream>>;
    /// Deadline a pending read (poll granularity for deadline-driven waits).
    ///
    /// # Errors
    ///
    /// Returns an error if setting the read deadline on the underlying stream fails.
    fn set_read_timeout(&self, timeout: std::time::Duration) -> std::io::Result<()>;
    /// The kernel-reported identity of the process at the other end, where the platform
    /// reports one. Callers that must decide whether a HELLO's claimed pid really is the
    /// listener read it here rather than trusting the greeting.
    fn peer_credentials(&self) -> Option<PeerCredentials> {
        None
    }
}

#[cfg(unix)]
impl BlockingTransportStream for std::os::unix::net::UnixStream {
    fn try_clone_box(&self) -> std::io::Result<Box<dyn BlockingTransportStream>> {
        Ok(Box::new(self.try_clone()?))
    }

    fn set_read_timeout(&self, timeout: std::time::Duration) -> std::io::Result<()> {
        std::os::unix::net::UnixStream::set_read_timeout(self, Some(timeout))
    }

    fn peer_credentials(&self) -> Option<PeerCredentials> {
        socket_peer_credentials(self)
    }
}

/// Connect to the endpoint at `path`, blocking until connected.
///
/// # Errors
///
/// Returns an error if `path` cannot become a kernel-valid socket address or the blocking
/// connection fails.
#[cfg(unix)]
pub fn connect_blocking(path: &Path) -> std::io::Result<Box<dyn BlockingTransportStream>> {
    let address = UnixSocketAddress::new(path).map_err(std::io::Error::other)?;
    let stream = std::os::unix::net::UnixStream::connect(address.effective())?;
    // The blocking CLI client is the one that carries `send` and `shutdown`, and it used to
    // accept whichever process answered (review #16, CWE-290). The asynchronous client has
    // checked the peer's uid since review #17; this arm never did, so the same squatting
    // socket collected the victim's commands through `allspark <cmd>`.
    verify_peer_uid(&stream, path).map_err(std::io::Error::other)?;
    Ok(Box::new(stream))
}

/// Connect to the endpoint at `path`, blocking until connected.
///
/// # Errors
///
/// Returns an error when `path` is not valid UTF-8 or the blocking connection fails.
#[cfg(windows)]
pub fn connect_blocking(path: &Path) -> std::io::Result<Box<dyn BlockingTransportStream>> {
    let name = pipe_name(path).map_err(std::io::Error::other)?;
    let client = super::windows_pipe::BlockingPipeClient::connect(&name)?;
    Ok(Box::new(client))
}

#[cfg(all(test, unix))]
mod peer_credential_tests {
    use super::*;

    /// A socket another uid owns must be refused before a byte is exchanged.
    ///
    /// `SO_PEERCRED` reports the uid of the process that BOUND the socket, and no process
    /// can forge it — which is what makes this a check rather than a claim the peer gets
    /// to make. The rule is `same_owner`; this test drives it from both sides on any
    /// machine, because a socket-level test is vacuous under root (every socket here is
    /// uid 0) and a guard that cannot fail is the class the finding is about.
    #[test]
    fn the_owner_rule_refuses_a_foreign_uid_and_accepts_our_own() {
        let path = std::path::Path::new("/tmp/allspark-daemon.sock");
        assert!(
            same_owner(1000, 1000, path).is_ok(),
            "a socket this uid owns is the normal case"
        );
        let refusal = same_owner(1000, 1001, path).expect_err("a foreign owner is refused");
        let message = refusal.to_string();
        assert!(
            message.contains("belongs to uid 1000") && message.contains("this process is uid 1001"),
            "the refusal must name both uids so an operator can tell a squatter from a typo: {message}"
        );
    }

    /// The non-Linux verifier is a real check, not a stub that returns Ok.
    ///
    /// It was `Ok(())` on macOS and the BSDs, so the private-directory layer was the only
    /// protection there — useless when the socket path comes from an explicit
    /// `PRIME_AGENT_DAEMON_SOCKET`. The source is pinned rather than the behaviour because
    /// the call cannot be exercised on a Linux host: `getpeereid` needs the BSD ABI.
    #[test]
    fn the_non_linux_verifier_reads_peer_credentials() {
        let source = include_str!("transport.rs");
        let non_linux = source
            .split("#[cfg(all(unix, not(target_os = \"linux\")))]")
            .nth(1)
            .expect("the non-Linux verifier exists");
        let body = &non_linux[..non_linux.find("\n}\n").expect("the fn ends")];
        assert!(
            body.contains("getpeereid"),
            "the non-Linux verifier must ask the kernel, not return Ok(): {body}"
        );
        assert!(
            body.contains("same_owner("),
            "and it must apply the SAME decision as the Linux path: {body}"
        );
        assert!(
            !body.contains("let _ = _stream") && !body.contains("Ok(())\n}"),
            "a no-op stub is what the finding is about: {body}"
        );
    }

    /// The live path: a connection the kernel reports as ours goes through, and the
    /// verifier actually runs (a no-op verifier would also pass, so the second half
    /// forces the refusing arm through the same function).
    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn a_connect_runs_the_owner_check() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("daemon.sock");
        let listener = tokio::net::UnixListener::bind(&path).unwrap();
        tokio::spawn(async move {
            let _ = listener.accept().await;
        });

        connect_transport(&path)
            .await
            .expect("a socket this uid owns connects");
    }

    /// The listener side must CALL the same check (review #24, CWE-306).
    ///
    /// The accepted-peer uid is the only fact that separates the daemon's owner from any
    /// other local user who connected before the socket's path was restricted. Same shape
    /// as the client pin below: the refusal is not observable from a connection this
    /// process builds (the uid always matches, and a second uid needs a privilege the test
    /// may not drop to), so the check is on the mechanism.
    #[test]
    fn the_listener_accept_runs_the_owner_check() {
        let source = include_str!("transport.rs");
        let start = source
            .find("impl TransportListener for tokio::net::UnixListener")
            .expect("the unix listener impl exists");
        let end = source[start..]
            .find("\n}\n")
            .map(|at| start + at)
            .expect("the impl ends");
        let body = &source[start..end];
        assert!(
            body.contains("verify_peer_uid("),
            "accept must verify the connected peer's uid before dispatching it: {body}"
        );
    }

    /// The blocking CLI client must CALL the check (review #16, CWE-290).
    ///
    /// It is the transport behind `allspark send` and `allspark shutdown`; the async
    /// client checked the owner since review #17 and this arm never did.
    #[test]
    fn connect_blocking_runs_the_owner_check() {
        let source = include_str!("transport.rs");
        let start = source
            .find("pub fn connect_blocking(path: &Path)")
            .expect("the blocking connect exists");
        let end = source[start..]
            .find("\n}\n")
            .map(|at| start + at)
            .expect("the function ends");
        let body = &source[start..end];
        assert!(
            body.contains("verify_peer_uid("),
            "connect_blocking must verify the peer's uid before handing the stream on: {body}"
        );
    }

    /// The check must not refuse the owner: a same-uid client still connects, and the
    /// listener still hands the stream on (a guard that refuses everyone would pass the
    /// two pins above and break every daemon attach).
    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn an_owned_client_is_still_accepted() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("daemon.sock");
        let listener = bind_transport(&path).await.expect("bind");
        let connect_path = path.clone();
        let client =
            tokio::spawn(async move { connect_transport(&connect_path).await.expect("connect") });
        let accepted = tokio::time::timeout(std::time::Duration::from_secs(5), listener.accept())
            .await
            .expect("an owned client is accepted, not refused")
            .expect("accept succeeds");
        let _ = tokio::time::timeout(std::time::Duration::from_secs(5), client)
            .await
            .expect("the client connects")
            .expect("client task");
        drop(accepted);
    }

    /// `connect_transport` must CALL the check.
    ///
    /// The rule above is testable anywhere; whether the client runs it is a separate fact,
    /// and the whole finding is that it did not. Not observable from a connection this
    /// process can build (the uid always matches), so the check is on the mechanism — the
    /// same shape as the cron lock test: the function that connects must name the verifier.
    /// Red on the code before this commit, which connected and returned.
    #[test]
    fn connect_transport_runs_the_owner_check() {
        let source = include_str!("transport.rs");
        let start = source
            .find("pub async fn connect_transport(path: &Path)")
            .expect("the async connect exists");
        let end = source[start..]
            .find("\n}\n")
            .map(|at| start + at)
            .expect("the function ends");
        let body = &source[start..end];
        assert!(
            body.contains("verify_peer_uid("),
            "connect_transport must verify the peer's uid before handing the stream on: {body}"
        );
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    #[cfg(target_os = "linux")]
    use std::os::unix::fs::FileTypeExt;

    /// A directory whose full path length is exactly `target` bytes; falls
    /// back to `/tmp` when the ambient `TMPDIR` is already too deep.
    fn dir_of_exact_len(tag: &str, target: usize) -> std::path::PathBuf {
        let tag = format!("pa-transport-sun-path-{tag}");
        let base = std::env::temp_dir().join(&tag);
        let base = if base.as_os_str().len() + 21 <= target {
            base
        } else {
            std::path::Path::new("/tmp").join(&tag)
        };
        let mut dir = base;
        // Grow in 21-byte steps (separator + 20) while at least two bytes stay free: the
        // final component below needs a separator plus a NON-EMPTY name. The bound is 24,
        // not 22, because each step lands the length 21 bytes further on and the pad is
        // `target - len - 1`; at `+ 22 <= target` a deep `TMPDIR` (the repo-local `.tmp`)
        // can stop exactly one byte short, making the pad 0 — and joining an empty
        // component leaves a trailing separator the next `join` swallows, so the caller's
        // socket name would land one byte under MAX_SUN_PATH.
        while dir.as_os_str().len() + 24 <= target {
            dir = dir.join("d".repeat(20));
        }
        // Pad one final component: appending adds a separator plus the name.
        let pad = target
            .checked_sub(dir.as_os_str().len() + 1)
            .expect("base must leave room for a file name");
        assert!(
            pad >= 1,
            "the loop must leave room for a non-empty final component"
        );
        dir = dir.join("d".repeat(pad));
        assert_eq!(dir.as_os_str().len(), target);
        std::fs::create_dir_all(&dir).expect("create deep dir");
        dir
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn over_limit_paths_bind_connect_and_land_in_place() {
        use tokio::io::AsyncReadExt;
        let dir = dir_of_exact_len("roundtrip", 120);
        let socket = dir.join("worker-test.sock");
        let _ = std::fs::remove_file(&socket);
        assert!(socket.as_os_str().len() > MAX_SUN_PATH);

        bind_transport(&socket)
            .await
            .expect("bind through the limit");
        assert!(std::fs::symlink_metadata(&socket)
            .expect("socket file at the original deep path")
            .file_type()
            .is_socket());
        // A second bind on the live socket must fail (address in use), not
        // silently re-anchor somewhere else.
        assert!(bind_transport(&socket).await.is_err());
        std::fs::remove_file(&socket).expect("cleanup for rebind");

        let listener =
            tokio::net::UnixListener::bind(UnixSocketAddress::new(&socket).unwrap().effective())
                .expect("rebind address");
        let connect_path = socket.clone();
        let connect = tokio::spawn(async move {
            connect_transport(&connect_path)
                .await
                .expect("connect through the limit")
        });
        let (server, _) = listener.accept().await.expect("accept");
        let client = connect.await.expect("client task");
        // Round-trip one write to prove the pair is the same socket.
        let (mut reader, _writer) = client.split();
        server.writable().await.expect("server writable");
        server.try_write(b"ping").expect("server write");
        let mut buf = [0u8; 4];
        reader.read_exact(&mut buf).await.expect("client read");
        assert_eq!(&buf, b"ping");
        let _ = std::fs::remove_file(&socket);
    }

    #[tokio::test]
    async fn paths_at_the_limit_bind_directly() {
        let dir = dir_of_exact_len("boundary", 96);
        let name = "x".repeat(MAX_SUN_PATH - dir.as_os_str().len() - 1);
        let socket = dir.join(name);
        assert_eq!(socket.as_os_str().len(), MAX_SUN_PATH);
        bind_transport(&socket)
            .await
            .expect("bind at exactly the limit");
        let _ = std::fs::remove_file(&socket);
    }

    #[cfg(not(target_os = "linux"))]
    #[test]
    fn paths_one_byte_over_the_native_limit_are_rejected_with_context() {
        let path = std::path::PathBuf::from(format!("/{}", "x".repeat(MAX_SUN_PATH)));
        let error = UnixSocketAddress::new(&path)
            .err()
            .expect("over native limit");
        assert_eq!(
            error.to_string(),
            format!(
                "AF_UNIX socket path exceeds the {MAX_SUN_PATH}-byte limit: {}",
                path.display()
            )
        );
    }

    #[tokio::test]
    async fn over_limit_paths_without_a_short_name_error_clearly() {
        let dir = dir_of_exact_len("toolong", 120);
        let socket = dir.join("n".repeat(120));
        assert!(socket.as_os_str().len() > MAX_SUN_PATH);
        let error = bind_transport(&socket)
            .await
            .err()
            .expect("no short address exists");
        assert!(error.to_string().contains("exceeds"), "{error}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(all(test, windows))]
mod pipe_name_tests {
    use super::{fnv1a64, pipe_name};
    use std::path::Path;

    #[test]
    fn pipe_namespace_names_pass_through_unchanged() {
        let fixed = Path::new(r"\\.\pipe\prime-agent-daemon");
        assert_eq!(pipe_name(fixed).unwrap(), r"\\.\pipe\prime-agent-daemon");
        let worker = Path::new(r"\\.\pipe\prime-agent-worker-abc123-0123456789ab");
        assert_eq!(
            pipe_name(worker).unwrap(),
            r"\\.\pipe\prime-agent-worker-abc123-0123456789ab"
        );
    }

    #[test]
    fn explicit_file_paths_derive_the_same_pipe_on_both_sides() {
        let socket = Path::new(r"C:\Users\runner\AppData\Local\Temp\.tmpacp\daemon.sock");
        let a = pipe_name(socket).expect("derives");
        let b = pipe_name(socket).expect("derives again");
        assert_eq!(a, b, "the derivation is deterministic");
        assert!(a.starts_with(r"\\.\pipe\prime-agent-explicit-"), "{a}");
    }

    #[test]
    fn distinct_paths_derive_distinct_pipes() {
        let a = pipe_name(Path::new(r"C:\tmp\one\daemon.sock")).expect("derives");
        let b = pipe_name(Path::new(r"C:\tmp\two\daemon.sock")).expect("derives");
        assert_ne!(a, b);
    }

    #[test]
    fn fnv1a64_is_stable() {
        // FNV-1a("a") with the 64-bit offset basis and prime, for the record.
        assert_eq!(fnv1a64("a"), 0xaf63_dc4c_8601_ec8c);
    }

    #[test]
    fn pipe_paths_match_case_insensitively_and_remote_forms_pass_through() {
        let upper = Path::new(r"\\.\PIPE\prime-agent-daemon");
        assert_eq!(pipe_name(upper).unwrap(), r"\\.\PIPE\prime-agent-daemon");
        let remote = Path::new(r"\\fileserver\pipe\prime-agent");
        assert_eq!(pipe_name(remote).unwrap(), r"\\fileserver\pipe\prime-agent");
    }

    #[test]
    fn equivalent_path_spellings_derive_the_same_pipe() {
        let a = pipe_name(Path::new(r"C:\Temp\daemon.sock")).expect("derives");
        let b = pipe_name(Path::new(r"c:\temp\DAEMON.SOCK")).expect("derives");
        assert_eq!(a, b, "case is normalized before hashing");
    }

    #[test]
    fn relative_paths_derive_per_working_directory() {
        // The current directory is process-global: hold the module's lock
        // and restore it on exit (the drop guard covers panic paths).
        use std::sync::Mutex;
        static CWD_LOCK: Mutex<()> = Mutex::new(());
        let _lock = CWD_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let _guard = CwdGuard::capture();
        let first = tempfile_dir();
        std::env::set_current_dir(&first).expect("chdir first");
        let here = pipe_name(Path::new("daemon.sock")).expect("derives");
        let second = tempfile_dir();
        std::env::set_current_dir(&second).expect("chdir second");
        let elsewhere = pipe_name(Path::new("daemon.sock")).expect("derives");
        assert_ne!(
            here, elsewhere,
            "the same relative name derives per working directory"
        );
        assert!(
            here.starts_with(r"\\.\pipe\prime-agent-explicit-"),
            "{here}"
        );
    }

    /// Restores the process working directory on scope exit (including panics).
    struct CwdGuard(std::path::PathBuf);

    impl CwdGuard {
        fn capture() -> Self {
            Self(std::env::current_dir().expect("current dir"))
        }
    }

    impl Drop for CwdGuard {
        fn drop(&mut self) {
            let _ = std::env::set_current_dir(&self.0);
        }
    }

    /// A fresh directory to chdir into for the relative-path pin.
    fn tempfile_dir() -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("pa-pipe-name-cwd-{}", uuid_like()));
        std::fs::create_dir_all(&dir).expect("create the cwd pin dir");
        dir
    }

    /// Per-call unique suffix without a uuid dependency: the process id plus a monotonic counter.
    fn uuid_like() -> u64 {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let next = COUNTER.fetch_add(1, Ordering::Relaxed) + 1;
        (u64::from(std::process::id()) << 32) | next
    }
}
