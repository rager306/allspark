//! Fork tools e2e: the REAL fork binary drives a REAL session turn that
//! executes the fork's native tools (lsp) against a real file. Deterministic
//! (the model is the scripted faux provider; the tool execution is real).
//!
//! This is the chain that matters: create_session registers the tool, the
//! scripted turn invokes it, the tool parses a real file, and the result
//! reaches the transcript.
//!
//! Run: cargo test -p pa-cli --test fork_tools_e2e

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
fn run_text_mode(home: &Path, cwd: &Path, script: &str, prompt: &str) -> (i32, String, String) {
    let script_path = home.join("faux-script.json");
    fs::write(&script_path, script).expect("write faux script");
    let output = Command::new(payload_binary())
        .arg("--mode").arg("text")
        .arg("--no-session")
        .arg(prompt)
        .env("HOME", home)
        .env("ALLSPARK_FAUX_SCRIPT", &script_path)
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

    let (code, stdout, stderr) =
        run_text_mode(&home, &cwd, MUSE_DEVICE_SCRIPT, "покажи символы demo.rs");
    assert_eq!(code, 0, "exit {code}: stderr={stderr}");
    // The tool result must carry the real symbol names from demo.rs.
    for symbol in ["Shape", "new", "scale", "main"] {
        assert!(
            stdout.contains(symbol) || stderr.contains(symbol),
            "symbol {symbol} must appear in the output: stdout={stdout}"
        );
    }
}
