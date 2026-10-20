//! Fork tools e2e: the REAL fork binary drives a REAL session turn that
//! executes the fork's native tools (lsp) against a real file. Deterministic
//! (the model is the scripted faux provider; the tool execution is real).
//!
//! This is the chain that matters: `create_session` registers the tool, the
//! scripted turn invokes it, the tool parses a real file, and the result
//! reaches the transcript.
//!
//! Run: cargo test -p pa-cli --test `fork_tools_e2e`

#![cfg(unix)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("repo root")
        .to_path_buf()
}

fn payload_binary() -> PathBuf {
    // The dist payload if deployed, else the debug binary.
    let dist = repo_root().join("target/dist/prime-agent");
    if dist.exists() {
        return dist;
    }
    repo_root().join("target/debug/prime-agent")
}

/// Spawn the fork binary in text mode with a faux-scripted lsp tool call.
fn run_text_mode(
    home: &Path,
    cwd: &Path,
    script: &serde_json::Value,
    prompt: &str,
) -> (i32, String, String) {
    // The faux seam carries the script inline, not a path to it.
    let inline = serde_json::to_string(script).expect("serialize script");
    let output = Command::new(payload_binary())
        .arg("--mode")
        .arg("text")
        .arg("--session-dir")
        .arg(home.join("sessions"))
        .arg(prompt)
        .env("HOME", home)
        .env("PRIME_AGENT_AGENT_DIR", home.join(".prime-agent-home"))
        .env("PRIME_AGENT_CODING_AGENT_DIR", home.join("agent"))
        .env("PRIME_AGENT_FAUX_SCRIPT", inline)
        .env_remove("ALLSPARK_DEBUG")
        .current_dir(cwd)
        .output()
        .expect("spawn fork binary");
    (
        output.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&output.stdout).to_string(),
        String::from_utf8_lossy(&output.stderr).to_string(),
    )
}

const DEMO_RS: &str = r#"pub struct Shape { pub area: f64 }

impl Shape {
    pub fn new(area: f64) -> Self { Self { area } }
    pub fn scale(&self, k: f64) -> f64 { self.area * k }
}

pub fn main() { let s = Shape::new(2.0); println!("{}", s.scale(3.0)); }
"#;

const MUSE_DEVICE_SCRIPT: &str = r#"{"responses": [
  {"content": [{"type": "toolCall", "name": "lsp",
                "arguments": {"action": "symbols", "file": "demo.rs"}}]},
  {"text": "Symbols listed."}
]}"#;

#[test]
fn lsp_tool_executes_through_the_fork_binary() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let home = tmp.path().join("home");
    let cwd = tmp.path().join("work");
    fs::create_dir_all(&home).expect("home dir");
    fs::create_dir_all(&cwd).expect("cwd dir");
    fs::write(cwd.join("demo.rs"), DEMO_RS).expect("write demo.rs");

    let script: serde_json::Value = serde_json::from_str(MUSE_DEVICE_SCRIPT).expect("script json");
    let (code, stdout, stderr) = run_text_mode(&home, &cwd, &script, "покажи символы demo.rs");
    assert_eq!(code, 0, "exit {code}: stderr={stderr}");
    // The printed answer is the scripted final text; the lsp result itself
    // lives in the session transcript under HOME/sessions.
    assert!(
        stdout.contains("Symbols listed."),
        "the scripted answer must print: stdout={stdout:?} stderr={stderr:?}"
    );
    // The chain this test exists for: the lsp tool actually executed against
    // demo.rs and returned the real symbol names without an error.
    let mut found: Option<Vec<String>> = None;
    for entry in fs::read_dir(home.join("sessions"))
        .expect("sessions dir")
        .flatten()
    {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
            continue;
        }
        let text = fs::read_to_string(&path).expect("read session file");
        for line in text.lines() {
            let Ok(row) = serde_json::from_str::<serde_json::Value>(line) else {
                continue;
            };
            let Some(message) = row.get("message") else {
                continue;
            };
            if message.get("role").and_then(|v| v.as_str()) != Some("toolResult") {
                continue;
            }
            if message.get("toolName").and_then(|v| v.as_str()) != Some("lsp") {
                continue;
            }
            if message.get("isError").and_then(serde_json::Value::as_bool) == Some(true) {
                continue;
            }
            let text = message
                .get("content")
                .and_then(|v| v.as_array())
                .map(|blocks| {
                    blocks
                        .iter()
                        .filter_map(|b| b.get("text").and_then(|v| v.as_str()))
                        .collect::<Vec<_>>()
                        .join("\n")
                })
                .unwrap_or_default();
            let symbols = ["Shape", "new", "scale", "main"]
                .iter()
                .filter(|s| text.contains(*s))
                .map(std::string::ToString::to_string)
                .collect::<Vec<_>>();
            if symbols.len() == 4 {
                found = Some(symbols);
            }
        }
    }
    assert!(
        found.is_some(),
        "the lsp tool result must carry all demo.rs symbols: stdout={stdout:?} stderr={stderr:?}"
    );
}
