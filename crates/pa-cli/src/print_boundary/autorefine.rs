//! The print boundary's compact-trigger auto-refine surface: what the round
//! prints. The machine itself — the refine gates, the review cooldown, the
//! in-flight guard, the retained approval — belongs to the session
//! (`pa_core::session_engine::auto_refine_trigger`); this module only derives
//! the settled-turn count the review prompt reads and renders the round's rows.

use pa_core::session_engine::auto_refine_trigger::CompactAutoRefineSurface;

use super::{json, Model, PathBuf, SessionAgentMessage, SessionEngine, TurnBoundary};

impl TurnBoundary {
    /// Count the settled non-error, non-aborted assistant turns appended since
    /// the last boundary call into the session's review-trigger counter. The
    /// print loop has no message-end hook, so it derives the count from the
    /// durable rows; the counter itself lives on the session.
    pub(super) async fn count_settled_turns(&mut self, engine: &SessionEngine) {
        let Some(baseline) = self.entry_baseline else {
            return;
        };
        let entries = engine.session.entries().await;
        let settled = entries
            .iter()
            .skip(baseline)
            .filter(|entry| {
                matches!(
                    entry,
                    pa_types::session::FileEntry::Message {
                        message: SessionAgentMessage::Assistant(assistant),
                        ..
                    } if assistant.stop_reason != pa_types::ai::StopReason::Error
                        && assistant.stop_reason != pa_types::ai::StopReason::Aborted
                )
            })
            .count();
        self.entry_baseline = Some(entries.len());
        for _ in 0..settled {
            engine.session.note_settled_turn_since_auto_refine_review();
        }
    }

    /// The disposal drain: a serialized compaction can finish without another model
    /// turn — drain its pending review here so disposal does not lose the
    /// trigger. The subscription is already torn down; only durable rows and
    /// harness state persist. Best-effort.
    pub(crate) async fn drain_compact_auto_refine_at_disposal(
        &mut self,
        engine: &SessionEngine,
        model: &Model,
        api_key: Option<String>,
        global_harness_dir: PathBuf,
    ) {
        self.service_compact_auto_refine(
            engine,
            model,
            api_key,
            global_harness_dir,
            CompactAutoRefineSurface::Dispose,
        )
        .await;
    }

    /// Service one pending compact-trigger round: the session runs the machine
    /// (gates, cooldown, in-flight guard, review, branch fence) and answers the
    /// round, this surface renders it. A failed round surfaces as its own
    /// `refine_failed` event, never as a boundary error — a broken refinement
    /// must not end the print run.
    pub(super) async fn service_compact_auto_refine(
        &mut self,
        engine: &SessionEngine,
        model: &Model,
        api_key: Option<String>,
        global_harness_dir: PathBuf,
        surface: CompactAutoRefineSurface,
    ) {
        // The cheap pre-check: most boundaries carry no trigger at all.
        if !engine.session.compact_auto_refine_pending() {
            return;
        }
        // The refinement rows this round appends land after this count.
        let entries_before = engine.session.entries().await.len();
        let outcome = match engine
            .session
            .consume_compact_auto_refine(model, api_key, global_harness_dir, surface)
            .await
        {
            // Every silent outcome: a gate drop, the cooldown, a round already
            // in flight, a retained approval waiting out the turn, or a
            // declining review.
            Ok(None) => return,
            Ok(Some(result)) => Ok(result),
            Err(error) => Err(error),
        };
        self.stream_refinement_outcome(
            engine,
            &outcome,
            entries_before,
            surface == CompactAutoRefineSurface::Checkpoint,
            "automatic",
        )
        .await;
    }

    /// One refinement outcome's surface: the durable rows' message pairs plus
    /// `refine_complete` on success, `refine_failed` on failure; `emit` false keeps
    /// the stream quiet (the rows still persist).
    pub(super) async fn stream_refinement_outcome(
        &self,
        engine: &SessionEngine,
        outcome: &anyhow::Result<pa_core::refinement::RefinementResult>,
        entries_before: usize,
        emit: bool,
        kind: &str,
    ) {
        match outcome {
            Ok(result) => {
                if emit && self.json_mode {
                    // The refinement rows this run appended: the outcome row always, the
                    // model-facing notice when edits applied.
                    for row in Self::refinement_rows_since(engine, entries_before).await {
                        let value = crate::headless_autonomous::custom_row_wire_value(&row);
                        for event_type in ["message_start", "message_end"] {
                            (self.sink)(&json!({ "type": event_type, "message": value }));
                        }
                    }
                    self.emit_json(&json!({
                        "type": "refine_complete",
                        "result": serde_json::to_value(result)
                            .unwrap_or(serde_json::Value::Null),
                    }));
                }
            }
            Err(error) => {
                if emit {
                    if self.json_mode {
                        self.emit_json(&json!({
                            "type": "refine_failed",
                            "error": format!("{error}"),
                        }));
                    } else {
                        eprintln!("pa-cli: {kind} refinement failed: {error:#}");
                    }
                }
            }
        }
    }

    /// The durable refinement rows appended after an entry count: the outcome row,
    /// then the model-facing notice when edits applied, in that order.
    pub(crate) async fn refinement_rows_since(
        engine: &SessionEngine,
        entries_before: usize,
    ) -> Vec<pa_types::session::CustomMessage> {
        engine
            .session
            .entries()
            .await
            .into_iter()
            .skip(entries_before)
            .filter_map(|entry| match entry {
                pa_types::session::FileEntry::CustomMessage { payload, base }
                    if payload.custom_type
                        == pa_core::session_engine::refine::REFINEMENT_OUTCOME_CUSTOM_TYPE
                        || payload.custom_type
                            == pa_core::session_engine::refine::REFINEMENT_NOTICE_CUSTOM_TYPE =>
                {
                    Some(pa_types::session::CustomMessage {
                        custom_type: payload.custom_type.clone(),
                        content: payload.content.clone(),
                        display: payload.display,
                        details: payload.details.clone(),
                        timestamp: base
                            .timestamp
                            .as_deref()
                            .map(pa_core::session::timestamp_to_millis)
                            .unwrap_or_default(),
                        rest: payload.rest,
                    })
                }
                _ => None,
            })
            .collect()
    }
}
