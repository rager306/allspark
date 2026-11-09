//! The `allspark` launcher e2e: the tracked launcher script
//! (`scripts/allspark-launcher.sh`) is the fork's user-facing entry point, so
//! its contract is verified against the real compiled payload, not a mock:
//!
//! 1. substitution — `HOME` moves to `<upstream HOME>/.allspark-home`
//!    (`ALLSPARK_HOME` wins when set) and the daemon socket is pinned to the
//!    fork's own `$ALLSPARK_HOME/run/daemon.sock`, in a private 0700 directory;
//! 2. seeding — first run copies upstream `settings.json`/`models.json` into
//!    the isolated home (`auth.json` deliberately not copied);
//! 3. the real fork binary answers `--version` with the fork-branded line.

#![cfg(unix)]
#![allow(clippy::type_complexity)]

use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

/// The repo root (`crates/pa-cli` -> `crates` -> root).
fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("repo root")
        .to_path_buf()
}

fn launcher_script() -> PathBuf {
    repo_root().join("scripts").join("allspark-launcher.sh")
}

/// Deploy the tracked launcher with `payload` as its libexec binary:
/// `<tmp>/bin/allspark` + `<tmp>/libexec/<payload file name>`, mirroring the
/// installed layout (`bin/allspark` + `libexec/prime-agent`).
fn deploy_launcher(dir: &Path, payload: &Path) -> PathBuf {
    let bin = dir.join("bin");
    let libexec = dir.join("libexec");
    fs::create_dir_all(&bin).expect("bin dir");
    fs::create_dir_all(&libexec).expect("libexec dir");
    let launcher = bin.join("allspark");
    fs::copy(launcher_script(), &launcher).expect("copy launcher");
    fs::copy(
        payload,
        libexec.join(payload.file_name().expect("payload name")),
    )
    .expect("copy payload");
    launcher
}

/// A payload stand-in that prints the two isolation variables the launcher
/// must substitute: `HOME` and `PRIME_AGENT_DAEMON_SOCKET`.
fn write_env_probe_payload_at(payload: &Path) {
    let mut f = fs::File::create(payload).expect("create probe payload");
    let script = "#!/bin/sh\nprintf 'HOME=%s\\nSOCKET=%s\\nAGENT_DIR=%s\\n' \"$HOME\" \
        \"$PRIME_AGENT_DAEMON_SOCKET\" \"$PRIME_AGENT_CODING_AGENT_DIR\"\n";
    f.write_all(script.as_bytes()).expect("write probe payload");
    f.sync_all().expect("sync probe payload");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(payload, fs::Permissions::from_mode(0o755))
            .expect("chmod probe payload");
    }
}

/// `HOME` must move under `.allspark-home` and the socket must pin to the
/// fork's own file; `ALLSPARK_HOME` overrides the isolated home.
#[test]
fn launcher_substitutes_home_and_pins_the_socket() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let upstream_home = tmp.path().join("upstream-home");
    fs::create_dir_all(&upstream_home).expect("upstream home");
    // The launcher layout first (creates libexec/), then the probe payload.
    let probe_path = tmp.path().join("libexec").join("prime-agent");

    // Default: HOME -> <upstream HOME>/.allspark-home.
    let launcher = deploy_launcher(tmp.path(), Path::new(env!("CARGO_BIN_EXE_prime-agent")));
    fs::remove_file(tmp.path().join("libexec").join("prime-agent")).expect("drop real payload");
    write_env_probe_payload_at(&probe_path);
    let out = Command::new(&launcher)
        .env("HOME", &upstream_home)
        .env_remove("ALLSPARK_HOME")
        // An ambient upstream state dir (launched inside a prime-agent
        // session) must be overridden by the launcher's own pin.
        .env("PRIME_AGENT_CODING_AGENT_DIR", "/root/.prime/agent")
        // An ambient pin (e.g. launched inside a running prime-agent session)
        // is respected by design; remove it so the launcher's own default is
        // what lands in the payload's environment.
        .env_remove("PRIME_AGENT_DAEMON_SOCKET")
        .output()
        .expect("run launcher");
    assert!(out.status.success(), "launcher failed: {out:?}");
    let text = String::from_utf8_lossy(&out.stdout);
    let home_line = text
        .lines()
        .find(|l| l.starts_with("HOME="))
        .expect("HOME in probe output");
    let expected_home = upstream_home.join(".allspark-home");
    assert_eq!(home_line, format!("HOME={}", expected_home.display()));
    let socket_line = text
        .lines()
        .find(|l| l.starts_with("SOCKET="))
        .expect("SOCKET in probe output");
    // A PRIVATE per-user directory, not the shared temp dir: a predictable path in /tmp is
    // bindable by any local user before us, and the client trusts the kernel's peer-uid
    // answer only (review #17, CWE-290).
    let socket = socket_line.trim_start_matches("SOCKET=");
    assert_eq!(
        Path::new(socket),
        expected_home.join("run").join("daemon.sock"),
        "the daemon socket must live in the fork's private run directory: {socket_line}"
    );
    let dir_mode = fs::metadata(expected_home.join("run"))
        .expect("the socket directory exists")
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(
        dir_mode, 0o700,
        "the socket directory must be private (0700), not {dir_mode:o}"
    );
    // An ambient upstream agent-dir override must be pinned to the isolated
    // home: otherwise the fork reads the upstream models.json/auth (the
    // reported "the fork sees upstream providers" leak).
    let agent_line = text
        .lines()
        .find(|l| l.starts_with("AGENT_DIR="))
        .expect("AGENT_DIR in probe output");
    assert_eq!(
        agent_line,
        format!(
            "AGENT_DIR={}",
            expected_home.join(".prime").join("agent").display()
        ),
        "the agent dir must pin into the isolated home: {agent_line}"
    );

    // Override: ALLSPARK_HOME wins over the HOME-derived path.
    let override_home = tmp.path().join("custom-home");
    let out = Command::new(&launcher)
        .env("HOME", &upstream_home)
        .env("ALLSPARK_HOME", &override_home)
        .output()
        .expect("run launcher with override");
    assert!(out.status.success(), "launcher failed: {out:?}");
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        text.contains(&format!("HOME={}", override_home.display())),
        "ALLSPARK_HOME must override: {text}"
    );
}

/// No seeding by design: the fork starts standalone — upstream
/// `settings.json` / `models.json` / `auth.json` must NOT be copied into the
/// isolated home (upstream providers and credentials must never leak in).
#[test]
fn launcher_never_seeds_upstream_state() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let upstream_home = tmp.path().join("upstream-home");
    let upstream_config = upstream_home.join(".prime").join("agent");
    fs::create_dir_all(&upstream_config).expect("upstream config dir");
    fs::write(upstream_config.join("settings.json"), "{\"a\":1}").expect("settings");
    fs::write(upstream_config.join("models.json"), "{\"providers\":{}}").expect("models");
    fs::write(upstream_config.join("auth.json"), "{\"secret\":true}").expect("auth");

    let payload = env!("CARGO_BIN_EXE_prime-agent");
    let launcher = deploy_launcher(tmp.path(), Path::new(payload));
    let out = Command::new(&launcher)
        .arg("--version")
        .env("HOME", &upstream_home)
        .env_remove("ALLSPARK_HOME")
        .output()
        .expect("run launcher");
    assert!(out.status.success(), "launcher failed: {out:?}");

    let isolated_config = upstream_home
        .join(".allspark-home")
        .join(".prime")
        .join("agent");
    for name in ["settings.json", "models.json", "auth.json"] {
        assert!(
            !isolated_config.join(name).exists(),
            "{name} must NOT be seeded from the upstream install"
        );
    }
}

/// The real compiled fork binary answers `--version` through the launcher
/// with the fork-branded line: the fork build id distinguishes binaries and
/// the upstream base is named.
#[test]
fn launcher_runs_the_real_fork_binary_version() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let payload = env!("CARGO_BIN_EXE_prime-agent");
    let launcher = deploy_launcher(tmp.path(), Path::new(payload));
    let upstream_home = tmp.path().join("upstream-home");
    fs::create_dir_all(&upstream_home).expect("upstream home");

    let out = Command::new(&launcher)
        .arg("--version")
        .env("HOME", &upstream_home)
        .env_remove("ALLSPARK_HOME")
        .output()
        .expect("run launcher");
    assert!(out.status.success(), "launcher failed: {out:?}");
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        text.starts_with("allspark ")
            && text.contains("(fork build ")
            && text.contains("upstream prime-agent "),
        "--version must carry the fork brand, the fork build id and the \
         upstream base: {text}"
    );
}

/// Re-running the launcher from inside an allspark session must not nest the home.
///
/// `allspark` exports `HOME=$ALLSPARK_HOME`, so a launcher started from a session
/// it created computes `$HOME/.allspark-home` =
/// `…/.allspark-home/.allspark-home`: a second agent home with its own kernel venv,
/// uv toolchain and sessions that the outer home cannot see. Measured on a live
/// machine on 2026-11-06: 184 MB, one session, four days idle.
///
/// The fix must be idempotent — one nesting level and two both land in the SAME
/// home — and must not touch a custom `ALLSPARK_HOME` that is not the default shape.
#[test]
fn a_launcher_run_from_inside_the_home_does_not_nest_it() {
    let script = std::fs::read_to_string(launcher_script()).expect("read the launcher");
    // Everything up to the payload lookup: the path resolution under test, with no
    // exec and no payload needed.
    let head = &script[..script.find("PAYLOAD_DIR=").expect("the payload lookup")];
    let probe = format!("{head}\nprintf '%s\\n' \"$ALLSPARK_HOME\"\n");

    let resolve = |home: &str, explicit: Option<&str>| -> String {
        let mut command = Command::new("sh");
        command
            .arg("-c")
            .arg(&probe)
            .env("HOME", home)
            .env_remove("ALLSPARK_HOME");
        if let Some(explicit) = explicit {
            command.env("ALLSPARK_HOME", explicit);
        }
        let output = command.output().expect("run the launcher head");
        assert!(
            output.status.success(),
            "the launcher head runs: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    };

    // The normal case is unchanged.
    assert_eq!(resolve("/home/u", None), "/home/u/.allspark-home");
    // One level of nesting collapses to the outer home — this is the bug.
    assert_eq!(
        resolve("/home/u/.allspark-home", None),
        "/home/u/.allspark-home",
        "a launcher run from inside its own home must reuse that home, not nest a copy"
    );
    // Two levels collapse too: the walk is a loop, not a single trim.
    assert_eq!(
        resolve("/home/u/.allspark-home/.allspark-home", None),
        "/home/u/.allspark-home",
        "the collapse must repeat, not run once"
    );
    // An explicit, differently-shaped home is untouched.
    assert_eq!(
        resolve("/home/u", Some("/srv/allspark")),
        "/srv/allspark",
        "a custom ALLSPARK_HOME that does not nest must be used as given"
    );
    // And a custom home that IS nested still collapses to its own root.
    assert_eq!(
        resolve(
            "/home/u",
            Some("/srv/allspark/.allspark-home/.allspark-home")
        ),
        "/srv/allspark/.allspark-home",
        "nesting collapses under a custom root too"
    );
}

/// The RELEASE launcher and the tracked launcher are the same contract twice.
///
/// `scripts/allspark-launcher.sh` is the source install's entry point;
/// `install-rust.sh` writes a second launcher into the release prefix. Nothing checked
/// that they agree, and they had already drifted: the embedded one lacked the
/// never-nest collapse, so a tarball install re-created the nested homedir the tracked
/// script had just been fixed against (184 MB measured, one level down and invisible to
/// the outer home).
///
/// The two are not textually equal on purpose — the release one is a heredoc with its
/// payload path substituted — so the test pins the CONTRACT: both resolve their home to
/// the outermost `…/.allspark-home` from the same five environments.
#[test]
fn the_release_launcher_and_the_tracked_one_resolve_the_same_home() {
    let installer = fs::read_to_string(repo_root().join("install-rust.sh")).expect("installer");
    let start = installer
        .find("cat > \"$launcher_tmp\" <<EOF")
        .expect("the fork launcher heredoc");
    let end = installer[start..]
        .find("\nEOF\n")
        .map(|at| start + at)
        .expect("the heredoc ends");
    let embedded = installer[start..end]
        .split("<<EOF\n")
        .nth(1)
        .expect("the heredoc body")
        .replace("\\$", "$");
    // Everything up to the payload exec: the path resolution under test.
    let head = embedded
        .split("mkdir -p \"$ALLSPARK_HOME\"")
        .next()
        .expect("the home resolution");
    let probe = format!("{head}\nprintf '%s\\n' \"$ALLSPARK_HOME\"\n");

    let resolve = |home: &str, explicit: Option<&str>| -> String {
        let mut command = Command::new("sh");
        command
            .arg("-c")
            .arg(&probe)
            .env("HOME", home)
            .env_remove("ALLSPARK_HOME");
        if let Some(explicit) = explicit {
            command.env("ALLSPARK_HOME", explicit);
        }
        let output = command.output().expect("run the embedded launcher head");
        assert!(
            output.status.success(),
            "the embedded launcher head runs: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    };

    assert_eq!(resolve("/home/u", None), "/home/u/.allspark-home");
    assert_eq!(
        resolve("/home/u/.allspark-home", None),
        "/home/u/.allspark-home",
        "a release install must not nest either: the same bug the tracked script fixed"
    );
    assert_eq!(
        resolve("/home/u/.allspark-home/.allspark-home", None),
        "/home/u/.allspark-home",
        "the collapse repeats, as in the tracked launcher"
    );
    assert_eq!(resolve("/home/u", Some("/srv/allspark")), "/srv/allspark");
    assert_eq!(
        resolve(
            "/home/u",
            Some("/srv/allspark/.allspark-home/.allspark-home")
        ),
        "/srv/allspark/.allspark-home"
    );
}

/// A 0755 socket directory is refused even when WE own it.
///
/// The mode check accepted 700, 750 and 755 on the reasoning that only the mode decided who
/// could write. It does not: on a multi-user host 0755 lets anyone traverse and read, and the
/// ownership check is what catches a directory another user created — so both are needed, and
/// 0755 is refused once the owner is known to be us (the fourth reverify wave's point).
///
/// The ownership arm cannot be exercised as root — every directory on this host is ours — so
/// it is pinned in `the_launcher_refuses_a_socket_directory_it_does_not_own` by giving the
/// launcher a different `id`.
#[test]
fn a_world_readable_socket_directory_is_refused() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let home = tmp.path().join("home");
    let run_dir = home.join(".allspark-home").join("run");
    fs::create_dir_all(&run_dir).expect("run dir");
    fs::set_permissions(&run_dir, fs::Permissions::from_mode(0o755)).expect("widen");

    let launcher = deploy_launcher(tmp.path(), Path::new(env!("CARGO_BIN_EXE_prime-agent")));
    let probe = tmp.path().join("libexec").join("prime-agent");
    write_env_probe_payload_at(&probe);
    let out = Command::new(&launcher)
        .env("HOME", &home)
        .env_remove("ALLSPARK_HOME")
        .env_remove("ALLSPARK_DAEMON_SOCKET_DIR")
        .output()
        .expect("run launcher");

    assert!(
        !out.status.success(),
        "a world-readable socket dir must refuse the launch: {out:?}"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("must be a private directory"),
        "the refusal must say why: {stderr}"
    );
    assert!(
        stderr.contains("755"),
        "the refusal must name the mode it found: {stderr}"
    );
}

/// Ownership is checked, not just the mode.
///
/// A 0755 directory owned by ANOTHER user passes a mode-only check — they own it and can
/// bind the socket in it, we connect. The launcher therefore reads the owner first. Simulated
/// by putting a fake `id` on PATH: the launcher compares the directory's real uid against
/// what `id -u` reports, so a launcher that thinks it is another user must refuse its own
/// directory.
#[test]
fn the_launcher_refuses_a_socket_directory_it_does_not_own() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let home = tmp.path().join("home");
    let run_dir = home.join(".allspark-home").join("run");
    fs::create_dir_all(&run_dir).expect("run dir");
    fs::set_permissions(&run_dir, fs::Permissions::from_mode(0o700)).expect("tighten");

    let launcher = deploy_launcher(tmp.path(), Path::new(env!("CARGO_BIN_EXE_prime-agent")));
    let probe = tmp.path().join("libexec").join("prime-agent");
    write_env_probe_payload_at(&probe);
    // A stand-in for "this process is somebody else": `stat` still reports the real owner.
    let stubs = tmp.path().join("stubs");
    fs::create_dir_all(&stubs).expect("stub dir");
    let fake_id = stubs.join("id");
    fs::write(&fake_id, "#!/bin/sh\necho 4242\n").expect("fake id");
    fs::set_permissions(&fake_id, fs::Permissions::from_mode(0o755)).expect("chmod the stub");
    let path = format!(
        "{}:{}",
        stubs.display(),
        std::env::var("PATH").unwrap_or_default()
    );

    let out = Command::new(&launcher)
        .env("HOME", &home)
        .env("PATH", path)
        .env_remove("ALLSPARK_HOME")
        .env_remove("ALLSPARK_DAEMON_SOCKET_DIR")
        .output()
        .expect("run launcher");

    assert!(
        !out.status.success(),
        "a directory this process does not own must refuse the launch: {out:?}"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("belongs to uid"),
        "the refusal must name the owner: {stderr}"
    );
}

/// A socket directory somebody else widened is refused, not reused.
///
/// The launch creates the directory with mode 0700, but a directory that ALREADY exists
/// keeps whatever mode it has — and a socket dir another local user can write is the
/// impersonation window itself (bind a socket there before the daemon does, answer with
/// the public hello fields, receive the session's prompts). So the launcher checks the mode
/// it actually got and refuses, which is the only honest answer: it cannot re-mode a
/// directory it did not create without guess-then-act.
#[test]
fn a_world_writable_socket_directory_is_refused() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let home = tmp.path().join("home");
    let run_dir = home.join(".allspark-home").join("run");
    fs::create_dir_all(&run_dir).expect("run dir");
    fs::set_permissions(&run_dir, fs::Permissions::from_mode(0o777)).expect("widen the dir");

    let launcher = deploy_launcher(tmp.path(), Path::new(env!("CARGO_BIN_EXE_prime-agent")));
    // A probe payload: the refusal happens before the payload runs, and the real binary
    // would spend its one-time kernel setup proving nothing about the socket directory.
    let probe = tmp.path().join("libexec").join("prime-agent");
    write_env_probe_payload_at(&probe);
    let out = Command::new(&launcher)
        .env("HOME", &home)
        .env_remove("ALLSPARK_HOME")
        .env_remove("ALLSPARK_DAEMON_SOCKET_DIR")
        .output()
        .expect("run launcher");

    assert!(
        !out.status.success(),
        "a world-writable socket dir must refuse the launch: {out:?}"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("must be a private directory"),
        "the refusal must say why: {stderr}"
    );
}

/// A PATH whose `stat` speaks ONLY the BSD dialect and whose `uname` says Darwin.
///
/// Stock macOS ships BSD `stat`: `-f <format> <path>`, `%u` for the owner, `%Lp` for the
/// numeric mode, and no `-c` at all. The stubs model that interface (mode and owner come
/// from the real inode, so a 0700 and a 0777 directory are still told apart), which is what
/// makes a launcher's dialect branch observable on a Linux host.
///
/// Shared by the two launcher tests below — the tracked script and the one
/// `install-rust.sh` writes have drifted before, and both need the same host.
fn bsd_stat_path(dir: &Path) -> String {
    let python = [
        "/usr/bin/python3",
        "/usr/local/bin/python3",
        "/opt/homebrew/bin/python3",
    ]
    .into_iter()
    .find(|path| Path::new(path).is_file())
    .expect("python3 for the BSD stat stub");
    let stubs = dir.join("bsd-stubs");
    fs::create_dir_all(&stubs).expect("bsd stub dir");
    let stat = stubs.join("stat");
    fs::write(
        &stat,
        format!(
            r#"#!/bin/sh
if [ "$1" = "-f" ]; then
    fmt="$2"
    shift 2
else
    echo "stat: illegal option -- $1" >&2
    exit 1
fi
case "$fmt" in
    %u|%Lp) : ;;
    *) echo "stat: unsupported format $fmt" >&2; exit 1 ;;
esac
exec "{python}" -c 'import os,sys
st = os.stat(sys.argv[2])
print(st.st_uid if sys.argv[1] == "%u" else format(st.st_mode & 0o777, "o"))' "$fmt" "$1"
"#,
            python = python
        ),
    )
    .expect("bsd stat stub");
    let uname = stubs.join("uname");
    fs::write(&uname, "#!/bin/sh\necho Darwin\n").expect("darwin uname stub");
    for stub in [&stat, &uname] {
        fs::set_permissions(stub, fs::Permissions::from_mode(0o755)).expect("chmod the stub");
    }
    format!(
        "{}:{}",
        stubs.display(),
        std::env::var("PATH").unwrap_or_default()
    )
}

/// The launcher's socket-directory check must speak the `stat` its HOST has.
///
/// `stat -c %u` is GNU's spelling. Stock macOS ships BSD `stat`, which knows only `-f`
/// (`%Lp` for the mode, not GNU's `%a`), so both reads failed there — and the
/// `|| echo unknown` fallback turned that failure into a VALUE, which can never equal
/// `$(id -u)`. The launcher therefore refused EVERY socket directory, including the 0700
/// one it had just created, and the product could not start on either supported Darwin
/// architecture (review #19).
///
/// The host is modelled, not emulated: a `stat` that rejects `-c` and answers `-f` the way
/// BSD does, plus a `uname` reporting Darwin.
#[test]
fn the_launcher_reads_the_socket_directory_with_the_hosts_stat_dialect() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let home = tmp.path().join("home");
    fs::create_dir_all(&home).expect("home");
    let launcher = deploy_launcher(tmp.path(), Path::new(env!("CARGO_BIN_EXE_prime-agent")));
    let probe = tmp.path().join("libexec").join("prime-agent");
    write_env_probe_payload_at(&probe);
    let path = bsd_stat_path(tmp.path());

    let out = Command::new(&launcher)
        .env("HOME", &home)
        .env("PATH", &path)
        .env_remove("ALLSPARK_HOME")
        .env_remove("ALLSPARK_DAEMON_SOCKET_DIR")
        .output()
        .expect("run launcher under a BSD stat");
    assert!(
        out.status.success(),
        "a Darwin host must not be refused its own 0700 socket directory: {out:?}"
    );
    let socket_line = String::from_utf8_lossy(&out.stdout);
    assert!(
        socket_line.contains("SOCKET="),
        "the payload must run and report the socket: {socket_line}"
    );

    // The refusal still has to FIRE when the directory really is widened: a dialect branch
    // that silently accepted any directory would pass the assertions above.
    let widened_home = tmp.path().join("widened-home");
    let widened = widened_home.join(".allspark-home").join("run");
    fs::create_dir_all(&widened).expect("widened run dir");
    fs::set_permissions(&widened, fs::Permissions::from_mode(0o777)).expect("widen the dir");
    let out = Command::new(&launcher)
        .env("HOME", &widened_home)
        .env("PATH", &path)
        .env_remove("ALLSPARK_HOME")
        .env_remove("ALLSPARK_DAEMON_SOCKET_DIR")
        .output()
        .expect("run launcher under a BSD stat");
    assert!(
        !out.status.success(),
        "a 0777 socket directory is still refused under the BSD dialect: {out:?}"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("must be a private directory") && stderr.contains("777"),
        "the BSD mode read must produce a real mode, not `unknown`: {stderr}"
    );
}

/// The release launcher carries the same private-socket rule.
///
/// `install-rust.sh` writes its own launcher; the two had already drifted once (the
/// nesting fix landed in one). Pinned as a contract, not as text: the embedded launcher's
/// socket block, extracted and run, creates a 0700 directory and refuses a 0777 one.
#[test]
fn the_release_launcher_creates_and_checks_a_private_socket_directory() {
    let installer = fs::read_to_string(repo_root().join("install-rust.sh")).expect("installer");
    let start = installer
        .find("cat > \"$launcher_tmp\" <<EOF")
        .expect("the fork launcher heredoc");
    let end = installer[start..]
        .find("\nEOF\n")
        .map(|at| start + at)
        .expect("the heredoc ends");
    let embedded = installer[start..end]
        .split("<<EOF\n")
        .nth(1)
        .expect("the heredoc body")
        .replace("\\$", "$");
    let home_resolution = embedded
        .split("export HOME=\"$ALLSPARK_HOME\"")
        .next()
        .expect("the home resolution");
    // From the socket-directory assignment to the exec: the block under test. Keyed on the
    // ASSIGNMENT, not on a comment, because the comment is prose that may be reworded.
    let socket_block = embedded
        .split("ALLSPARK_SOCKET_DIR=")
        .nth(1)
        .expect("the socket block exists");
    let socket_block = format!(
        "ALLSPARK_SOCKET_DIR={}\n",
        socket_block
            .split("exec ")
            .next()
            .expect("ends before exec")
    );
    let parts = [
        "#!/bin/sh",
        home_resolution,
        "export HOME=\"$ALLSPARK_HOME\"",
        &socket_block,
        "printf '%s\\n' \"$PRIME_AGENT_DAEMON_SOCKET\"",
    ];
    let script = parts.join("\n");

    let run = |home: &Path, path: Option<&str>| {
        let mut command = Command::new("sh");
        command
            .arg("-c")
            .arg(&script)
            .env("HOME", home)
            .env_remove("ALLSPARK_HOME")
            .env_remove("ALLSPARK_DAEMON_SOCKET")
            .env_remove("ALLSPARK_DAEMON_SOCKET_DIR");
        if let Some(path) = path {
            command.env("PATH", path);
        }
        command
            .output()
            .expect("run the embedded socket block")
    };

    let tmp = tempfile::tempdir().expect("tempdir");
    let home = tmp.path().join("home");
    fs::create_dir_all(&home).expect("home");
    let out = run(&home, None);
    assert!(
        out.status.success(),
        "the embedded launcher runs: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let socket = String::from_utf8_lossy(&out.stdout);
    let run_dir = home.join(".allspark-home").join("run");
    assert_eq!(
        Path::new(socket.trim()),
        run_dir.join("daemon.sock"),
        "the release launcher must pin the private socket: {socket}"
    );
    assert_eq!(
        fs::metadata(&run_dir)
            .expect("run dir")
            .permissions()
            .mode()
            & 0o777,
        0o700,
        "created 0700, not merely chmodded later"
    );

    // The same block on a Darwin host: BSD `stat` knows `-f %u` / `-f %Lp` and no `-c`, so
    // a GNU-only read here refuses the launcher's OWN 0700 directory (review #19, the
    // same defect the tracked launcher carried).
    let bsd_home = tmp.path().join("bsd-home");
    fs::create_dir_all(&bsd_home).expect("bsd home");
    let bsd_path = bsd_stat_path(&tmp.path().join("bsd-host"));
    let out = run(&bsd_home, Some(&bsd_path));
    assert!(
        out.status.success(),
        "the release launcher must read a Darwin host's stat dialect: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        Path::new(String::from_utf8_lossy(&out.stdout).trim()),
        bsd_home.join(".allspark-home").join("run").join("daemon.sock"),
        "the Darwin-dialect run must pin the same private socket"
    );

    // And the refusal: a widened directory is not reused.
    let other = tempfile::tempdir().expect("tempdir");
    let widened_home = other.path().join("home");
    let widened_run = widened_home.join(".allspark-home").join("run");
    fs::create_dir_all(&widened_run).expect("run dir");
    fs::set_permissions(&widened_run, fs::Permissions::from_mode(0o777)).expect("widen");
    let out = run(&widened_home, None);
    assert!(
        !out.status.success(),
        "the release launcher must refuse a world-writable socket dir: {out:?}"
    );
}

