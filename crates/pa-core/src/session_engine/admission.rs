use super::slash_commands::parse_session_command;
use super::{
    session_message_to_loop, user_prompt_message, AgentSession, PromptOptions, PromptOutcome,
    SessionAgentMessage, SessionSlashCommand, StreamingBehavior,
};

/// One drained next-turn row beside its loop form: the durable custom row is
/// what a refused admission puts back, and the loop row is what the turn
/// carries (and what tells a landed row from one that never ran).
type NextTurnRow = (
    pa_types::session::CustomMessage,
    pa_agent::types::AgentMessage,
);

impl AgentSession {
    /// Submit a prompt. Session commands (compact/refine/goal/autonomous)
    /// are recognized before admission and never reach the model.
    ///
    /// # Errors
    ///
    /// The underlying prompt admission error (see [`AgentSession::prompt_with_images`]).
    pub async fn prompt(
        &self,
        text: &str,
        options: PromptOptions,
    ) -> anyhow::Result<PromptOutcome> {
        self.prompt_with_images(text, Vec::new(), options).await
    }

    /// Admit an injected custom message as the turn's prompt (TS
    /// `_promptInjectedMessage`): the loop context and transcript hold ONE
    /// representation — the custom row itself — while the provider request
    /// carries its user-role view; never template-expanded or command-parsed.
    ///
    /// # Errors
    ///
    /// Error when the session is busy, the digest capture fails, or the
    /// agent rejects the prompt.
    pub async fn prompt_injected_message(
        &self,
        message: &pa_types::session::CustomMessage,
    ) -> anyhow::Result<PromptOutcome> {
        let state = self.agent.state().await;
        let busy = state.is_streaming;
        if busy {
            anyhow::bail!(
                "Agent is already processing. Specify streamingBehavior ('steer' or 'followUp') to queue the message."
            );
        }
        let digest_row = self.pending_digest_prompt_row().await?;
        let custom_row = session_message_to_loop(&SessionAgentMessage::Custom(message.clone()))
            .ok_or_else(|| anyhow::anyhow!("injected custom message conversion failed"))?;
        // The dispatch-time routing fires for every dispatched turn; injected
        // rows do not re-run the route decision (a goal objective start's
        // images ride the row's content and the row runs on the session's
        // model), so a route left by the previous turn clears here.
        self.apply_image_model_routing(&[], &[]).await?;
        // The queue is read only now that nothing can refuse this turn:
        // taking the rows before the checks above lost them on every
        // refusal (review #34).
        let queued_rows = self.take_queued_next_turn_pairs();
        let mut prompt_messages = Vec::new();
        prompt_messages.extend(digest_row.clone());
        prompt_messages.extend(queued_rows.iter().map(|(_, loop_row)| loop_row.clone()));
        prompt_messages.push(custom_row);
        // What the transcript held BEFORE this attempt: a refusal puts back
        // only the rows that appeared after it, so a row equal to one an
        // earlier turn committed is not mistaken for one this attempt landed.
        let history_len = self.agent.state().await.messages.len();
        if let Err(error) = self
            .agent
            .prompt(pa_agent::agent::AgentPromptInput::Messages(prompt_messages))
            .await
        {
            self.restore_refused_next_turn_rows(queued_rows, history_len)
                .await;
            self.rearm_refused_digest_row(digest_row.as_ref(), history_len)
                .await;
            return Err(error);
        }
        Ok(PromptOutcome::Prompt)
    }

    /// Classify a prompt as a session command without admitting it (the same
    /// expansion-plus-grammar parse `prompt` applies). Pre-turn compaction
    /// arms stay off the session-command path (TS never reaches `_prepareForCommit`).
    pub fn classify_session_command(&self, text: &str) -> Option<SessionSlashCommand> {
        let normalized = crate::skills::expand_prompt_template(text, &self.prompt_templates);
        parse_session_command(&self.slash_commands, &normalized)
    }

    /// Prompt with images attached (the ACP prompt-capability path). Busy
    /// sessions queue the text and images together as one follow-up batch,
    /// so an admitted prompt never loses its images to a queue race.
    ///
    /// # Errors
    ///
    /// Error when the prompt fails validation, the session is busy, or the
    /// agent rejects the turn.
    pub async fn prompt_with_images(
        &self,
        text: &str,
        images: Vec<pa_agent::types::ImageContent>,
        options: PromptOptions,
    ) -> anyhow::Result<PromptOutcome> {
        let expand = options.expand_prompt_templates.unwrap_or(true);
        // TS `_finishSubmissionNormalization` order: skill commands expand first, prompt templates
        // second; both are gated by the same policy flag.
        let (normalized, used_skill) = if expand {
            let (skill_expanded, used_skill) =
                crate::skills::expand_skill_command(text, &self.skills);
            let normalized =
                crate::skills::expand_prompt_template(&skill_expanded, &self.prompt_templates);
            (normalized, used_skill)
        } else {
            (text.to_string(), None)
        };

        if let Some(command) = parse_session_command(&self.slash_commands, &normalized) {
            return Ok(PromptOutcome::SessionCommand(command));
        }

        let state = self.agent.state().await;
        let busy = state.is_streaming;
        // The `skill_use_count` session counter counts at the admission
        // seam: an admitted user turn whose text IS a skill block counts
        // once. A pre-expanded block (the daemon emits the accepted row
        // before admission) counts here too — the block parse carries the
        // skill identity.
        let skill_used = used_skill.is_some()
            || pa_types::skill_blocks::parse_skill_block(&normalized)
                .is_some_and(|block| self.skills.iter().any(|skill| skill.name == block.name));
        if skill_used {
            if let Some(telemetry) = &self.skill_telemetry {
                telemetry.note_skill_used();
            }
        }
        if busy && options.streaming_behavior.is_none() {
            anyhow::bail!(
                "Agent is already processing. Specify streamingBehavior ('steer' or 'followUp') to queue the message."
            );
        }
        // User messages persist through the loop's `message_end` event
        // (matching TS `_processAgentEvent`); appending here too would
        // double-persist.

        if busy {
            let message = user_prompt_message(&normalized, &images);
            match options.streaming_behavior {
                Some(StreamingBehavior::Steer) => self.agent.steer(message),
                Some(StreamingBehavior::FollowUp) => self.agent.follow_up(message),
                None => unreachable!("busy without a streaming behavior errors above"),
            }
        } else {
            // The dispatch-time image-model routing decision: an
            // image-attaching batch routes to the host's configured image
            // model or fails with the actionable refusal, never silently
            // downgrading the images to placeholders.
            self.apply_image_model_routing(&images, &options.batch)
                .await?;
            // FORK-ONLY (jev Phase 2): the advisory difficulty classification
            // runs once per admitted turn, before the prompt reaches the loop.
            // It never fails the turn (fail-open) and never runs without the
            // engine-installed wiring (the default).
            self.apply_auto_thinking(&normalized).await;
            // The turn's prompt messages: the deferred first-turn harness
            // digest rides first when one is due, streamed ahead of the user
            // prompt and carried on `agent_end`. The queue is read only now
            // that nothing above can refuse the turn: reading it earlier lost
            // every queued row on a refusal (review #34).
            let digest_row = self.pending_digest_prompt_row().await?;
            let queued_rows = self.take_queued_next_turn_pairs();
            let mut prompt_messages = Vec::new();
            prompt_messages.extend(digest_row.clone());
            prompt_messages.extend(queued_rows.iter().map(|(_, loop_row)| loop_row.clone()));
            prompt_messages.push(user_prompt_message(&normalized, &images));
            // Each batched action contributes its user row after the
            // primary, through the same admission normalization (TS
            // normalizes each submission at queue time; this engine
            // normalizes every row at the shared admission).
            for row in &options.batch {
                let row_text = if expand {
                    let (skill_expanded, _) =
                        crate::skills::expand_skill_command(&row.text, &self.skills);
                    crate::skills::expand_prompt_template(&skill_expanded, &self.prompt_templates)
                } else {
                    row.text.clone()
                };
                prompt_messages.push(user_prompt_message(&row_text, &row.images));
            }
            // The transcript's length BEFORE this attempt: a refusal puts back
            // only the rows that appeared after it. Comparing with the whole
            // history dropped a row equal to one an earlier turn committed —
            // the notices are templated, so the collision is real (review #34).
            let history_len = self.agent.state().await.messages.len();
            // A refused admission never ran this turn: put its queued rows
            // back so the next admitted turn still carries them.
            let admitted = if options.return_after_accepted {
                self.agent
                    .prompt_until_accepted(pa_agent::agent::AgentPromptInput::Messages(
                        prompt_messages,
                    ))
                    .await
            } else {
                self.agent
                    .prompt(pa_agent::agent::AgentPromptInput::Messages(prompt_messages))
                    .await
            };
            if let Err(error) = admitted {
                self.restore_refused_next_turn_rows(queued_rows, history_len)
                    .await;
                self.rearm_refused_digest_row(digest_row.as_ref(), history_len)
                    .await;
                // A concurrent admission won the agent's run slot: this
                // prompt never started, so its route must not survive. The
                // unwind only happens while NO run streams (the winner keeps
                // its target); TS's single-threaded commit cannot see this race.
                if !self.agent.state().await.is_streaming {
                    if let Some(router) = self.image_model_router.as_ref() {
                        (router.swap_target)(None);
                        self.agent.set_model_override(None);
                    }
                }
                return Err(error);
            }
        }
        Ok(PromptOutcome::Prompt)
    }

    /// Queue one custom row for the next admitted turn: the row rides the
    /// turn's prompt messages ahead of the prompt's own user row.
    pub fn queue_next_turn_row(&self, message: pa_types::session::CustomMessage) {
        self.pending_next_turn_rows
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(message);
    }

    /// Adopt a shared next-turn mailbox (the engine's restore-notice seam):
    /// rows a kernel boot parked before the session existed merge in, and
    /// later pushes land in the same queue: the kernel provisioner outlives
    /// the construction order, so the mailbox must cross the build boundary.
    pub fn adopt_next_turn_rows(
        &mut self,
        shared: std::sync::Arc<std::sync::Mutex<Vec<pa_types::session::CustomMessage>>>,
    ) {
        let own_rows: Vec<_> = {
            let mut own = self
                .pending_next_turn_rows
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            own.drain(..).collect()
        };
        {
            let mut next = shared
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            // The shared mailbox is authoritative: rows parked pre-build
            // (a restore that finished during construction) come first,
            // then anything this session queued before adoption.
            next.extend(own_rows);
        }
        self.pending_next_turn_rows = shared;
    }

    /// Every queued next-turn row, drained for a turn that WILL be admitted:
    /// that turn owns them and an empty take leaves nothing for later turns.
    ///
    /// Both hops of the admission call this only after nothing can refuse the
    /// turn any more — reading the queue first lost it on every refusal
    /// (review #34). Each returned pair keeps the durable row next to its
    /// loop form so a refusal can put the exact rows back. A row that fails
    /// the conversion stays queued with a warning rather than vanishing from
    /// both the queue and the turn.
    fn take_queued_next_turn_pairs(&self) -> Vec<NextTurnRow> {
        let mut kept: Vec<pa_types::session::CustomMessage> = Vec::new();
        let taken: Vec<NextTurnRow> = {
            let mut queue = self
                .pending_next_turn_rows
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let rows: Vec<pa_types::session::CustomMessage> = queue.drain(..).collect();
            drop(queue);
            let mut taken = Vec::with_capacity(rows.len());
            for row in rows {
                if let Some(message) =
                    session_message_to_loop(&SessionAgentMessage::Custom(row.clone()))
                {
                    taken.push((row, message));
                } else {
                    tracing::warn!(
                        custom_type = %row.custom_type,
                        "next-turn row could not be converted; it stays queued"
                    );
                    kept.push(row);
                }
            }
            taken
        };
        if !kept.is_empty() {
            let mut queue = self
                .pending_next_turn_rows
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            queue.extend(kept);
        }
        taken
    }

    /// Re-arm the deferred first-turn digest a refused admission consumed:
    /// the turn that was supposed to carry it never ran, so the next admitted
    /// turn still owes the model its harness context. A row the losing attempt
    /// already landed is not re-armed — it would reach the context twice.
    ///
    /// The debt is the `digest_pending` flag, so it is re-armed whether or not
    /// this turn got as far as rendering a row; the next admitted turn renders
    /// the digest fresh.
    ///
    /// `history_len` is the transcript length before the refused attempt: only
    /// a row past that boundary counts as landed by THIS turn.
    async fn rearm_refused_digest_row(
        &self,
        row: Option<&pa_agent::types::AgentMessage>,
        history_len: usize,
    ) {
        let landed = match row {
            Some(row) => self
                .agent
                .state()
                .await
                .messages
                .get(history_len..)
                .is_some_and(|landed| landed.contains(row)),
            None => false,
        };
        if !landed {
            self.digest_pending
                .store(true, std::sync::atomic::Ordering::SeqCst);
        }
    }

    /// Give a refused admission's rows back, in their original order and in
    /// front of anything queued meanwhile: the turn those rows were taken for
    /// never started, so the next admitted turn must see them first.
    ///
    /// A row the losing attempt already landed is NOT re-queued: `Agent::prompt`
    /// can fail after part of the batch reached the loop, and a second copy
    /// would reach the provider twice. Only the transcript PAST `history_len`
    /// (its length before the attempt) counts as landed: the rows are templated
    /// notices, so a row equal to one an earlier turn committed would otherwise
    /// be dropped as if this attempt had carried it (review #34).
    async fn restore_refused_next_turn_rows(&self, rows: Vec<NextTurnRow>, history_len: usize) {
        if rows.is_empty() {
            return;
        }
        let landed = self.agent.state().await.messages;
        let landed = landed.get(history_len..).unwrap_or_default();
        let mut restore: Vec<pa_types::session::CustomMessage> = Vec::with_capacity(rows.len());
        for (row, loop_row) in rows {
            if landed.contains(&loop_row) {
                continue;
            }
            restore.push(row);
        }
        if restore.is_empty() {
            return;
        }
        let mut queue = self
            .pending_next_turn_rows
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        queue.splice(0..0, restore);
    }
}
