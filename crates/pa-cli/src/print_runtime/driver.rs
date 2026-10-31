//! The print prompt driver: admit the prompts, stream json events when the
//! output mode asks for them, and decide the exit code from the headless
//! terminal result plus the goal and autonomous continuation contracts.

use pa_core::session_engine::provider_adapter::json_round_trip;
use pa_core::session_engine::session_events::agent_event_json;
use pa_types::ai::Model;

use crate::headless_autonomous::HeadlessAutonomous;
use crate::mode::{AppMode, RunOptions};

/// The session header line: the session file's `type: "session"` entry in the
/// TS wire shape and field order.
pub(super) async fn session_header_json(
    engine: &pa_core::session_engine::engine::SessionEngine,
) -> Option<String> {
    let persistence = engine.session.shared_persistence();
    let session = persistence.lock().await;
    let header = session.get_header()?;
    // The TS field order, optional fields only when present.
    let mut object = serde_json::Map::new();
    let field = |map: &mut serde_json::Map<String, serde_json::Value>,
                 key: &str,
                 value: Option<serde_json::Value>| {
        if let Some(value) = value {
            map.insert(key.to_string(), value);
        }
    };
    field(&mut object, "type", Some(serde_json::json!("session")));
    field(
        &mut object,
        "version",
        header.version.map(serde_json::Value::from),
    );
    field(&mut object, "id", Some(serde_json::json!(header.id)));
    field(
        &mut object,
        "timestamp",
        Some(serde_json::json!(header.timestamp)),
    );
    field(&mut object, "cwd", Some(serde_json::json!(header.cwd)));
    field(
        &mut object,
        "parentSession",
        header
            .parent_session
            .as_ref()
            .map(|parent| serde_json::json!(parent)),
    );
    field(
        &mut object,
        "rlmDepth",
        header.rlm_depth.map(serde_json::Value::from),
    );
    field(
        &mut object,
        "git",
        header
            .git
            .as_ref()
            .map(|git| serde_json::to_value(git).unwrap_or(serde_json::Value::Null)),
    );
    Some(serde_json::Value::Object(object).to_string())
}

/// Admit prompts, stream json events when requested, and decide the exit code
/// from the headless terminal result plus the autonomous gate contract.
pub(super) async fn run_prompts_and_emit(
    engine: &std::sync::Arc<pa_core::session_engine::engine::SessionEngine>,
    model: &Model,
    api_key: Option<String>,
    options: &RunOptions,
    root_node: &std::sync::Arc<crate::print_rlm::InlineSessionNode>,
) -> Result<i32, String> {
    let json_mode = options.app_mode == AppMode::Json;
    let mut unsubscribe: Option<pa_agent::agent::Subscription> = None;
    if json_mode {
        if let Some(header) = session_header_json(engine).await {
            println!("{header}");
        }
        unsubscribe = Some(
            engine
                .session
                .agent()
                .subscribe(|event, _signal| {
                    Box::pin(async move {
                        if let Some(json) = agent_event_json(&event) {
                            println!("{json}");
                        }
                        Ok(())
                    })
                })
                .await,
        );
    }
    // The goal continuation surface: wired in every output mode — the loop runs in text mode too,
    // only silently.
    let goal = std::sync::Arc::new(crate::print_goal::PrintGoalSurface::new(json_mode));
    goal.seed_publish_baseline(engine).await;
    let goal_updated_at_start = engine.goal_state().await.updated_at;
    let goal_accounting = goal.wire_accounting(engine, engine.session.agent()).await;
    // The autonomous run: the CLI flags enable it, a no-flag session starts disabled and
    // `/autonomous` rewrites it live.
    let autonomous = std::sync::Arc::new(match options.config.autonomous.as_ref() {
        Some(config) => HeadlessAutonomous::from_cli(config, &options.config.cwd),
        None => HeadlessAutonomous::disabled(&options.config.cwd),
    });
    let accounting = autonomous.wire_accounting(engine.session.agent()).await;
    // The composed natural-turn-end hook: the goal arm first (exclusive priority), the
    // autonomous arm on the fall-through; both arms defer while inline-subagent work is
    // unsettled.
    crate::print_autonomous::wire_continuation_hook(
        engine,
        engine.session.agent(),
        model,
        &goal,
        &autonomous,
        Some(root_node),
    );
    let global_harness_dir =
        pa_core::refinement::get_global_harness_state_dir(&options.config.agent_dir);
    let mut boundary = crate::print_boundary::TurnBoundary::new(json_mode);
    // The autonomous runtime state the session-command executor mutates —
    // `/autonomous` rewrites what the hook, accounting, and exit contract read.
    let autonomous_state = autonomous.state_handle();
    // A failed session command rejects the prompt wait: the raw error prints to stderr and the run
    // exits 1.
    let mut command_failure: Option<String> = None;
    // The `@file` image attachments ride the initial prompt only; the later CLI messages stay text.
    'prompts: for (prompt, images) in options
        .initial_message
        .iter()
        .map(|prompt| (prompt, options.initial_images.clone()))
        .chain(options.messages.iter().map(|prompt| (prompt, Vec::new())))
    {
        // Session commands never reach the model loop: the pre-turn boundary stays theirs to skip
        // and the prompt's turn never exists.
        if let Some(command) = engine.session.classify_session_command(prompt) {
            let execution = crate::print_session_command::execute_prompt_session_command(
                engine,
                &goal,
                model,
                api_key.clone(),
                global_harness_dir.clone(),
                &autonomous_state,
                &command,
                &images,
            )
            .await;
            if let Some(error) = execution.error {
                command_failure = Some(error);
                break 'prompts;
            }
            // A `/goal` start scheduled its continuation as queued session input: the prompt wait
            // drains it inside the same wait.
            if let Some(continuation) = execution.continuation_message {
                goal.run_session_command_continuation(
                    engine,
                    &mut boundary,
                    model,
                    api_key.clone(),
                    global_harness_dir.clone(),
                    &continuation,
                )
                .await?;
            }
            // The same queue drain a settled turn gets: held continuations and armed steers run as
            // this prompt's follow-up turns.
            goal.drive_boundary(
                engine,
                &mut boundary,
                model,
                api_key.clone(),
                global_harness_dir.clone(),
            )
            .await?;
            continue;
        }
        boundary
            .run_pre_turn(engine, model, api_key.clone())
            .await?;
        engine
            .session
            .prompt_with_images(
                prompt,
                images,
                pa_core::session_engine::PromptOptions::default(),
            )
            .await
            .map_err(|error| format!("{error:#}"))?;
        engine.session.agent().wait_for_idle().await;
        boundary
            .run_at_settled_turn(engine, model, api_key.clone(), global_harness_dir.clone())
            .await?;
        goal.drive_boundary(
            engine,
            &mut boundary,
            model,
            api_key.clone(),
            global_harness_dir.clone(),
        )
        .await?;
        // The autonomous arm runs only when the goal does not own the boundary (the goal arm takes
        // exclusive priority).
        if !engine.goal_owns_continuation_wakeup().await {
            // The held threshold continuation drains as this invocation's follow-up
            // turn; the stop surfaces only through the exit contract.
            autonomous
                .drive_boundary(
                    engine,
                    &mut boundary,
                    model,
                    api_key.clone(),
                    global_harness_dir.clone(),
                )
                .await
                .map_err(|error| format!("{error:#}"))?;
        }
    }
    // The rejected prompt wait: print the raw command error to stderr and
    // exit 1 — no later prompts ran, and the disposal drain still runs.
    if let Some(error) = command_failure {
        eprintln!("{error}");
        goal_accounting.unsubscribe().await;
        accounting.unsubscribe().await;
        if let Some(subscription) = unsubscribe {
            subscription.unsubscribe().await;
        }
        boundary
            .drain_compact_auto_refine_at_disposal(engine, model, api_key, global_harness_dir)
            .await;
        return Ok(1);
    }
    // The #2270 fix: print waits for RLM quiescence — every admitted
    // descendant settles and its replies and notices drain as turns —
    // before the terminal result is selected (json mode keeps streaming;
    // the subscription lives until after the drive). One drive loop then
    // keeps the run alive while deferred work exists: the deferred
    // goal/autonomous continuations deliver after the settle ("no
    // front": the settling child's terminal notice is read first), and
    // the boundary drain runs whatever turns it drained armed — a
    // threshold continuation, a budget steer, more descendants.
    loop {
        if crate::print_rlm::drive_to_quiescence(
            root_node,
            engine,
            &mut boundary,
            model,
            api_key.clone(),
            global_harness_dir.clone(),
        )
        .await?
        {
            continue;
        }
        if let Some(continuation) = engine.take_owed_goal_continuation().await {
            goal.publish_goal_update(engine).await;
            goal.run_queued_turn(
                engine,
                &mut boundary,
                model,
                api_key.clone(),
                global_harness_dir.clone(),
                &continuation,
            )
            .await?;
            continue;
        }
        if autonomous
            .deliver_rlm_continuation(
                engine,
                &mut boundary,
                model,
                api_key.clone(),
                global_harness_dir.clone(),
            )
            .await?
        {
            continue;
        }
        // The boundary drain: turns it ran may spawn descendants or arm
        // more holds, so it reports whether it ran and the loop goes
        // around — the leading quiescence pass then waits for whatever
        // they left behind.
        let mut boundary_ran = goal
            .drive_boundary(
                engine,
                &mut boundary,
                model,
                api_key.clone(),
                global_harness_dir.clone(),
            )
            .await?;
        if !engine.goal_owns_continuation_wakeup().await {
            boundary_ran |= autonomous
                .drive_boundary(
                    engine,
                    &mut boundary,
                    model,
                    api_key.clone(),
                    global_harness_dir.clone(),
                )
                .await?;
        }
        if boundary_ran {
            continue;
        }
        break;
    }
    goal_accounting.unsubscribe().await;
    accounting.unsubscribe().await;
    if let Some(subscription) = unsubscribe {
        subscription.unsubscribe().await;
    }
    let state = engine.session.agent().state().await;
    let messages: Vec<pa_types::session::AgentMessage> =
        state.messages.iter().filter_map(json_round_trip).collect();
    let result = pa_core::session_engine::headless::select_headless_terminal_result(&messages);
    // The print-mode exit contract: json mode never derives the exit code from the
    // terminal selection — only the autonomous gates exit non-zero; text mode prints
    // the primary message (error to stderr with exit 1, settled answer to stdout).
    let mut exit_code = 0;
    if json_mode {
        let failure = result
            .primary
            .as_ref()
            .and_then(pa_core::session_engine::headless::HeadlessPrimary::failure_reason);
        let outcome = match failure {
            Some(reason) => serde_json::json!({ "ok": false, "reason": reason }),
            None => serde_json::json!({ "ok": true }),
        };
        println!(
            "{}",
            serde_json::json!({ "type": "outcome", "outcome": outcome })
        );
    }
    if !json_mode {
        let goal = engine.goal_state().await;
        let cap_reason = pa_core::session_engine::goal_driver::CONTINUATION_NO_PROGRESS_CAP_REASON;
        if goal.status == pa_types::goal::GoalStatus::Error
            && goal.last_error.as_deref() == Some(cap_reason)
            && goal.updated_at != goal_updated_at_start
        {
            eprintln!("{cap_reason}");
            exit_code = 1;
        }
        if let Some(primary) = result.primary {
            if let Some(stderr) = primary.stderr_text(&mut exit_code) {
                eprintln!("{stderr}");
            }
            if exit_code == 0 {
                if let Some(text) = primary.stdout_text() {
                    println!("{text}");
                }
            }
        }
        for outcome in result.compaction_outcomes {
            eprintln!("{}", outcome.content);
            if outcome.outcome == "failed" {
                exit_code = 1;
            }
        }
    }
    // The autonomous contract applies to both output modes.
    if let Some(stderr) = autonomous.exit_stderr().await {
        eprintln!("{stderr}");
        exit_code = 1;
    }
    // The disposal order: the exit code first, then the teardown drains a
    // compact-trigger auto-refine no later boundary consumed.
    boundary
        .drain_compact_auto_refine_at_disposal(engine, model, api_key, global_harness_dir)
        .await;
    Ok(exit_code)
}
