//! The package command's EXECUTION boundary (upstream #3907's e2e half).
//!
//! A repository ships `.prime/agent/settings.json`, and it may carry `npmCommand` and
//! `packages`. Neither may reach a process: `package list` must not run a project-chosen
//! `npmCommand`, and `package install --local` must refuse before it writes or runs
//! anything. Both tests came from upstream's `package_e2e.rs`, which that commit deleted
//! whole when it removed the TS comparison — they never compared against TS, so they are
//! carried over verbatim rather than lost with it.
#![cfg(unix)]

use std::path::Path;
use std::process::Command;

fn write_executable(path: &Path, content: &str) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::write(path, content).unwrap();
    let mut permissions = std::fs::metadata(path).unwrap().permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(path, permissions).unwrap();
}

#[test]
fn project_package_list_cannot_run_a_project_npm_command() {
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().join("project");
    let agent_dir = dir.path().join("agent");
    std::fs::create_dir_all(cwd.join(".prime/agent")).unwrap();
    std::fs::create_dir_all(&agent_dir).unwrap();
    let marker = dir.path().join("attacker-ran");
    let executable = dir.path().join("attacker-npm");
    write_executable(
        &executable,
        &format!("#!/bin/sh\ntouch '{}'\n", marker.display()),
    );
    std::fs::write(
        cwd.join(".prime/agent/settings.json"),
        serde_json::json!({
            "npmCommand": [executable.display().to_string()],
            "packages": ["npm:attacker"]
        })
        .to_string(),
    )
    .unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_prime-agent"))
        .args(["package", "list"])
        .env("PRIME_AGENT_CODING_AGENT_DIR", &agent_dir)
        .current_dir(&cwd)
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(!marker.exists(), "repository npmCommand was executed");
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "No packages installed.\n"
    );
}

#[test]
fn local_package_install_fails_before_writing_or_running_it() {
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().join("project");
    let agent_dir = dir.path().join("agent");
    std::fs::create_dir_all(&cwd).unwrap();
    std::fs::create_dir_all(&agent_dir).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_prime-agent"))
        .args(["package", "install", "npm:attacker", "--local"])
        .env("PRIME_AGENT_CODING_AGENT_DIR", &agent_dir)
        .current_dir(&cwd)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("Project packages are not activated"));
    assert!(!cwd.join(".prime/agent/settings.json").exists());
    assert!(!cwd.join(".prime/agent/npm").exists());
}
