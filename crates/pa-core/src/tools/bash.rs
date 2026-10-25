//! The `bash` tool: shell command execution with output truncation and a destructive-git dirty-tree
//! guard (execution, guard, truncation, and formatting match the TS tool).

use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::json;

use crate::tools::bash_guard::{
    find_destructive_git_discard_commands, format_dirty_tree_refusal, is_truthy_env_value,
    resolve_discard_probe_target, DiscardProbeResolution, BASH_DESTRUCTIVE_GIT_BYPASS_ENV,
};
use crate::tools::output_accumulator::{OutputAccumulator, OutputAccumulatorOptions};
use crate::tools::shell_utils::get_shell_env;
use crate::tools::tool_definition::{
    AbortSignal, OnUpdate, ToolContentBlock, ToolDefinition, ToolExecutionResult, ToolUpdate,
};
use crate::tools::truncate::{
    format_size, TruncatedBy, TruncationResult, DEFAULT_MAX_BYTES, DEFAULT_MAX_LINES,
};

/// The default porcelain status command the guard probes with.
const GIT_STATUS_PORCELAIN_COMMAND: &str = "git status --porcelain --untracked-files=all";

/// Throttle for streamed output updates (TS: `BASH_UPDATE_THROTTLE_MS`).
const BASH_UPDATE_THROTTLE_MS: Duration = Duration::from_millis(100);

pub use crate::tools::bash_local::LocalBashOperations;

pub(crate) type ExecFuture<'a> =
    Pin<Box<dyn Future<Output = anyhow::Result<Option<i32>>> + Send + 'a>>;

/// Execution options for [`BashOperations::exec`].
pub struct ExecOptions<'a> {
    /// Receives streamed stdout/stderr chunks.
    pub on_data: &'a (dyn Fn(&[u8]) + Send + Sync),
    /// Cancellation handle; kills the process tree when fired.
    pub signal: Option<AbortSignal>,
    /// Timeout in seconds.
    pub timeout: Option<f64>,
    /// Environment for the child; defaults to the agent shell env.
    pub env: Option<HashMap<String, String>>,
}

/// Pluggable operations for the bash tool. Implementations stream merged stdout/stderr through
/// `on_data`, kill the process tree on cancellation, resolve with the child exit code (128 + the
/// signal number when killed by a signal, like TS `waitForChildProcess`), and reject with
/// `"aborted"` / `"timeout:<seconds>"`. Object-safe on purpose (`&dyn` injection).
pub trait BashOperations: Send + Sync {
    /// Execute a command and stream output through `on_data`; resolves with the exit code (128 +
    /// the signal number when killed by a signal), or an error: `"aborted"` or `"timeout:<seconds>"`.
    fn exec<'a>(
        &'a self,
        command: &'a str,
        cwd: &'a str,
        options: ExecOptions<'a>,
    ) -> ExecFuture<'a>;
}

/// Context for the spawn hook: command, cwd, env.
pub struct BashSpawnContext {
    pub command: String,
    pub cwd: String,
    pub env: HashMap<String, String>,
}

pub type BashSpawnHook = Arc<dyn Fn(BashSpawnContext) -> BashSpawnContext + Send + Sync>;

fn resolve_spawn_context(
    command: &str,
    cwd: &str,
    spawn_hook: Option<&BashSpawnHook>,
) -> BashSpawnContext {
    let base = BashSpawnContext {
        command: command.to_string(),
        cwd: cwd.to_string(),
        env: get_shell_env(),
    };
    match spawn_hook {
        Some(hook) => hook(base),
        None => base,
    }
}

/// Options for the bash tool.
#[derive(Default)]
pub struct BashToolOptions {
    /// Custom operations for command execution. Default: local shell.
    pub operations: Option<Arc<dyn BashOperations>>,
    /// Command prefix prepended to every command.
    pub command_prefix: Option<String>,
    /// Optional explicit shell path from settings.
    pub shell_path: Option<String>,
    /// Hook to adjust command, cwd, or env before execution.
    pub spawn_hook: Option<BashSpawnHook>,
    /// FORK-ONLY (jev Phase 2): the advisory tool-selection arbiter. When
    /// armed and a rule fires, the result carries a `[tool-arbiter]` note
    /// pointing at the specialized tool. The command runs unchanged and its
    /// exit status is untouched - the note is appended, never substituted.
    pub arbiter: Option<Arc<crate::jev::ToolArbiter>>,
}

/// Probe for at-risk files via `git status --porcelain`. Returns `Ok(None)`
/// when dirtiness cannot be determined, so the guard fails open.
async fn probe_uncommitted_changes(
    ops: &dyn BashOperations,
    probe_command: &str,
    cwd: &str,
    env: &HashMap<String, String>,
    signal: Option<AbortSignal>,
    timeout: Option<f64>,
) -> anyhow::Result<Option<Vec<String>>> {
    let output = std::sync::Mutex::new(String::new());
    let result = ops
        .exec(
            probe_command,
            cwd,
            ExecOptions {
                on_data: &|data: &[u8]| {
                    output
                        .lock()
                        .unwrap()
                        .push_str(&String::from_utf8_lossy(data));
                },
                signal,
                timeout,
                env: Some(env.clone()),
            },
        )
        .await;
    match result {
        Ok(Some(0)) => {}
        Err(err) if err.to_string() == "aborted" => return Err(err),
        Ok(_) | Err(_) => return Ok(None),
    }
    Ok(Some(
        output
            .into_inner()
            .expect("probe output unlocked")
            .split('\n')
            .filter(|line| !line.trim().is_empty())
            .map(|line| line.trim_end_matches('\r').to_string())
            .collect(),
    ))
}

/// Serialize a truncation result with the TS wire shape.
pub fn truncation_to_json(truncation: &TruncationResult) -> serde_json::Value {
    json!({
        "content": truncation.content,
        "truncated": truncation.truncated,
        "truncatedBy": match truncation.truncated_by {
            Some(TruncatedBy::Lines) => json!("lines"),
            Some(TruncatedBy::Bytes) => json!("bytes"),
            None => serde_json::Value::Null,
        },
        "totalLines": truncation.total_lines,
        "totalBytes": truncation.total_bytes,
        "outputLines": truncation.output_lines,
        "outputBytes": truncation.output_bytes,
        "lastLinePartial": truncation.last_line_partial,
        "firstLineExceedsLimit": truncation.first_line_exceeds_limit,
        "maxLines": truncation.max_lines,
        "maxBytes": truncation.max_bytes,
    })
}

struct FormattedOutput {
    text: String,
    details: Option<serde_json::Value>,
}

fn format_output(
    snapshot: &crate::tools::output_accumulator::OutputSnapshot,
    last_line_bytes: usize,
    empty_text: &str,
) -> FormattedOutput {
    let truncation = &snapshot.truncation;
    let mut text = if snapshot.content.is_empty() {
        empty_text.to_string()
    } else {
        snapshot.content.clone()
    };
    let mut details = None;
    if truncation.truncated {
        let mut d = json!({ "truncation": truncation_to_json(truncation) });
        if let Some(path) = &snapshot.full_output_path {
            d["fullOutputPath"] = json!(path);
        }
        details = Some(d);
        let start_line = truncation.total_lines - truncation.output_lines + 1;
        let end_line = truncation.total_lines;
        // A degraded spill has no path; never advertise a missing file.
        let location = snapshot
            .full_output_path
            .as_deref()
            .map(|p| format!(". Full output: {p}"))
            .unwrap_or_default();
        if truncation.last_line_partial {
            let line_size = if last_line_bytes > 0 {
                format!(" (line is {})", format_size(last_line_bytes))
            } else {
                String::new()
            };
            let _ = write!(
                text,
                "\n\n[Showing last {} of line {start_line}{line_size}{location}]",
                format_size(truncation.output_bytes)
            );
        } else if truncation.truncated_by == Some(TruncatedBy::Lines) {
            let _ = write!(
                text,
                "\n\n[Showing lines {start_line}-{end_line} of {}{location}]",
                truncation.total_lines
            );
        } else {
            let _ = write!(
                text,
                "\n\n[Showing lines {start_line}-{end_line} of {} ({} limit){location}]",
                truncation.total_lines,
                format_size(DEFAULT_MAX_BYTES)
            );
        }
    }
    FormattedOutput { text, details }
}

fn append_status(text: &str, status: &str) -> String {
    if text.is_empty() {
        status.to_string()
    } else {
        format!("{text}\n\n{status}")
    }
}

/// Execute one bash tool call. Errors carry the model-facing text.
#[tracing::instrument(
    level = "debug",
    name = "tool_bash_execute",
    skip(cwd, options, on_update)
    fields(command),
)]
pub async fn execute_bash(
    cwd: &str,
    options: &BashToolOptions,
    command: &str,
    timeout: Option<f64>,
    allow_destructive_git: Option<bool>,
    signal: Option<AbortSignal>,
    on_update: Option<OnUpdate>,
) -> anyhow::Result<ToolExecutionResult> {
    let default_ops;
    let ops: &dyn BashOperations = if let Some(ops) = &options.operations {
        ops.as_ref()
    } else {
        default_ops = LocalBashOperations {
            shell_path: options.shell_path.clone(),
        };
        &default_ops
    };
    let command_prefix = options.command_prefix.as_deref();
    let spawn_hook = options.spawn_hook.as_ref();

    let resolved_command = match command_prefix {
        Some(prefix) => format!("{prefix}\n{command}"),
        None => command.to_string(),
    };
    let spawn_context = resolve_spawn_context(&resolved_command, cwd, spawn_hook);

    // Refuse destructive git discard commands while the tree is dirty.
    let discard_indices = find_destructive_git_discard_commands(&resolved_command);
    if !discard_indices.is_empty()
        && allow_destructive_git != Some(true)
        && !is_truthy_env_value(spawn_context.env.get(BASH_DESTRUCTIVE_GIT_BYPASS_ENV))
    {
        let mut probes: Vec<(BashSpawnContext, bool)> = Vec::new();
        let mut seen_probes: HashSet<String> = HashSet::new();
        let user_command_start = command_prefix.map_or(0, |p| p.len() + 1);
        for index in discard_indices {
            let target = resolve_discard_probe_target(&resolved_command, index, user_command_start);
            match target {
                DiscardProbeResolution::Unresolvable => {
                    anyhow::bail!("{RELOCATION_REFUSAL}");
                }
                DiscardProbeResolution::NotRelocated => {
                    let raw_probe = match command_prefix {
                        Some(prefix) => format!("{prefix}\n{GIT_STATUS_PORCELAIN_COMMAND}"),
                        None => GIT_STATUS_PORCELAIN_COMMAND.to_string(),
                    };
                    let context = resolve_spawn_context(&raw_probe, cwd, spawn_hook);
                    let key = format!("{}\u{0}{}", context.command, context.cwd);
                    if seen_probes.insert(key) {
                        probes.push((context, false));
                    }
                }
                DiscardProbeResolution::Target(target) => {
                    let relocation_prefix = target.relocation_prefix.as_deref().unwrap_or("");
                    let raw_probe = match command_prefix {
                        Some(prefix) => {
                            format!("{prefix}\n{relocation_prefix}{}", target.git_status_command)
                        }
                        None => format!("{relocation_prefix}{}", target.git_status_command),
                    };
                    let context = resolve_spawn_context(&raw_probe, cwd, spawn_hook);
                    let key = format!("{}\u{0}{}", context.command, context.cwd);
                    if seen_probes.insert(key) {
                        probes.push((
                            context,
                            target.git_status_command.contains("--ignored=matching"),
                        ));
                    }
                }
            }
        }
        for (context, includes_ignored) in probes {
            let dirty_paths = probe_uncommitted_changes(
                ops,
                &context.command,
                &context.cwd,
                &context.env,
                signal.clone(),
                timeout,
            )
            .await?;
            if let Some(paths) = dirty_paths.filter(|paths| !paths.is_empty()) {
                anyhow::bail!("{}", format_dirty_tree_refusal(&paths, includes_ignored));
            }
        }
    }

    let acc = Arc::new(Mutex::new(OutputAccumulator::new(
        &OutputAccumulatorOptions {
            temp_file_prefix: "pi-bash".to_string(),
            ..OutputAccumulatorOptions::default()
        },
    )));
    let notify = Arc::new(tokio::sync::Notify::new());
    let dirty = Arc::new(AtomicBool::new(false));
    let mut last_update_at: Option<std::time::Instant> = None;

    if let Some(on_update) = &on_update {
        on_update(ToolUpdate {
            content: Vec::new(),
            details: None,
        });
    }

    let on_data = {
        let notify = notify.clone();
        let dirty = dirty.clone();
        let acc = acc.clone();
        move |data: &[u8]| {
            acc.lock().unwrap().append(data);
            dirty.store(true, Ordering::SeqCst);
            notify.notify_one();
        }
    };

    let exec_fut = ops.exec(
        &spawn_context.command,
        &spawn_context.cwd,
        ExecOptions {
            on_data: &on_data,
            signal: signal.clone(),
            timeout,
            env: Some(spawn_context.env.clone()),
        },
    );
    tokio::pin!(exec_fut);

    let exec_result: anyhow::Result<Option<i32>> = loop {
        let deadline = if dirty.load(Ordering::SeqCst) {
            last_update_at.map(|last| {
                let elapsed = last.elapsed();
                if elapsed >= BASH_UPDATE_THROTTLE_MS {
                    std::time::Instant::now()
                } else {
                    last + BASH_UPDATE_THROTTLE_MS
                }
            })
        } else {
            None
        };
        tokio::select! {
            () = notify.notified() => {}
            () = async {
                match deadline {
                    Some(deadline) => tokio::time::sleep_until(tokio::time::Instant::from_std(deadline)).await,
                    None => std::future::pending::<()>().await,
                }
            }, if deadline.is_some() => {}
            result = &mut exec_fut => {
                break result;
            }
        }
        if dirty.swap(false, Ordering::SeqCst) {
            last_update_at = Some(std::time::Instant::now());
            if let Some(on_update) = &on_update {
                let snapshot = acc.lock().unwrap().snapshot();
                on_update(ToolUpdate {
                    content: vec![ToolContentBlock::text(snapshot.content.clone())],
                    details: Some(json!({
                        "truncation": if snapshot.truncation.truncated {
                            truncation_to_json(&snapshot.truncation)
                        } else {
                            serde_json::Value::Null
                        },
                        "fullOutputPath": snapshot.full_output_path,
                    })),
                });
            }
        }
    };

    {
        let mut acc = acc.lock().unwrap();
        acc.finish();
    }
    let (snapshot, last_line_bytes) = {
        let mut acc = acc.lock().unwrap();
        acc.close_temp_file_sync();
        let snapshot = acc.snapshot();
        (snapshot, acc.get_last_line_bytes())
    };

    match exec_result {
        Err(err) => {
            // TS catch path: timeouts and aborts keep the partial output and
            // append a status line; other errors surface raw.
            let message = err.to_string();
            if message == "aborted" {
                let formatted = format_output(&snapshot, last_line_bytes, "");
                anyhow::bail!(append_status(&formatted.text, "Command aborted"));
            }
            if let Some(secs) = message.strip_prefix("timeout:") {
                let formatted = format_output(&snapshot, last_line_bytes, "");
                anyhow::bail!(append_status(
                    &formatted.text,
                    &format!("Command timed out after {secs} seconds")
                ));
            }
            Err(err)
        }
        Ok(exit_code) => {
            let formatted = format_output(&snapshot, last_line_bytes, "(no output)");
            let text = formatted.text;
            // `None` means the child was killed by a signal (not a clean
            // exit): that is a failure, not a success. Treating it as success
            // reported a signalled run as a passing command.
            match exit_code {
                Some(0) => {}
                Some(code) => anyhow::bail!(append_status(
                    &text,
                    &format!("Command exited with code {code}")
                )),
                None => anyhow::bail!(append_status(&text, "Command was killed by a signal")),
            }
            // FORK-ONLY (jev Phase 2): the advisory arbiter note rides the
            // successful result. Fail-open: no arbiter, no matching rule, or
            // a judge failure leaves the text and details byte-identical.
            let mut text = text;
            let mut details = formatted.details;
            if let Some(arbiter) = &options.arbiter {
                if let Some(note) = arbiter.note(command).await {
                    let payload = note.to_json();
                    if let Some(details) =
                        details.as_mut().and_then(serde_json::Value::as_object_mut)
                    {
                        details.insert("toolArbiter".to_string(), payload);
                    } else {
                        details = Some(json!({ "toolArbiter": payload }));
                    }
                    text = append_status(&text, &note.text);
                }
            }
            Ok(ToolExecutionResult {
                content: vec![ToolContentBlock::text(text)],
                details,
                is_error: false,
            })
        }
    }
}

pub fn bash_tool_schema() -> serde_json::Value {
    json!({
        "type": "object",
        "required": ["command"],
        "properties": {
            "command": {
                "type": "string",
                "description": "Bash command to execute"
            },
            "timeout": {
                "type": "number",
                "description": "Timeout in seconds (optional, no default timeout)"
            },
            "allowDestructiveGit": {
                "type": "boolean",
                "description": "Skip the dirty-tree guard for destructive git discard commands. Only set when discarding uncommitted work is intentional."
            }
        }
    })
}

pub fn bash_tool_description() -> String {
    format!(
        "Execute a bash command in the current working directory. Returns stdout and stderr. Output is truncated to last {DEFAULT_MAX_LINES} lines or {}KB (whichever is hit first). If truncated, full output is saved to a temp file. Optionally provide a timeout in seconds. Destructive git discard commands (git checkout -- ., git checkout ., git clean -f..., git reset --hard, git restore .) are refused while uncommitted changes exist; retry with allowDestructiveGit: true only when the discard is intentional.",
        DEFAULT_MAX_BYTES / 1024
    )
}

/// The `bash` tool definition: exact name, schema, and description.
#[must_use]
pub fn create_bash_tool_definition(cwd: &str) -> ToolDefinition {
    create_bash_tool_definition_with_options(cwd, &BashToolOptions::default())
}

#[must_use]
pub fn create_bash_tool_definition_with_options(
    cwd: &str,
    options: &BashToolOptions,
) -> ToolDefinition {
    let cwd = cwd.to_string();
    let operations = options.operations.clone();
    let command_prefix = options.command_prefix.clone();
    let shell_path = options.shell_path.clone();
    let spawn_hook = options.spawn_hook.clone();
    let arbiter = options.arbiter.clone();
    let execute: crate::tools::tool_definition::ExecuteFn = {
        Arc::new(move |_tool_call_id, params, signal, on_update| {
            let cwd = cwd.clone();
            let options = BashToolOptions {
                operations: operations.clone(),
                command_prefix: command_prefix.clone(),
                shell_path: shell_path.clone(),
                spawn_hook: spawn_hook.clone(),
                arbiter: arbiter.clone(),
            };
            Box::pin(async move {
                let command = params
                    .get("command")
                    .and_then(serde_json::Value::as_str)
                    .ok_or_else(|| anyhow::anyhow!("bash tool requires a command string"))?
                    .to_string();
                let timeout = params.get("timeout").and_then(serde_json::Value::as_f64);
                let allow_destructive_git = params
                    .get("allowDestructiveGit")
                    .and_then(serde_json::Value::as_bool);
                execute_bash(
                    &cwd,
                    &options,
                    &command,
                    timeout,
                    allow_destructive_git,
                    signal,
                    on_update,
                )
                .await
            })
        })
    };
    ToolDefinition {
        name: "bash".to_string(),
        label: "bash".to_string(),
        description: bash_tool_description(),
        prompt_snippet: "Execute bash commands (ls, grep, find, etc.)".to_string(),
        parameters: bash_tool_schema(),
        execution_mode: None,
        prepare_arguments: None,
        execute,
    }
}

/// Refusal text for discard commands whose target repository cannot be
/// resolved safely (TS: `formatRelocationRefusal`).
const RELOCATION_REFUSAL: &str = "Refusing to run this destructive git command: it changes directory (or repository) first, and the uncommitted changes of the repository it targets cannot be checked safely.\n\nRun the discard as its own command from the target directory, or retry with allowDestructiveGit: true, or set PI_BASH_ALLOW_DESTRUCTIVE_GIT=1.";

#[cfg(test)]
mod signal_exit_tests {
    use super::*;

    /// Operations that resolve with `Ok(None)` — the child was killed by a
    /// signal (the `BashOperations` contract for that case).
    struct SignalledOps;

    impl BashOperations for SignalledOps {
        fn exec<'a>(
            &'a self,
            _command: &'a str,
            _cwd: &'a str,
            _options: ExecOptions<'a>,
        ) -> ExecFuture<'a> {
            Box::pin(async move { Ok(None) })
        }
    }

    /// The reported regression: a child killed by a signal (`exit_code =
    /// None`) was treated as a SUCCESS because `is_some_and(|code| code != 0)`
    /// is false for `None`. It must be a failure.
    #[tokio::test]
    async fn a_signal_killed_command_is_a_failure_not_a_success() {
        let options = BashToolOptions {
            operations: Some(std::sync::Arc::new(SignalledOps)),
            ..Default::default()
        };
        let result = execute_bash("/tmp", &options, "sleep 100", None, None, None, None).await;
        let error = result.expect_err("a signalled child is a failure");
        assert!(
            error.to_string().contains("killed by a signal"),
            "the failure names the signal kill: {error}"
        );
    }
}

#[cfg(test)]
mod arbiter_note_tests {
    use super::*;
    use std::sync::Mutex;

    use crate::jev::{ToolArbiter, ToolAvailability};

    /// Operations that record the commands they were asked to run and answer
    /// with a fixed exit code - so the test asserts the arbiter never altered
    /// what reached the shell.
    struct RecordingOps {
        commands: Arc<Mutex<Vec<String>>>,
        exit_code: i32,
    }

    impl BashOperations for RecordingOps {
        fn exec<'a>(
            &'a self,
            command: &'a str,
            _cwd: &'a str,
            options: ExecOptions<'a>,
        ) -> ExecFuture<'a> {
            self.commands
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(command.to_string());
            (options.on_data)(b"src/lib.rs:12:    fn main() {}\n");
            let code = self.exit_code;
            Box::pin(async move { Ok(Some(code)) })
        }
    }

    fn recording_options(exit_code: i32) -> (BashToolOptions, Arc<Mutex<Vec<String>>>) {
        let commands = Arc::new(Mutex::new(Vec::new()));
        let arbiter = ToolArbiter::new();
        arbiter.arm(ToolAvailability::all(), None);
        let options = BashToolOptions {
            operations: Some(Arc::new(RecordingOps {
                commands: Arc::clone(&commands),
                exit_code,
            })),
            arbiter: Some(Arc::new(arbiter)),
            ..Default::default()
        };
        (options, commands)
    }

    /// The core promise: the note is ADVISORY. The command reaches the shell
    /// exactly as written, the exit status decides success, and the hint is
    /// appended after the output.
    #[tokio::test]
    async fn a_matching_command_runs_unchanged_and_gains_a_note() {
        let (options, commands) = recording_options(0);
        let command = r#"grep -rn "fn main" src"#;
        let result = execute_bash("/tmp", &options, command, None, None, None, None)
            .await
            .expect("a zero exit is a success");
        assert_eq!(
            commands.lock().unwrap().as_slice(),
            [command.to_string()],
            "the arbiter never rewrites the command"
        );
        let text = match &result.content[0] {
            ToolContentBlock::Text { text } => text,
            other @ ToolContentBlock::Image { .. } => {
                panic!("expected text, got {other:?}")
            }
        };
        assert!(
            text.contains("src/lib.rs:12"),
            "the output is intact: {text}"
        );
        assert!(
            text.contains("[tool-arbiter]"),
            "the note is appended: {text}"
        );
        assert!(text.contains("graph.search"), "{text}");
        assert!(text.contains("ran unchanged"), "{text}");
        let details = result.details.expect("the note rides the details");
        assert_eq!(details["toolArbiter"]["tool"], "graph.search");
        assert_eq!(details["toolArbiter"]["judged"], false);
    }

    /// No rule, no note: an unrelated command's result is byte-identical to
    /// the pre-arbiter path.
    #[tokio::test]
    async fn an_unrelated_command_gets_no_note() {
        let (options, _commands) = recording_options(0);
        let result = execute_bash("/tmp", &options, "cargo test --lib", None, None, None, None)
            .await
            .expect("success");
        let text = match &result.content[0] {
            ToolContentBlock::Text { text } => text,
            other @ ToolContentBlock::Image { .. } => {
                panic!("expected text, got {other:?}")
            }
        };
        assert!(!text.contains("[tool-arbiter]"), "{text}");
        assert!(result.details.is_none(), "{:?}", result.details);
    }

    /// A failing command keeps its failure: the arbiter does not turn a
    /// non-zero exit into a success just because it has advice.
    #[tokio::test]
    async fn a_failing_command_still_fails() {
        let (options, _commands) = recording_options(2);
        let error = execute_bash(
            "/tmp",
            &options,
            r#"grep -rn "fn main" src"#,
            None,
            None,
            None,
            None,
        )
        .await
        .expect_err("exit 2 is a failure");
        assert!(error.to_string().contains("exited with code 2"), "{error}");
    }
}
