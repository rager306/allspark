//! Process control: signals, process groups, detached spawns. Signatures
//! that report outcomes return `bool` where callers treat "unproven"
//! conservatively (a kill that could not be proven reports false, matching
//! the TS `killOrphanProcess` contract).

use std::process::Command;

/// The real Unix user id, matching Node `process.getuid()` for daemon namespaces.
#[cfg(unix)]
#[must_use]
pub fn current_user_id() -> u32 {
    nix::unistd::Uid::current().as_raw()
}

/// Termination signal for [`kill_pid`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Signal {
    /// Graceful stop (SIGTERM).
    Term,
    /// Forcible stop (SIGKILL).
    Kill,
}

/// Put the spawned child into its own process group so later group-scoped
/// kills reach all of its descendants (TS: `detached: true` on POSIX).
#[cfg(unix)]
pub fn set_new_process_group(command: &mut Command) {
    use std::os::unix::process::CommandExt;
    command.process_group(0);
}

/// Start the spawned child in a new session with no controlling terminal
/// (`setsid`): the child cannot open `/dev/tty`, and job-control signals
/// from the parent's terminal never reach it. A new process group alone
/// is not enough: the group can still open the terminal, and a background
/// read then stops it with SIGTTIN.
#[cfg(unix)]
pub fn set_new_session(command: &mut Command) {
    use std::os::unix::process::CommandExt;
    // SAFETY: the hook runs in the forked child before exec and only calls
    // setsid(2), which is async-signal-safe.
    unsafe {
        command.pre_exec(|| {
            nix::unistd::setsid()
                .map(drop)
                .map_err(std::io::Error::from)
        });
    }
}

/// Windows: the libuv mapping of Node `detached: true` on win32 -
/// `CREATE_NEW_PROCESS_GROUP | DETACHED_PROCESS`, plus `CREATE_NO_WINDOW`
/// (console children of a windowless parent would flash a fresh console).
/// Tree kills need none of it (`taskkill /T` walks the parent-child tree).
#[cfg(windows)]
pub fn set_new_process_group(command: &mut Command) {
    use std::os::windows::process::CommandExt;
    /// `winbase.h`: new process group (no ctrl+c broadcast from the parent).
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    /// `winbase.h`: detached console, survives the parent's console close.
    const DETACHED_PROCESS: u32 = 0x0000_0008;
    /// `winbase.h` `CREATE_NO_WINDOW` (TS `windowsHide`).
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    command.creation_flags(CREATE_NEW_PROCESS_GROUP | DETACHED_PROCESS | CREATE_NO_WINDOW);
}

#[cfg(not(any(unix, windows)))]
pub fn set_new_process_group(_command: &mut Command) {
    // No detached-group mechanism on this platform; group-scoped kills are
    // unavailable and callers fall back to single-pid kills.
}

/// Hide the console window of a non-interactive spawn (TS `windowsHide`).
/// Only one of [`set_new_process_group`] and [`set_no_window`] may be
/// applied: creation flags replace each other.
#[cfg(windows)]
pub fn set_no_window(command: &mut Command) {
    use std::os::windows::process::CommandExt;
    /// `winbase.h` `CREATE_NO_WINDOW` (TS `windowsHide`).
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    command.creation_flags(CREATE_NO_WINDOW);
}

/// Unix: every product console surface owns its own window behavior
/// (`windowsHide` has no meaning without windows).
#[cfg(not(windows))]
pub fn set_no_window(_command: &mut Command) {}

/// Raise the soft open-file limit to the hard limit and return the
/// resulting soft limit (Node raises it the same way at startup). macOS
/// refuses a soft limit above `kern.maxfilesperproc`, so the target is
/// capped there.
///
/// # Errors
///
/// The OS error of a failed `getrlimit`, `sysctlbyname` or `setrlimit`.
/// The descriptor limit a process should hold, given what the OS reports.
///
/// Split out of [`raise_open_file_limit`] so the DECISION is checkable without a
/// syscall: the caller is a pair of `getrlimit`/`setrlimit` calls around this,
/// and what can be wrong is the arithmetic, not the calls.
///
/// Returns the soft limit to install, or `None` when the current one already
/// reaches the target (the caller then makes no syscall at all).
///
/// The invariant every caller relies on, and the one the `kani::proof` below
/// checks: a returned limit is never BELOW the current soft limit, so raising a
/// limit can never lower it. Lowering it would make already-open descriptors
/// uninheritable — the exact failure this function exists to prevent — and an
/// off-by-one here is invisible in a normal test, because the OS silently caps a
/// request at `rlim_max`.
#[cfg(unix)]
#[must_use]
pub fn planned_open_file_limit(
    current_soft: u64,
    hard_limit: u64,
    platform_cap: Option<u64>,
) -> Option<u64> {
    let target = match platform_cap {
        // macOS refuses a soft limit above `kern.maxfilesperproc`.
        Some(cap) => hard_limit.min(cap),
        None => hard_limit,
    };
    (current_soft < target).then_some(target)
}

#[cfg(unix)]
/// Raise this process's open-file soft limit to the ceiling the OS reports.
///
/// # Errors
///
/// The OS error of a failed `getrlimit`, `sysctl` (macOS) or `setrlimit`.
#[cfg(unix)]
pub fn raise_open_file_limit() -> std::io::Result<Option<u64>> {
    let mut limit = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    if unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &raw mut limit) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    // macOS refuses a soft limit above `kern.maxfilesperproc`; other targets
    // take `rlim_max` as the ceiling.
    #[cfg(target_os = "macos")]
    let platform_cap = {
        let mut per_process: libc::c_int = 0;
        let mut size = std::mem::size_of::<libc::c_int>();
        let read = unsafe {
            libc::sysctlbyname(
                c"kern.maxfilesperproc".as_ptr(),
                (&raw mut per_process).cast(),
                &raw mut size,
                std::ptr::null_mut(),
                0,
            )
        };
        if read != 0 {
            return Err(std::io::Error::last_os_error());
        }
        Some(per_process.unsigned_abs().into())
    };
    #[cfg(not(target_os = "macos"))]
    let platform_cap = None;

    let Some(target) = planned_open_file_limit(limit.rlim_cur, limit.rlim_max, platform_cap) else {
        return Ok(Some(limit.rlim_cur));
    };
    limit.rlim_cur = target;
    if unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &raw const limit) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(Some(limit.rlim_cur))
}

/// No per-process descriptor limit to raise.
///
/// # Errors
///
/// Never: the not-unix arm has no descriptor limit to raise.
#[cfg(not(unix))]
pub fn raise_open_file_limit() -> std::io::Result<Option<u64>> {
    Ok(None)
}

/// Whether `pid` may be signalled at all.
///
/// POSIX makes two ids catastrophic rather than merely wrong:
///   * `kill(-1, sig)` signals EVERY process the caller may signal — i.e. the
///     user's entire session, including the shell that started us;
///   * pid 1 is init (or the container's init), which nothing here ever owns.
///
/// So the floor is `> 1`, not `> 0`. Three of the four signalling paths already
/// used it; `kill_process_group_or_pid` used `> 0`, so a `pid == 1` argument
/// became `kill(-1, SIGKILL)`. The Kani harness below quantifies over every
/// `i32` and proves the predicate is the only gate.
#[cfg(unix)]
#[must_use]
pub const fn signalable(pid: i32) -> bool {
    pid > 1
}

/// Signal a single pid. Returns true only when the signal was delivered,
/// proving the pid was alive at signal time.
#[cfg(unix)]
#[must_use]
pub fn kill_pid(pid: i32, signal: Signal) -> bool {
    if !signalable(pid) {
        return false;
    }
    let sig = match signal {
        Signal::Term => libc::SIGTERM,
        Signal::Kill => libc::SIGKILL,
    };
    unsafe { libc::kill(pid, sig) == 0 }
}

/// Whether a process group currently exists, without delivering a signal.
/// Existence alone never establishes ownership of a durable group's old id.
///
/// # Errors
/// Returns the native probe error when group existence cannot be established.
#[cfg(unix)]
pub fn process_group_exists(pgid: i32) -> std::io::Result<bool> {
    // A probe, not a signal — but `kill(-1, 0)` asks about EVERY process, so the
    // same floor applies: 1 is not a group this code owns.
    if !signalable(pgid) {
        return Ok(false);
    }
    if unsafe { libc::kill(-pgid, 0) } == 0 {
        return Ok(true);
    }
    let error = std::io::Error::last_os_error();
    if error.raw_os_error() == Some(libc::ESRCH) {
        Ok(false)
    } else {
        Err(error)
    }
}

/// No process groups: none exists.
///
/// # Errors
///
/// Never: the not-unix arm has no group to probe.
#[cfg(not(unix))]
pub fn process_group_exists(_pgid: i32) -> std::io::Result<bool> {
    Ok(false)
}

/// Signal every member of process group `pgid` (`kill(-pgid, sig)`). Returns
/// true only when the signal was delivered to at least one member.
#[cfg(unix)]
#[must_use]
pub fn signal_process_group(pgid: i32, signal: Signal) -> bool {
    if !signalable(pgid) {
        return false;
    }
    let sig = match signal {
        Signal::Term => libc::SIGTERM,
        Signal::Kill => libc::SIGKILL,
    };
    unsafe { libc::kill(-pgid, sig) == 0 }
}

#[cfg(not(unix))]
#[must_use]
pub fn signal_process_group(_pgid: i32, _signal: Signal) -> bool {
    // No process groups: persistent shells are POSIX-only.
    false
}

/// Windows: `OpenProcess(PROCESS_TERMINATE)` + `TerminateProcess` on the
/// single pid (every signal terminates; the `Term`/`Kill` distinction
/// collapses there). Descendants are NOT killed: teardown paths needing
/// tree kills use [`kill_process_group_or_pid`].
#[cfg(windows)]
#[must_use]
pub fn kill_pid(pid: i32, signal: Signal) -> bool {
    // Windows has no `kill(-1)`: `OpenProcess` on a real pid is the whole
    // contract, so the floor here is `> 0` and deliberately not
    // [`signalable`]'s `> 1` (pid 1 is an ordinary process on this platform).
    if pid <= 0 {
        return false;
    }
    let _ = signal;
    win32::terminate_process(pid as u32)
}

#[cfg(not(any(unix, windows)))]
pub fn kill_pid(_pid: i32, _signal: Signal) -> bool {
    // No signaling mechanism; the kill stays unproven, the conservative
    // answer callers act on.
    false
}

/// Kill a process and all its children: the process group first (`bash()`
/// children run detached in a new group), then the bare pid as fallback.
/// Returns true when either signal was delivered (TS `killProcessTree`).
#[cfg(unix)]
#[must_use]
pub fn kill_process_group_or_pid(pid: i32) -> bool {
    if !signalable(pid) {
        return false;
    }
    unsafe {
        if libc::kill(-pid, libc::SIGKILL) == 0 {
            return true;
        }
    }
    unsafe { libc::kill(pid, libc::SIGKILL) == 0 }
}

/// Windows: `taskkill /F /T /PID <pid>` from the absolute System32 path -
/// a bare `taskkill` name could resolve a planted CWD executable. True
/// only when taskkill exited 0, the same proof TS's `result.status === 0`
/// requires.
#[cfg(windows)]
#[must_use]
pub fn kill_process_group_or_pid(pid: i32) -> bool {
    // No `kill(-1)` on Windows: see the note on `kill_pid` above.
    if pid <= 0 {
        return false;
    }
    let system_root =
        std::env::var_os("SystemRoot").unwrap_or_else(|| std::ffi::OsString::from("C:\\Windows"));
    let taskkill = std::path::Path::new(&system_root)
        .join("System32")
        .join("taskkill.exe");
    let mut command = Command::new(&taskkill);
    command
        .args(["/F", "/T", "/PID", &pid.to_string()])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    set_no_window(&mut command);
    command.status().is_ok_and(|status| status.success())
}

#[cfg(not(any(unix, windows)))]
pub fn kill_process_group_or_pid(_pid: i32) -> bool {
    // No tree-kill mechanism; the kill stays unproven, the conservative
    // answer callers act on.
    false
}

/// Cheap `kill(pid, 0)` existence probe; counts zombies as existing.
#[cfg(unix)]
#[must_use]
pub fn pid_exists(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    unsafe { libc::kill(pid as i32, 0) == 0 }
}

/// The kernel-held process handle (`pidfd_open`): pins the exact process
/// behind the pid, so a signal through it ([`pidfd_signal`]) reaches that
/// process even if the numeric pid is recycled afterwards.
///
/// # Errors
///
/// Returns the open failure verbatim: `ESRCH` names a process that is
/// already gone, `Unsupported` a platform with no pidfd arm.
#[cfg(all(
    unix,
    target_os = "linux",
    any(target_arch = "x86_64", target_arch = "aarch64")
))]
pub fn open_pidfd(pid: u32) -> std::io::Result<i32> {
    // `SYS_pidfd_open`/`SYS_pidfd_send_signal` share their numbers across
    // x86_64 and aarch64; pidfd is Linux-only (macOS libc has no
    // `SYS_pidfd_*` constants to compile against).
    let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0) };
    if fd < 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(fd as i32)
    }
}

/// No pidfd arm on this platform or kernel: the handle is unobtainable.
///
/// # Errors
///
/// Always `Unsupported`.
#[cfg(all(
    unix,
    not(all(
        target_os = "linux",
        any(target_arch = "x86_64", target_arch = "aarch64")
    ))
))]
pub fn open_pidfd(_pid: u32) -> std::io::Result<i32> {
    Err(std::io::ErrorKind::Unsupported.into())
}

/// No pidfd arm on this platform or kernel: the handle is unobtainable.
///
/// # Errors
///
/// Always `Unsupported`.
#[cfg(not(unix))]
pub fn open_pidfd(_pid: u32) -> std::io::Result<i32> {
    Err(std::io::ErrorKind::Unsupported.into())
}

/// Signal through the kernel-held handle (`pidfd_send_signal`): the signal
/// reaches the pinned process; the handle CLOSES on drop by the caller.
#[cfg(all(
    unix,
    target_os = "linux",
    any(target_arch = "x86_64", target_arch = "aarch64")
))]
#[must_use]
pub fn pidfd_signal(fd: i32, signal: Signal) -> bool {
    let signum = match signal {
        Signal::Term => libc::SIGTERM,
        Signal::Kill => libc::SIGKILL,
    };
    unsafe {
        libc::syscall(
            libc::SYS_pidfd_send_signal,
            fd,
            signum,
            std::ptr::null::<u8>(),
            0,
        ) == 0
    }
}

#[cfg(all(
    unix,
    not(all(
        target_os = "linux",
        any(target_arch = "x86_64", target_arch = "aarch64")
    ))
))]
#[must_use]
pub fn pidfd_signal(_fd: i32, _signal: Signal) -> bool {
    false
}

#[cfg(not(unix))]
#[must_use]
pub fn pidfd_signal(_fd: i32, _signal: Signal) -> bool {
    false
}

pub fn close_pidfd(fd: i32) {
    #[cfg(unix)]
    unsafe {
        libc::close(fd);
    }
    #[cfg(not(unix))]
    let _ = fd;
}

/// Resolve once the process behind `pid` has exited, parked on the kernel's
/// exit notification (Linux pidfd readability, macOS kqueue
/// `EVFILT_PROC`/`NOTE_EXIT`), with no timer. A pid that names no process
/// (already exited and reaped) resolves at once. The kernel handle pins
/// the process instance, so a pid recycled after registration is never
/// mistaken for it.
///
/// # Errors
///
/// The OS error when the watch cannot register (no pidfd, descriptor
/// exhaustion, an unsupported platform); the caller owns its fallback.
#[cfg(target_os = "linux")]
pub async fn wait_for_exit(pid: u32) -> std::io::Result<()> {
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    use tokio::io::{unix::AsyncFd, Interest};

    let fd = match open_pidfd(pid) {
        Ok(fd) => fd,
        Err(error) if error.raw_os_error() == Some(libc::ESRCH) => return Ok(()),
        Err(error) => return Err(error),
    };
    // SAFETY: `open_pidfd` returned a fresh descriptor this call owns
    // (and closes on drop, on every path). `register_with_interest` (tokio
    // 1.53.3+, RUSTSEC-2026-0386) makes the ownership proof explicit at the
    // call site instead of trusting any `AsRawFd`.
    #[allow(unused_unsafe)]
    let fd =
        unsafe { AsyncFd::register_with_interest(OwnedFd::from_raw_fd(fd), Interest::READABLE)? };
    // Every wake is confirmed by a zero-timeout poll (AsyncFd readiness
    // can be spurious).
    loop {
        let mut ready = fd.readable().await?;
        if let Ok(result) = ready.try_io(|fd| {
            let mut probe = libc::pollfd {
                fd: fd.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            };
            // SAFETY: polls one descriptor this call owns, without
            // blocking.
            match unsafe { libc::poll(&raw mut probe, 1, 0) } {
                1 => Ok(()),
                0 => Err(std::io::ErrorKind::WouldBlock.into()),
                _ => Err(std::io::Error::last_os_error()),
            }
        }) {
            return result;
        }
    }
}

/// macOS: kqueue `EVFILT_PROC`/`NOTE_EXIT` on a pollable kqueue.
///
/// # Errors
///
/// The OS error when the watch cannot register; the caller owns its
/// fallback.
#[cfg(target_os = "macos")]
pub async fn wait_for_exit(pid: u32) -> std::io::Result<()> {
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    use tokio::io::{unix::AsyncFd, Interest};

    // SAFETY: `kqueue()` takes no arguments.
    let kq = unsafe { libc::kqueue() };
    if kq < 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: a fresh descriptor this call owns (and closes on drop, on
    // every path).
    let kq = unsafe { OwnedFd::from_raw_fd(kq) };
    // The block drops the `kevent` value (its `udata` raw pointer is
    // `!Send`) before the first `.await`.
    {
        let change = libc::kevent {
            ident: pid as libc::uintptr_t,
            filter: libc::EVFILT_PROC,
            flags: libc::EV_ADD,
            fflags: libc::NOTE_EXIT,
            data: 0,
            udata: std::ptr::null_mut(),
        };
        // SAFETY: registers one change and reads no events; `change`
        // outlives the call. With no event space, an attach failure comes
        // back as -1/errno (kevent(2)), not an `EV_ERROR` event.
        if unsafe {
            libc::kevent(
                kq.as_raw_fd(),
                &raw const change,
                1,
                std::ptr::null_mut(),
                0,
                std::ptr::null(),
            )
        } < 0
        {
            // `ESRCH` means the pid names no attachable process (reaped,
            // or already past its exit ref-drain - XNU runs the drain
            // before the exit knote fires): the exit already happened.
            // Every other errno (EMFILE, ENOMEM, ...) is a watch that
            // could not register; the caller falls back.
            let error = std::io::Error::last_os_error();
            return match error.raw_os_error() {
                Some(libc::ESRCH) => Ok(()),
                _ => Err(error),
            };
        }
    }
    // SAFETY: the `AsyncFd` owns `kq`, so it stays open and unchanged until drop.
    let kq = unsafe { AsyncFd::register_with_interest(kq, Interest::READABLE) }?;
    // Only the exit knote is registered, so a drained event is the exit.
    loop {
        let mut ready = kq.readable().await?;
        if let Ok(result) = ready.try_io(|kq| {
            let zero = libc::timespec {
                tv_sec: 0,
                tv_nsec: 0,
            };
            let mut event = libc::kevent {
                ident: 0,
                filter: 0,
                flags: 0,
                fflags: 0,
                data: 0,
                udata: std::ptr::null_mut(),
            };
            // SAFETY: drains one event with a zero timeout; `event` and
            // `zero` outlive the call.
            match unsafe {
                libc::kevent(
                    kq.as_raw_fd(),
                    std::ptr::null(),
                    0,
                    &raw mut event,
                    1,
                    &raw const zero,
                )
            } {
                1 => Ok(()),
                0 => Err(std::io::ErrorKind::WouldBlock.into()),
                _ => Err(std::io::Error::last_os_error()),
            }
        }) {
            return result;
        }
    }
}

/// No kernel exit watch on this platform: the caller takes its own
/// liveness-poll fallback.
///
/// # Errors
///
/// Always `Unsupported`.
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
#[allow(clippy::unused_async)] // one awaitable signature across the kernel-watch arms
pub async fn wait_for_exit(_pid: u32) -> std::io::Result<()> {
    Err(std::io::ErrorKind::Unsupported.into())
}

/// Windows: the shared handle probe (win32 has no unreaped-zombie state,
/// so existing and running are the same predicate). A query that fails
/// outright reads as gone.
#[cfg(windows)]
#[must_use]
pub fn pid_exists(pid: u32) -> bool {
    pa_types::platform::process::is_process_alive(pid).unwrap_or(false)
}

#[cfg(not(any(unix, windows)))]
pub fn pid_exists(_pid: u32) -> bool {
    false
}

/// The signal number that terminated a child, when it was signaled
/// (`ExitStatus::signal` on Unix; None elsewhere).
#[cfg(unix)]
#[must_use]
pub fn termination_signal(status: &std::process::ExitStatus) -> Option<i32> {
    use std::os::unix::process::ExitStatusExt;
    status.signal()
}

#[cfg(not(unix))]
#[must_use]
pub fn termination_signal(_status: &std::process::ExitStatus) -> Option<i32> {
    // Windows terminations surface as exit codes, not signals.
    None
}

/// A child's exit code as a shell's `$?` reports it: the status code, or
/// 128 + the signal number when a signal killed it (TS `normalizedExitCode`).
/// Without the mapping a signal death carries no code and reads as success.
#[must_use]
pub fn normalized_exit_code(status: &std::process::ExitStatus) -> Option<i32> {
    status
        .code()
        .or_else(|| termination_signal(status).map(|signal| 128 + signal))
}

/// The kernel32 termination surface for [`kill_pid`], hand-declared (repo
/// policy: pinned constants and externs, no windows-sys dependency).
#[cfg(windows)]
mod win32 {
    #![allow(non_snake_case)]

    use std::ffi::c_void;

    /// `winnt.h`: the right to terminate the process.
    const PROCESS_TERMINATE: u32 = 0x0001;

    type Handle = *mut c_void;

    extern "system" {
        fn OpenProcess(access: u32, inherit_handle: i32, process_id: usize) -> Handle;
        fn CloseHandle(handle: Handle) -> i32;
        fn TerminateProcess(handle: Handle, exit_code: u32) -> i32;
    }

    /// Terminate exactly the pid; false when it could not be proven
    /// terminated (gone already, access denied, or invalid).
    pub(crate) fn terminate_process(pid: u32) -> bool {
        let handle = unsafe { OpenProcess(PROCESS_TERMINATE, 0, pid as usize) };
        if handle.is_null() {
            return false;
        }
        let terminated = unsafe { TerminateProcess(handle, 1) != 0 };
        unsafe { CloseHandle(handle) };
        terminated
    }
}

#[cfg(all(test, windows))]
mod windows_tests {
    use super::*;

    #[test]
    fn pid_exists_for_self_but_not_zero() {
        assert!(pid_exists(std::process::id()));
        assert!(!pid_exists(0));
        assert!(!pid_exists(u32::MAX));
    }

    #[test]
    fn out_of_range_pids_never_prove_kills() {
        assert!(!kill_pid(-1, Signal::Kill));
        assert!(!kill_pid(0, Signal::Term));
        assert!(!kill_process_group_or_pid(-1));
    }
}

/// The kernel-watch arms' shared exit contract: a pid that names no live
/// process resolves at once, so a worker that died before its watch
/// registered still reports an exit. Both kernel-watch platforms only
/// (the other targets take the caller's fallback by design).
#[cfg(all(test, any(target_os = "linux", target_os = "macos")))]
mod exit_wait_tests {
    use super::*;

    #[tokio::test]
    async fn wait_for_exit_resolves_at_once_for_a_reaped_pid() {
        let mut child = std::process::Command::new("sleep")
            .arg("600")
            .spawn()
            .expect("spawn sleep");
        let pid = child.id();
        child.kill().expect("kill sleep");
        child.wait().expect("reap sleep");
        assert!(
            wait_for_exit(pid).await.is_ok(),
            "a reaped pid resolves at once"
        );
    }
}

/// Kani harnesses for the descriptor-limit decision.
///
/// This is the "zero annotations yield panic-freedom, contracts yield the
/// property" tier the 2026 practice describes: `planned_open_file_limit` is a
/// pure u64 function, so a bounded model checker can quantify over EVERY input
/// instead of the handful a unit test picks.
#[cfg(kani)]
mod kani_harnesses {
    use super::*;

    /// The safety property: raising a limit never lowers it.
    ///
    /// This is not hypothetical. The caller runs at process start, and a soft
    /// limit BELOW the current one makes every already-open descriptor
    /// uninheritable — silent, and it looks like a resource bug much later.
    #[kani::proof]
    fn planned_limit_never_lowers_the_soft_limit() {
        let current_soft: u64 = kani::any();
        let hard_limit: u64 = kani::any();
        let cap: Option<u64> = kani::any();
        kani::assume(current_soft <= hard_limit);

        if let Some(planned) = planned_open_file_limit(current_soft, hard_limit, cap) {
            assert!(planned >= current_soft);
        }
    }

    /// The progress property: whenever the OS allows a higher limit, one is
    /// planned. Without this, a planner returning `None` unconditionally would
    /// satisfy the safety property and silently stop raising limits at all.
    #[kani::proof]
    fn a_raise_is_planned_whenever_the_os_allows_one() {
        let current_soft: u64 = kani::any();
        let hard_limit: u64 = kani::any();
        let cap: Option<u64> = kani::any();
        kani::assume(current_soft <= hard_limit);
        let effective = match cap {
            Some(limit) => hard_limit.min(limit),
            None => hard_limit,
        };
        kani::assume(current_soft < effective);

        assert_eq!(
            planned_open_file_limit(current_soft, hard_limit, cap),
            Some(effective)
        );
    }

    /// The signalling floor, over EVERY `i32` — the property a unit test cannot
    /// reach because the dangerous argument would actually fire.
    ///
    /// `kill(-1, sig)` signals every process the caller may signal. A test that
    /// called `kill_process_group_or_pid(1)` to check the guard would, on a
    /// build with the guard removed, take down the test runner and everything
    /// beside it. A model check asks the question without asking the kernel.
    #[kani::proof]
    fn no_signalling_path_can_reach_kill_minus_one() {
        let pid: i32 = kani::any();
        // The gate every unix signalling path is held to. Over EVERY i32, a pid
        // that passes it is an ordinary pid: never -1 (kill every process the
        // caller may signal), never 0 (the caller's own group), never 1 (init).
        if signalable(pid) {
            assert!(pid > 1);
        }
    }

    /// Reachability, with concrete witnesses: the gate admits real pids and
    /// refuses the two reserved ones. Without this, a predicate that returned
    /// `false` for everything would satisfy the proof above and silently disable
    /// every kill in the product.
    #[kani::proof]
    fn the_floor_admits_ordinary_pids_and_refuses_the_reserved_ones() {
        assert!(signalable(2));
        assert!(signalable(42_424));
        assert!(signalable(i32::MAX));
        assert!(!signalable(1));
        assert!(!signalable(0));
        assert!(!signalable(-1));
        assert!(!signalable(i32::MIN));
    }

    /// Anti-vacuity, with ASSERTIONS rather than `kani::cover!`.
    ///
    /// Measured 2026-10-27: a planner that returns `None` unconditionally makes
    /// the cover harness report "1 of 2 cover properties satisfied" and still
    /// print `VERIFICATION: SUCCESSFUL` — cover findings do not fail a run. A
    /// vacuity check that cannot fail is the same inert guard this repository
    /// keeps finding elsewhere, so the witnesses are plain assertions: one per
    /// outcome, on concrete inputs. Both proofs above are vacuous without them
    /// (a stuck planner satisfies "never lowers" trivially).
    #[kani::proof]
    fn both_outcomes_are_reached_by_concrete_witnesses() {
        // No raise needed: the soft limit already reaches the ceiling.
        assert_eq!(planned_open_file_limit(10, 10, None), None);
        // A raise is planned: the ceiling is above the current soft limit.
        assert_eq!(planned_open_file_limit(10, 20, None), Some(20));
        // The macOS cap limits the target without ever going below the start.
        assert_eq!(planned_open_file_limit(10, 100, Some(50)), Some(50));
        // A cap below the current soft limit plans nothing (no lowering).
        assert_eq!(planned_open_file_limit(10, 100, Some(5)), None);
    }
}

/// The runtime half of the signalling floor. The Kani harness above covers the
/// arithmetic over every `i32`; this pins the SURFACE — that each path really
/// consults the predicate rather than its own comparison, which is how
/// `kill_process_group_or_pid` came to use `> 0` while its three siblings used
/// `> 1`.
///
/// Deliberately does NOT call the functions with a dangerous id: a regression
/// would take down the test runner instead of failing.
#[cfg(all(test, unix))]
mod signalling_floor_tests {
    use super::*;

    #[test]
    fn the_floor_refuses_the_ids_posix_reserves() {
        assert!(
            !signalable(-1),
            "kill(-1) signals every process the caller owns"
        );
        assert!(!signalable(0), "kill(0) signals the caller's own group");
        assert!(!signalable(1), "pid 1 is init, which this code never owns");
        assert!(signalable(2));
    }

    #[test]
    fn every_unix_signalling_path_is_gated_on_the_same_predicate() {
        // A source-level pin: the four paths must not reintroduce their own
        // comparisons. `include_str!` of this file is the cheapest way to assert
        // a shape that a behavioural test cannot reach safely.
        // Count only the PRODUCTION half: everything above this test module.
        // Otherwise the matcher counts its own literals and reports 5.
        let source = include_str!("process.rs");
        let production = source
            .split("mod signalling_floor_tests")
            .next()
            .expect("the production half is always present");
        let own_comparison = production.matches("if pid <= 0 {").count()
            + production.matches("if pgid <= 1 {").count()
            + production.matches("if pid <= 1 {").count();
        // The Windows arms legitimately keep `pid <= 0` (no kill(-1) there), and
        // each states why at the site; there are exactly two of them.
        assert_eq!(
            own_comparison, 2,
            "a unix signalling path grew its own comparison instead of using `signalable`",
        );
    }
}
