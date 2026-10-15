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
    let script = "#!/bin/sh\nprintf 'HOME=%s\\nSOCKET=%s\\n' \"$HOME\" \
        \"$PRIME_AGENT_DAEMON_SOCKET\"\n";
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

/// First run seeds upstream `settings.json`/`models.json` into the isolated
/// home and deliberately leaves `auth.json` behind (rotating refresh tokens
/// invalidate each other).
#[test]
fn launcher_seeds_settings_but_never_auth() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let upstream_home = tmp.path().join("upstream-home");
    let upstream_config = upstream_home.join(".prime").join("agent");
    fs::create_dir_all(&upstream_config).expect("upstream config dir");
    fs::write(upstream_config.join("settings.json"), "{\"a\":1}").expect("settings");
    fs::write(upstream_config.join("models.json"), "{}").expect("models");
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
    assert!(
        isolated_config.join("settings.json").exists(),
        "settings.json must be seeded"
    );
    assert!(
        isolated_config.join("models.json").exists(),
        "models.json must be seeded"
    );
    assert!(
        !isolated_config.join("auth.json").exists(),
        "auth.json must NOT be seeded (rotating refresh tokens)"
    );
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
