//! Live differential test against the prime-inference endpoint.
//!
//! Streams the captured `z-ai/glm-5.3-flash` SSE exchange through a local replay server, runs the
//! TS reference provider and this Rust port against it, and requires identical text and usage.
//! Ignored by default; run:
//!
//! ```sh
//! tests/differential/run.sh
//! ```

use serde_json::{json, Map, Value};

use crate::providers::openai_completions::{stream_openai_completions, OpenAICompletionsOptions};
use crate::types::{AssistantContent, Model, StopReason, StreamOptions};

/// Mirrors the prime-inference catalog entry for `z-ai/glm-5.3-flash`.
fn glm_53_flash(base_url: &str) -> Model {
    let compat_json = json!({
        "supportsStore": false,
        "supportsDeveloperRole": false,
        "supportsReasoningEffort": true,
        "maxTokensField": "max_tokens",
        "supportsStrictMode": false,
    });
    Model {
        id: "z-ai/glm-5.3-flash".into(),
        name: "GLM 5.3 Flash".into(),
        api: "openai-completions".into(),
        provider: "prime-inference".into(),
        base_url: base_url.into(),
        reasoning: true,
        thinking_level_map: None,
        input: vec![
            crate::types::ModelInput::Text,
            crate::types::ModelInput::Image,
        ],
        cost: crate::types::ModelCost {
            input: 0.15.into(),
            output: 0.5.into(),
            cache_read: 0.0.into(),
            cache_write: 0.0.into(),
        },
        context_window: 1_310_720,
        max_tokens: 131_072,
        max_tokens_explicit: false,
        featured: None,
        headers: None,
        compat: Some(crate::types::ModelCompat::from_kind(
            crate::types::CompatKind::OpenAiCompletions(Box::new(
                serde_json::from_value(compat_json).expect("compat parses"),
            )),
        )),
        serving: None,
    }
}

// `context` and `content`: the local mirrors the `content` JSON field it builds; a rename would churn the differential harness.
#[allow(clippy::similar_names)]
async fn normalized_result(model: &Model) -> Value {
    let context = crate::types::Context {
        system_prompt: Some("You are a helpful assistant. Be concise.".into()),
        system_prompt_cache_prefix: None,
        messages: vec![crate::types::Message::User(crate::types::UserMessage {
            content: crate::types::UserMessageContent::Text(
                "In one short sentence: what is the capital of France, and what is 7 times 8?"
                    .into(),
            ),
            timestamp: 1_789_529_142_000,
            rest: Map::default(),
        })],
        tools: None,
    };
    let options = OpenAICompletionsOptions::from_base(StreamOptions {
        api_key: Some("replay".into()),
        ..Default::default()
    });
    let stream = stream_openai_completions(model, &context, Some(&options));
    let result = stream.result().await;
    let content: Vec<Value> = result
        .content
        .iter()
        .map(|block| match block {
            AssistantContent::Thinking(thinking) => {
                // Match the TS driver normalization: `redacted` is omitted when
                // unset (undefined fields are dropped by JSON.stringify).
                let mut value = json!({
                    "type": "thinking",
                    "thinking": thinking.thinking,
                });
                if let Some(redacted) = thinking.redacted {
                    value["redacted"] = json!(redacted);
                }
                value
            }
            AssistantContent::Text(text) => json!({
                "type": "text",
                "text": text.text,
            }),
            AssistantContent::ToolCall(tool_call) => json!({
                "type": "toolCall",
                "id": tool_call.id,
                "name": tool_call.name,
                "arguments": tool_call.arguments,
            }),
        })
        .collect();
    json!({
        "content": content,
        "usage": {
            "input": result.usage.input,
            "output": result.usage.output,
            "cacheRead": result.usage.cache_read,
            "cacheWrite": result.usage.cache_write,
            "totalTokens": result.usage.total_tokens,
            "cost": {
                "input": result.usage.cost.input.as_f64(),
                "output": result.usage.cost.output.as_f64(),
                "cacheRead": result.usage.cost.cache_read.as_f64(),
                "cacheWrite": result.usage.cost.cache_write.as_f64(),
                "total": result.usage.cost.total.as_f64(),
            },
        },
        "stopReason": result.stop_reason,
        "responseModel": result.response_model,
        "responseId": result.response_id,
        "errorMessage": result.error_message,
    })
}

#[tokio::test]
#[ignore = "needs the SSE replay server and the TS baseline; run tests/differential/run.sh"]
async fn differential_prime_inference_glm_53_flash() {
    let replay_base_url = std::env::var("PA_DIFF_REPLAY_URL")
        .expect("PA_DIFF_REPLAY_URL must point at the replay server base URL");
    let expected_path = std::env::var("PA_DIFF_EXPECTED")
        .expect("PA_DIFF_EXPECTED must point at the TS baseline JSON file");
    let expected_text = std::fs::read_to_string(&expected_path).expect("read TS baseline");
    let expected: Value = serde_json::from_str(&expected_text).expect("parse TS baseline");

    let model = glm_53_flash(&replay_base_url);
    let actual = normalized_result(&model).await;

    assert_eq!(
        actual["content"], expected["content"],
        "assistant content differs from the TS reference"
    );
    assert_eq!(
        actual["stopReason"], expected["stopReason"],
        "stop reason differs from the TS reference"
    );
    assert_eq!(
        actual["responseId"], expected["responseId"],
        "response id differs from the TS reference"
    );
    assert_eq!(
        actual["responseModel"], expected["responseModel"],
        "response model differs from the TS reference"
    );
    assert_eq!(
        actual["errorMessage"], expected["errorMessage"],
        "error message differs from the TS reference"
    );

    // Usage accounting: token counts exact, costs within float epsilon.
    for field in ["input", "output", "cacheRead", "cacheWrite", "totalTokens"] {
        assert_eq!(
            actual["usage"][field], expected["usage"][field],
            "usage.{field} differs from the TS reference"
        );
    }
    for field in ["input", "output", "cacheRead", "cacheWrite", "total"] {
        let left = actual["usage"]["cost"][field].as_f64().unwrap_or(0.0);
        let right = expected["usage"]["cost"][field].as_f64().unwrap_or(0.0);
        assert!(
            (left - right).abs() < 1e-9,
            "usage.cost.{field} differs from the TS reference: {left} vs {right}"
        );
    }

    assert_eq!(expected["stopReason"], json!("stop"));
    let _ = StopReason::Stop;
}

#[cfg(test)]
mod self_test_tests {
    //! The isolation rule, exercised from Rust so the ordinary `cargo test` lane sees it.
    //!
    //! `tests/differential/run.sh` is a shell script with no runner outside an operator's
    //! hands, and the two defects it had (a shared baseline path, one fixed port) were races
    //! between two CONCURRENT runs — invisible to a single serial run. `PA_DIFF_SELFTEST=1`
    //! checks both on any machine; these tests check the same two properties from the lane
    //! that actually runs, so a regression cannot hide behind "nobody runs that script".

    use std::path::PathBuf;
    use std::process::{Command, Stdio};

    fn repo_root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(2)
            .expect("repo root")
            .to_path_buf()
    }

    /// Two servers started at once must report DIFFERENT ports, and each must print the
    /// port it actually bound (port 0 means the caller cannot know it any other way).
    #[test]
    fn two_replay_servers_bind_different_ports_and_report_them() {
        let script = repo_root().join("crates/pa-ai/tests/differential/replay_server.py");
        let spawn = || {
            Command::new("python3")
                .arg(&script)
                .arg("0")
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
                .expect("spawn the replay server")
        };
        let first = spawn();
        let second = spawn();

        let read_port = |mut child: std::process::Child| {
            use std::io::{BufRead, BufReader};
            let stdout = child.stdout.take().expect("stdout is piped");
            let line = BufReader::new(stdout)
                .lines()
                .next()
                .expect("the server prints its port")
                .expect("the line reads");
            let _ = child.kill();
            let _ = child.wait();
            line.strip_prefix("REPLAY_PORT=")
                .expect("the port line is the contract the runner parses")
                .parse::<u16>()
                .expect("a port number")
        };
        let port_a = read_port(first);
        let port_b = read_port(second);

        assert_ne!(
            port_a, port_b,
            "two concurrent runs must not share a replay port"
        );
        assert_ne!(port_a, 0, "the kernel's own port is reported, not 0");
    }

    /// The runner must give each run its own scratch directory, its own baseline, and a
    /// port the SERVER reports — and each of those must be pinned by the line that does it.
    ///
    /// Measured, and the reason the pins are shaped this way: the first version asserted
    /// `script.contains("REPLAY_PORT=")`, which the `sed` parser satisfies on its own even
    /// when `PORT_REQUEST` is a fixed 18097 — the natural regression went through. Likewise
    /// `PA_DIFF_SELFTEST` appears in the usage comment, so disabling the selftest passed.
    /// Each assertion now names the ASSIGNMENT or the branch, not a substring that can
    /// live in prose:
    ///   * `PORT_REQUEST="${PA_DIFF_PORT:-0}"` — port 0, with the override still an
    ///     override. A fixed default changes the brace group, so it fails;
    ///   * `TS_BASELINE="$RUN_DIR/ts.json"` — the baseline lives under the per-run dir;
    ///   * `RUN_DIR="$(mktemp -d …)"` — the dir is created per run and removed by the trap;
    ///   * the selftest condition is `if [ "${PA_DIFF_SELFTEST:-}" = "1" ]`, not a mention.
    #[test]
    fn the_runner_isolates_its_baseline_and_is_self_testable() {
        let script =
            std::fs::read_to_string(repo_root().join("crates/pa-ai/tests/differential/run.sh"))
                .expect("the differential runner");
        for (needle, why) in [
            (
                "PORT_REQUEST=\"${PA_DIFF_PORT:-0}\"",
                "port 0 lets the kernel choose; a fixed default is the race the finding is about",
            ),
            (
                "TS_BASELINE=\"$RUN_DIR/ts.json\"",
                "the baseline must live under the per-run scratch directory",
            ),
            (
                "RUN_DIR=\"$(mktemp -d",
                "each run needs its own scratch directory, created fresh",
            ),
            (
                "if [ \"${PA_DIFF_SELFTEST:-}\" = \"1\" ]; then",
                "the isolation selftest must be reachable AND guarded as a branch, not mentioned",
            ),
        ] {
            assert!(script.contains(needle), "{why}: missing {needle}");
        }
        assert!(
            !script.contains("/tmp/pa_diff_ts.json"),
            "a shared baseline path is the race the finding is about"
        );
        // The assignment is only load-bearing if the server is STARTED with it: declaring
        // `PORT_REQUEST` and then passing a literal leaves the slot free and every assertion
        // above green (measured).
        assert!(
            script.contains("replay_server.py\" \"$PORT_REQUEST\""),
            "the server must be started with the resolved port request"
        );
    }
}
