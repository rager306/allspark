//! The `allspark` launcher e2e: the tracked launcher script
//! (`scripts/allspark-launcher.sh`) is the fork's user-facing entry point, so
//! its contract is verified against the real compiled payload, not a mock:
//!
//! 1. substitution — `HOME` moves to `<upstream HOME>/.allspark-home`
//!    (`ALLSPARK_HOME` wins when set) and the daemon socket is pinned to the
//!    fork's own `/tmp/allspark-daemon.sock`;
//! 2. seeding — first run copies upstream `settings.json`/`models.json` into
//!    the isolated home (`auth.json` deliberately not copied);
//! 3. the real fork binary answers `--version` with the fork-branded line.

#![cfg(unix)]
#![allow(clippy::type_complexity)]

use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;

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
    assert!(
        socket_line.ends_with("allspark-daemon.sock"),
        "the daemon socket must pin to the fork's own file: {socket_line}"
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
