//! Named-pipe transport for Windows endpoints (`\\.\pipe\` namespace): byte-mode duplex instances,
//! local-only, clients waiting while every instance is busy (Node's `net` on Windows); implements
//! the shared [`TransportListener`] / [`TransportStream`] / [`BlockingTransportStream`] contracts.
//! Endpoint naming lives in `pa-daemon::platform`.
//!
//! # Peer authentication: what is checked, and what is not (review #23, CWE-290)
//!
//! The Unix transports ask the kernel who is at the other end (`SO_PEERCRED`, or `getpeereid` +
//! `LOCAL_PEERPID` on macOS; see [`super::transport`]). Here the client can now ask too: the
//! blocking CLI client reports the SERVER's pid from `GetNamedPipeServerProcessId`
//! ([`BlockingPipeClient`]'s `peer_credentials`), and the HELLO gate
//! (`pa-cli::daemon_discovery::verify_hello_supervisor_pid`) only believes a greeting that names
//! that process. A squatter on the pipe name is then named as itself and its greeting supplies no
//! kill target.
//!
//! The SERVER side is owner-only. Every instance is created through
//! [`create_owner_only_instance`], which passes a `SECURITY_ATTRIBUTES` carrying the descriptor
//! `D:P(A;;GA;;;OW)` (see [`OWNER_ONLY_SDDL`]): a protected DACL granting full access to the
//! pipe's owner and to nobody else. The kernel then refuses another local account's
//! `CreateFile` — the Windows analogue of the Unix listener dropping a foreign-uid accept, and
//! the reason "any process that opens the pipe is served" no longer holds. This is the same
//! mechanism the review's suggested fix names, and it removes the impersonation check from the
//! critical path rather than adding a second, weaker one.
//!
//! Two limits remain, stated rather than papered over:
//!
//! - Another local account can still create the NAME before the daemon starts (its own instance
//!   carries its own descriptor). `first_pipe_instance` turns that into a bind FAILURE — the
//!   daemon does not serve a pipe it did not create — and a client that does connect elsewhere
//!   is caught by the pid gate above: the squatter is named as itself, and its greeting supplies
//!   no kill target. Nothing here can prevent the squat; it can only refuse to be the squatted.
//! - We do not `ImpersonateNamedPipeClient` on accept, so a served peer is not NAMED (no client
//!   uid/pid is reported anywhere); ownership is enforced at connect time by the DACL instead.
//!   A token comparison would add a second layer, and it needs Win32 calls this fork cannot
//!   exercise — shipping one blind is the defect class this review exists to catch.
//!
//! NOT EXERCISED ON WINDOWS by this fork's CI: the tree has no Windows runner on this host, so
//! the cross target is compile-checked (`cargo check --target x86_64-pc-windows-gnu`, and the
//! `make windows-cross` gate) and reviewed, not executed. The `#[cfg(all(test, windows))]`
//! round-trip below is the verifier a real Windows runner (the on-demand
//! `windows-runtime-triage` battery's `cargo test -p pa-types`) executes against this path: a
//! descriptor the daemon itself cannot open would fail it. The options and their costs are in
//! `.research/review-runs/20261109-041525-a01cd1a0e/DECISIONS-23-26.md`.

use std::io;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::windows::named_pipe::{
    ClientOptions, NamedPipeClient, NamedPipeServer, ServerOptions,
};
use tokio::runtime::Runtime;
use tokio::sync::Mutex as AsyncMutex;

use super::transport::{BlockingTransportStream, PeerCredentials};

/// `winerror.h` `ERROR_PIPE_BUSY`: the pipe name exists but no instance is
/// listening (pinned constant, no windows-sys dependency).
const ERROR_PIPE_BUSY: i32 = 231;
/// Busy-pipe poll interval; Node's clients wait the same way.
const BUSY_RETRY_INTERVAL: Duration = Duration::from_millis(50);
/// Overall cap on busy-pipe waiting, so a wedged server surfaces as an
/// error instead of hanging the caller.
const BUSY_RETRY_DEADLINE: Duration = Duration::from_secs(10);

/// The DACL every pipe instance is created with, as SDDL (review #23, CWE-290).
///
/// `D:P` is a PROTECTED DACL: no inherited entry can widen it, so the pipe's effective access is
/// exactly what is written here regardless of the directory the process runs in. The two ACEs
/// grant full access to Local System (`SY`, the Windows analogue of uid 0 on a Unix socket) and
/// to the object's OWNER (`OW`, the SID the kernel resolves to whoever created the pipe) — and to
/// nobody else.
const OWNER_ONLY_SDDL: &str = "D:P(A;;GA;;;SY)(A;;GA;;;OW)";

/// The Win32 security surface, as a hand-declared extern wall (repo policy: pinned constants and
/// externs, no windows-sys dependency — the same wall `platform::process` and `windows_console`
/// build).
mod winapi {
    // The Win32 header names stay verbatim: the pinned-constants wall mirrors them.
    #![allow(non_snake_case)]

    use std::ffi::c_void;

    /// `sddl.h` `SDDL_REVISION_1`: the only revision `ConvertStringSecurityDescriptor…` accepts.
    pub(crate) const SDDL_REVISION_1: u32 = 1;

    // `#[link]` on each block, not a bare `extern`: a hand-declared import without its library
    // fails at LINK time on Windows (which no `cargo check` on this host reaches) rather than at
    // compile time.
    #[link(name = "advapi32")]
    extern "system" {
        /// `sddl.h`: parse SDDL into a self-relative `SECURITY_DESCRIPTOR` allocated with
        /// `LocalAlloc` (the caller frees it with `LocalFree`).
        pub(crate) fn ConvertStringSecurityDescriptorToSecurityDescriptorW(
            string_security_descriptor: *const u16,
            revision: u32,
            security_descriptor: *mut *mut c_void,
            size: *mut u32,
        ) -> i32;
    }

    #[link(name = "kernel32")]
    extern "system" {
        pub(crate) fn LocalFree(memory: *mut c_void) -> *mut c_void;
    }
}

/// `SECURITY_ATTRIBUTES` (`winbase.h`): the descriptor `CreateNamedPipeW` copies into the new
/// kernel object. `nLength` must be this exact size — the kernel validates it.
#[repr(C)]
struct SecurityAttributes {
    length: u32,
    descriptor: *mut std::ffi::c_void,
    inherit_handle: i32,
}

/// One instance's owner-only security: the parsed [`OWNER_ONLY_SDDL`] descriptor plus the
/// `SECURITY_ATTRIBUTES` that points at it. Dropped right after the instance is created;
/// `CreateNamedPipeW` copies the descriptor into the object, so nothing points at this afterwards.
struct OwnerOnlySecurity {
    attributes: SecurityAttributes,
}

impl OwnerOnlySecurity {
    fn new() -> io::Result<Self> {
        let sddl: Vec<u16> = OWNER_ONLY_SDDL
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        let mut descriptor: *mut std::ffi::c_void = std::ptr::null_mut();
        let mut size: u32 = 0;
        // SAFETY: `sddl` is NUL-terminated and outlives the call; the callee writes one pointer
        // and one length through valid pointers.
        let parsed = unsafe {
            winapi::ConvertStringSecurityDescriptorToSecurityDescriptorW(
                sddl.as_ptr(),
                winapi::SDDL_REVISION_1,
                std::ptr::from_mut(&mut descriptor),
                std::ptr::from_mut(&mut size),
            )
        };
        if parsed == 0 || descriptor.is_null() {
            // A descriptor this process cannot build is a pipe it must not create: without the
            // DACL the instance would fall back to the default (world-accessible) descriptor.
            return Err(io::Error::last_os_error());
        }
        Ok(Self {
            attributes: SecurityAttributes {
                length: u32::try_from(std::mem::size_of::<SecurityAttributes>())
                    .expect("SECURITY_ATTRIBUTES is far smaller than u32::MAX"),
                descriptor,
                inherit_handle: 0,
            },
        })
    }

    /// The `SECURITY_ATTRIBUTES` pointer `CreateNamedPipeW` takes, borrowed for the call.
    fn as_ptr(&mut self) -> *mut std::ffi::c_void {
        std::ptr::from_mut(&mut self.attributes).cast()
    }
}

impl Drop for OwnerOnlySecurity {
    fn drop(&mut self) {
        // SAFETY: the pointer came from `ConvertStringSecurityDescriptor…`, which allocates with
        // `LocalAlloc`; it is freed exactly once, here.
        unsafe {
            winapi::LocalFree(self.attributes.descriptor);
        }
    }
}

/// Create ONE pipe instance whose DACL is [`OWNER_ONLY_SDDL`].
///
/// Every instance in this module is created through here, both halves included: a listener's
/// first instance and its replacement, which is why the restriction cannot be dropped by a later
/// instance (the kernel applies the DACL of the instance a client connects to).
///
/// A descriptor failure FAILS the creation rather than falling back to the default descriptor: a
/// pipe that cannot be made owner-only must not be served.
fn create_owner_only(options: &ServerOptions, name: &str) -> io::Result<NamedPipeServer> {
    let mut security = OwnerOnlySecurity::new()?;
    // SAFETY: `security` holds a valid, self-relative SECURITY_DESCRIPTOR from
    // `ConvertStringSecurityDescriptorToSecurityDescriptorW` and lives across the call;
    // `CreateNamedPipeW` copies the descriptor into the new kernel object, so the pointer itself
    // need not outlive it.
    unsafe { options.create_with_security_attributes_raw(name, security.as_ptr()) }
}

/// A listening named-pipe endpoint. One instance waits for the next client at all times; `accept`
/// connects that instance and has already created its replacement, so the pipe name stays
/// connectable - the named-pipe equivalent of a backlog.
pub(crate) struct NamedPipeListener {
    name: String,
    /// The instance currently listening for the next client.
    pending: AsyncMutex<Option<NamedPipeServer>>,
}

impl NamedPipeListener {
    /// Create the first pipe instance at `name`. `first_pipe_instance`
    /// fails when another process owns the name (a live Unix socket blocking
    /// `bind`); a stale pipe cannot exist - instances die with their process.
    pub(crate) fn bind(name: &str) -> io::Result<Self> {
        let first = create_owner_only(ServerOptions::new().first_pipe_instance(true), name)?;
        Ok(Self {
            name: name.to_string(),
            pending: AsyncMutex::new(Some(first)),
        })
    }

    /// Hand the next client a connected server instance. The next instance is created before
    /// waiting, so a creation failure surfaces first; two instances listen while this call waits,
    /// keeping clients off the busy path.
    pub(crate) async fn accept(&self) -> io::Result<NamedPipeServer> {
        let mut pending = self.pending.lock().await;
        let next = create_owner_only(&ServerOptions::new(), &self.name)?;
        let server = pending
            .replace(next)
            .expect("a listening instance is always kept ready");
        server.connect().await?;
        Ok(server)
    }
}

/// Open a client connection; a busy pipe (`ERROR_PIPE_BUSY`) is retried until the server frees one
/// (Node's clients wait the same way through libuv's connect machinery).
pub(crate) async fn connect(name: &str) -> io::Result<NamedPipeClient> {
    let deadline = tokio::time::Instant::now() + BUSY_RETRY_DEADLINE;
    loop {
        match ClientOptions::new().open(name) {
            Ok(client) => return Ok(client),
            Err(error) if error.raw_os_error() == Some(ERROR_PIPE_BUSY) => {}
            Err(error) => return Err(error),
        }
        if tokio::time::Instant::now() + BUSY_RETRY_INTERVAL > deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                format!("named pipe {name} stayed busy for more than {BUSY_RETRY_DEADLINE:?}"),
            ));
        }
        tokio::time::sleep(BUSY_RETRY_INTERVAL).await;
    }
}

/// State shared by the read and write halves of a blocking client: one pipe handle driven by a
/// dedicated single-threaded runtime, because std exposes no read deadline for named pipes; the
/// runtime gives `set_read_timeout` a real bounded wait.
struct BlockingPipeState {
    runtime: Runtime,
    client: AsyncMutex<NamedPipeClient>,
    /// Read deadline for pending reads; `None` blocks indefinitely.
    read_timeout: Mutex<Option<Duration>>,
}

/// A blocking named-pipe client for one-shot command surfaces (the CLI
/// daemon client). Cloning shares the underlying connection.
pub(crate) struct BlockingPipeClient {
    name: String,
    state: Arc<BlockingPipeState>,
}

impl std::fmt::Debug for BlockingPipeClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BlockingPipeClient")
            .field("name", &self.name)
            .finish_non_exhaustive()
    }
}

impl BlockingPipeClient {
    /// Connect to the pipe at `name`, blocking until connected.
    pub(crate) fn connect(name: &str) -> io::Result<Self> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        let client = runtime.block_on(connect(name))?;
        Ok(Self {
            name: name.to_string(),
            state: Arc::new(BlockingPipeState {
                runtime,
                client: AsyncMutex::new(client),
                read_timeout: Mutex::new(None),
            }),
        })
    }

    /// Duplicate this handle so reads and writes can proceed on separate
    /// owned halves of the same connection.
    pub(crate) fn try_clone(&self) -> Self {
        Self {
            name: self.name.clone(),
            state: Arc::clone(&self.state),
        }
    }

    /// One blocking read, bounded by the configured deadline when present.
    fn read_bounded(&self, buf: &mut [u8]) -> io::Result<usize> {
        let deadline = self
            .state
            .read_timeout
            .lock()
            .map_err(|_| io::Error::other("read deadline lock poisoned"))?;
        let read = async { self.state.client.lock().await.read(buf).await };
        match *deadline {
            Some(timeout) => match self
                .state
                .runtime
                // Construct the timer only after entering this client's runtime.
                .block_on(async { tokio::time::timeout(timeout, read).await })
            {
                Ok(result) => result,
                // Cancelled reads leave the buffer untouched; callers treat
                // the deadline as an empty poll, like Unix `SO_RCVTIMEO`.
                Err(_elapsed) => Err(io::ErrorKind::TimedOut.into()),
            },
            None => self.state.runtime.block_on(read),
        }
    }
}

impl std::io::Read for BlockingPipeClient {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.read_bounded(buf)
    }
}

impl std::io::Write for BlockingPipeClient {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.state
            .runtime
            .block_on(async { self.state.client.lock().await.write(buf).await })
    }

    fn flush(&mut self) -> io::Result<()> {
        self.state
            .runtime
            .block_on(async { self.state.client.lock().await.flush().await })
    }
}

impl BlockingTransportStream for BlockingPipeClient {
    fn try_clone_box(&self) -> io::Result<Box<dyn BlockingTransportStream>> {
        Ok(Box::new(self.try_clone()))
    }

    /// The SERVER process behind this connection, as the kernel names it
    /// (`GetNamedPipeServerProcessId`). Windows has no `SO_PEERCRED` and no uid to report here,
    /// so `uid` stays 0 (unspecified) and the pid is the whole answer.
    ///
    /// This is what lets the HELLO gate stay uniform: a discovery that cannot learn the listener
    /// pid refuses the greeting's own claim (#5), which on Windows meant every daemon lost its
    /// pid. The kernel pid is not forgeable by the peer — a squatter that owns the pipe name is
    /// the process the kernel names, so its greeting can no longer point at the victim daemon.
    ///
    /// NOT EXERCISED ON WINDOWS by this fork's CI: the tree has no Windows runner, so this arm
    /// is compile-checked (`cargo check --target x86_64-pc-windows-gnu`) and reviewed, not
    /// executed. The decision it feeds is host-tested in `pa-cli`
    /// (`a_hello_pid_without_a_kernel_listener_pid_is_refused`).
    fn peer_credentials(&self) -> Option<PeerCredentials> {
        let pid = self
            .state
            .runtime
            .block_on(async { server_process_id(&*self.state.client.lock().await) });
        pid.map(|pid| PeerCredentials { pid, uid: 0 })
    }

    fn set_read_timeout(&self, timeout: Duration) -> io::Result<()> {
        *self
            .state
            .read_timeout
            .lock()
            .map_err(|_| io::Error::other("read deadline lock poisoned"))? = Some(timeout);
        Ok(())
    }
}

/// The pid of the process that owns the server end of `client`'s pipe
/// (`GetNamedPipeServerProcessId`, `PULONG` out), or `None` when the query fails.
fn server_process_id(client: &NamedPipeClient) -> Option<u32> {
    use std::os::windows::io::AsRawHandle as _;

    // The kernel32 surface the query needs, as a hand-declared extern (repo policy: pinned
    // constants and externs, no windows-sys dependency - the same wall `platform::process`
    // builds for its identity queries).
    #[link(name = "kernel32")]
    extern "system" {
        fn GetNamedPipeServerProcessId(pipe: *mut std::ffi::c_void, pid: *mut u32) -> i32;
    }

    let mut pid: u32 = 0;
    // SAFETY: `client`'s handle outlives the call (borrowed for it), and the callee writes one
    // `u32` through a valid pointer.
    let ok = unsafe {
        GetNamedPipeServerProcessId(client.as_raw_handle(), std::ptr::from_mut(&mut pid))
    };
    (ok != 0).then_some(pid).filter(|pid| *pid > 0)
}

#[cfg(all(test, windows))]
mod tests {
    //! Round-trips through the public transport API. Windows-only; the sandbox verifies compilation
    //! for the cross target, a real Windows runner executes them.

    use std::io::{Read, Write};
    use std::path::PathBuf;
    use std::time::Duration;

    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    use super::super::transport::{bind_transport, connect_blocking, connect_transport};

    /// Unique pipe name per test process (parallel binaries must not
    /// collide in the global `\.\pipe\` namespace).
    fn test_pipe(tag: &str) -> PathBuf {
        PathBuf::from(format!(
            r"\\.\pipe\prime-agent-pa-types-test-{}-{tag}",
            std::process::id()
        ))
    }

    #[tokio::test]
    async fn pipe_roundtrip_through_public_api() {
        let path = test_pipe("async-roundtrip");
        let listener = bind_transport(&path).await.expect("bind");
        let connect_path = path.clone();
        let client =
            tokio::spawn(async move { connect_transport(&connect_path).await.expect("connect") });
        let server = listener.accept().await.expect("accept");
        let client = client.await.expect("client task");
        let (mut client_read, mut client_write) = client.split();
        let (mut server_read, mut server_write) = server.split();
        server_write.write_all(b"ping").await.expect("server write");
        let mut buf = [0u8; 4];
        client_read.read_exact(&mut buf).await.expect("client read");
        assert_eq!(&buf, b"ping");
        client_write.write_all(b"pong").await.expect("client write");
        let mut buf = [0u8; 4];
        server_read.read_exact(&mut buf).await.expect("server read");
        assert_eq!(&buf, b"pong");
    }

    #[tokio::test]
    async fn second_bind_on_a_live_pipe_name_fails() {
        let path = test_pipe("bind-conflict");
        let _listener = bind_transport(&path).await.expect("first bind");
        let error = bind_transport(&path)
            .await
            .err()
            .expect("a live pipe name must not be rebindable");
        assert!(
            error.to_string().to_lowercase().contains("denied"),
            "first-instance conflict, got: {error}"
        );
    }

    #[test]
    fn blocking_client_deadline_then_roundtrip() {
        let path = test_pipe("blocking-roundtrip");
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        let server_path = path.clone();

        let server = std::thread::spawn(move || {
            let runtime = tokio::runtime::Runtime::new().expect("server runtime");
            runtime.block_on(async move {
                let listener = bind_transport(&server_path).await.expect("bind");
                ready_tx.send(()).expect("listener ready");
                let server = listener.accept().await.expect("accept");
                let (mut reader, mut writer) = server.split();
                reply_rx.await.expect("release reply");
                writer.write_all(b"hello").await.expect("server write");
                let mut buf = [0u8; 4];
                reader.read_exact(&mut buf).await.expect("server read");
                assert_eq!(&buf, b"echo");
            });
        });

        ready_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("listener ready");
        let mut stream = connect_blocking(&path).expect("blocking connect");
        stream
            .set_read_timeout(Duration::from_millis(50))
            .expect("deadline");
        let mut buf = [0u8; 5];
        assert_eq!(
            stream
                .read(&mut buf)
                .expect_err("reply is held back")
                .kind(),
            std::io::ErrorKind::TimedOut
        );
        reply_tx.send(()).expect("release reply");
        stream
            .set_read_timeout(Duration::from_secs(5))
            .expect("roundtrip deadline");
        stream.read_exact(&mut buf).expect("client read");
        assert_eq!(&buf, b"hello");
        stream.write_all(b"echo").expect("client write");
        stream.flush().expect("client flush");
        server.join().expect("server thread");
    }

    #[test]
    fn blocking_connect_to_missing_pipe_reports_not_found() {
        let path = test_pipe("missing");
        let error = connect_blocking(&path).expect_err("no listener owns this pipe name");
        // The CLI maps NotFound to Node's `connect ENOENT <path>` error text.
        assert_eq!(error.kind(), std::io::ErrorKind::NotFound);
    }
}
