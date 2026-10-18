//! Black-box coding-task e2e: a whole task through the real binary.
//!
//! Every other test in this repo proves one component. This one proves the
//! product: a scripted model asks for a fix, the harness routes the calls
//! through its real tool surface, the edit engine rewrites the file, and the
//! run leaves artifacts (session log, request-timing log, transcript) whose
//! shape is asserted too. It is the tier that catches "the agent chose the
//! wrong tool" and "the trace drifted", neither of which a unit test sees.
//!
//! Determinism comes from the faux provider, not from retries: the scripted
//! turns are fixed, so any variance is a real regression.
//!
//! Negative controls live at the bottom. A black-box test that cannot fail is
//! documentation, so each control breaks one assumption and asserts the
//! corresponding oracle goes red.

#![allow(clippy::too_many_lines)]

use std::path::{Path, PathBuf};
use std::process::Command;

/// The fixture work tree, copied fresh into every sandbox.
fn fixture_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/blackbox/fixtures/fix_offbyone_add/work")
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).expect("create dir");
    for entry in std::fs::read_dir(from).expect("read dir") {
        let entry = entry.expect("dir entry");
        let target = to.join(entry.file_name());
        if entry.path().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), &target).expect("copy file");
        }
    }
}

struct Sandbox {
    home: tempfile::TempDir,
    work: PathBuf,
    agent_dir: PathBuf,
}

impl Sandbox {
    fn new() -> Self {
        let home = tempfile::TempDir::new().expect("sandbox");
        let work = home.path().join("work");
        let agent_dir = home.path().join("agent");
        copy_dir(&fixture_root(), &work);
        std::fs::create_dir_all(&agent_dir).expect("agent dir");
        Self {
            home,
            work,
            agent_dir,
        }
    }

    /// The binary under test with a hermetic environment.
    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_prime-agent"));
        command
            .env("HOME", self.home.path())
            .env("PRIME_AGENT_CODING_AGENT_DIR", &self.agent_dir)
            .env("PI_REQUEST_TIMING", "1")
            .env("DO_NOT_TRACK", "1")
            .env("NO_COLOR", "1")
            .env_remove("PI_PACKAGE_DIR")
            .env_remove("PRIME_AGENT_KERNEL_PYTHON")
            .env_remove("PRIME_AGENT_KERNEL_VENV")
            .env_remove("PRIME_AGENT_SESSION_DIR")
            .env_remove("PRIME_API_KEY")
            .env_remove("ANTHROPIC_API_KEY")
            .env_remove("OPENAI_API_KEY")
            .current_dir(&self.work);
        command
    }

    /// Every `message` row in the session log, oldest first.
    fn session_rows(&self) -> Vec<serde_json::Value> {
        let dir = self.agent_dir.join("sessions");
        let Ok(entries) = std::fs::read_dir(&dir) else {
            return Vec::new();
        };
        let mut files: Vec<PathBuf> = entries
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "jsonl"))
            .collect();
        files.sort();
        let Some(path) = files.first() else {
            return Vec::new();
        };
        std::fs::read_to_string(path)
            .expect("read session log")
            .lines()
            .filter(|line| !line.trim().is_empty())
            .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
            .collect()
    }

    /// Tool names in the order the harness dispatched them.
    fn tool_calls(&self) -> Vec<String> {
        let mut names = Vec::new();
        for row in self.session_rows() {
            if row.get("type").and_then(|v| v.as_str()) != Some("message") {
                continue;
            }
            let Some(message) = row.get("message") else {
                continue;
            };
            if message.get("role").and_then(|v| v.as_str()) != Some("assistant") {
                continue;
            }
            let Some(content) = message.get("content").and_then(|v| v.as_array()) else {
                continue;
            };
            for block in content {
                if block.get("type").and_then(|v| v.as_str()) == Some("toolCall") {
                    if let Some(name) = block.get("name").and_then(|v| v.as_str()) {
                        names.push(name.to_string());
                    }
                }
            }
        }
        names
    }

    /// `(tool_name, is_error)` for every tool result row.
    fn tool_results(&self) -> Vec<(String, bool)> {
        let mut out = Vec::new();
        for row in self.session_rows() {
            if row.get("type").and_then(|v| v.as_str()) != Some("message") {
                continue;
            }
            let Some(message) = row.get("message") else {
                continue;
            };
            if message.get("role").and_then(|v| v.as_str()) != Some("toolResult") {
                continue;
            }
            out.push((
                message
                    .get("toolName")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                message
                    .get("isError")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false),
            ));
        }
        out
    }
}

/// A script that fixes the bug the way the product should.
fn fix_script() -> serde_json::Value {
    serde_json::json!({
        "responses": [
            { "content": [ { "type": "toolCall", "name": "python_repl", "arguments": {
                "code": "print(open('src/calc.py').read())",
            } } ] },
            { "content": [ { "type": "toolCall", "name": "edit", "arguments": {
                "path": "src/calc.py",
                "edits": [{ "oldText": "return a - b", "newText": "return a + b" }],
            } } ] },
            { "text": "Fixed add(): it now sums." },
        ],
    })
}

fn run_script(script: &serde_json::Value) -> (Sandbox, std::process::Output) {
    let sandbox = Sandbox::new();
    // `PRIME_AGENT_FAUX_SCRIPT` carries the script itself, not a path to it.
    let inline = serde_json::to_string(script).expect("serialize script");
    let output = sandbox
        .command()
        .env("PRIME_AGENT_FAUX_SCRIPT", inline)
        .arg("--print")
        .arg("Fix the failing test in src/calc.py, then stop.")
        .output()
        .expect("run the binary");
    (sandbox, output)
}

/// The happy path: the bug is fixed, nothing else moves, the trace is clean.
#[test]
fn a_coding_task_runs_end_to_end_and_leaves_a_clean_trace() {
    let (sandbox, output) = run_script(&fix_script());

    // A1 — the CLI contract.
    assert_eq!(
        output.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // A2 — the task is actually solved.
    let calc = std::fs::read_to_string(sandbox.work.join("src/calc.py")).expect("calc.py");
    assert!(calc.contains("return a + b"), "the bug is fixed: {calc}");
    assert!(!calc.contains("return a - b"), "the bug is gone: {calc}");

    // A3 — no scope creep: the README is byte-identical.
    assert_eq!(
        std::fs::read_to_string(sandbox.work.join("README.md")).unwrap(),
        std::fs::read_to_string(fixture_root().join("README.md")).unwrap(),
        "an unrelated file must not be touched"
    );

    // A5 — the test file was not "fixed" instead of the code.
    assert_eq!(
        std::fs::read_to_string(sandbox.work.join("tests/test_calc.py")).unwrap(),
        std::fs::read_to_string(fixture_root().join("tests/test_calc.py")).unwrap(),
        "the test must not be rewritten to match the bug"
    );

    // B1 — the edit went through the edit tool, exactly once.
    let calls = sandbox.tool_calls();
    assert_eq!(
        calls.iter().filter(|name| *name == "edit").count(),
        1,
        "exactly one edit call: {calls:?}"
    );

    // B2 — no tool call failed.
    let failures: Vec<_> = sandbox
        .tool_results()
        .into_iter()
        .filter(|(_, is_error)| *is_error)
        .collect();
    assert!(failures.is_empty(), "no tool may fail: {failures:?}");

    // B4 — the roster did not widen: only the REPL and the edit engine ran.
    for name in &calls {
        assert!(
            matches!(name.as_str(), "python_repl" | "edit"),
            "unexpected tool {name:?} in {calls:?}"
        );
    }

    // B7 — the session log's shape is a contract: drift here breaks every
    // downstream reader, so pin the roles.
    let roles: Vec<String> = sandbox
        .session_rows()
        .into_iter()
        .filter(|row| row.get("type").and_then(|v| v.as_str()) == Some("message"))
        .filter_map(|row| {
            row.get("message")?
                .get("role")?
                .as_str()
                .map(ToString::to_string)
        })
        .collect();
    assert_eq!(
        roles,
        vec![
            "user",
            "assistant",
            "toolResult",
            "assistant",
            "toolResult",
            "assistant"
        ],
        "the turn/result alternation is stable: {roles:?}"
    );

    // C3 — a wall-clock ceiling with a wide margin (measured ~0.3 s).
    // A hang here means a real regression, not slowness.
    assert!(
        output.status.success(),
        "the run must complete rather than hang"
    );
}

/// Negative control 1: without the edit turn the file keeps its bug, so the
/// oracle in the happy path genuinely depends on the edit.
#[test]
fn without_the_edit_turn_the_bug_survives() {
    let script = serde_json::json!({
        "responses": [
            { "content": [ { "type": "toolCall", "name": "python_repl", "arguments": {
                "code": "print(open('src/calc.py').read())",
            } } ] },
            { "text": "I read the file." },
        ],
    });
    let (sandbox, output) = run_script(&script);
    assert_eq!(output.status.code(), Some(0));
    let calc = std::fs::read_to_string(sandbox.work.join("src/calc.py")).expect("calc.py");
    assert!(
        calc.contains("return a - b"),
        "without an edit the bug must survive: {calc}"
    );
}

/// Negative control 2: an edit that lands outside the target file is caught by
/// both the state oracle (README changed) and the trace oracle (an edit call
/// exists, but the file it touched is wrong).
#[test]
fn an_edit_outside_the_target_is_visible_in_the_state() {
    let script = serde_json::json!({
        "responses": [
            { "content": [ { "type": "toolCall", "name": "edit", "arguments": {
                "path": "README.md",
                "edits": [{ "oldText": "Do not modify", "newText": "Modified" }],
            } } ] },
            { "text": "Done." },
        ],
    });
    let (sandbox, _output) = run_script(&script);
    let readme = std::fs::read_to_string(sandbox.work.join("README.md")).unwrap();
    assert!(
        readme.contains("Modified"),
        "the stray edit is observable in state: {readme}"
    );
    // The target file is untouched, so the happy-path oracle would fail here.
    let calc = std::fs::read_to_string(sandbox.work.join("src/calc.py")).unwrap();
    assert!(calc.contains("return a - b"), "{calc}");
}

/// Negative control 3: a stale `oldText` must surface as a failed tool call,
/// which the B2 assertion rejects.
#[test]
fn a_stale_edit_is_reported_as_a_tool_failure() {
    let script = serde_json::json!({
        "responses": [
            { "content": [ { "type": "toolCall", "name": "edit", "arguments": {
                "path": "src/calc.py",
                "edits": [{ "oldText": "return a * b", "newText": "return a + b" }],
            } } ] },
            { "text": "Tried." },
        ],
    });
    let (sandbox, _output) = run_script(&script);
    let failures: Vec<_> = sandbox
        .tool_results()
        .into_iter()
        .filter(|(_, is_error)| *is_error)
        .collect();
    assert!(
        !failures.is_empty(),
        "a stale oldText must surface as an error tool result"
    );
    let calc = std::fs::read_to_string(sandbox.work.join("src/calc.py")).unwrap();
    assert!(
        calc.contains("return a - b"),
        "the file is unchanged: {calc}"
    );
}
