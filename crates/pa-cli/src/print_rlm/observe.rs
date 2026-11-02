//! The `agent_observe.*` controller for an inline print session: the nuclear
//! family roster and the transcript previews, read off the same in-process node
//! tree the messaging controller drives.

use std::sync::{Arc, Weak};

use pa_core::session_engine::agent_messaging::{
    create_agent_observe_message_preview, AgentFamilyRelationship, AgentFamilyStatus,
    AgentObserveActivity, AgentObserveController, AgentObserveListResult,
    AgentObserveRecentMessagesResult, AgentObserveSummary, AGENT_OBSERVE_PREVIEW_MAX_CHARS,
};

use super::{has_unsettled_rlm_work, InlineSessionNode};

/// The family roster and transcript-preview controller over the same node
/// tree the messaging controller drives: the caller itself, its parent, its
/// siblings, and its direct children.
///
/// The daemon answers `agent_observe.*` from its supervisor roster — a
/// cross-process walk in which a member may have no resident session. An
/// inline print session has no supervisor: the in-process node tree IS the
/// roster, and every row is resident by construction. Two facts have no
/// inline source and stay at their resting value: `isCompacting` (the print
/// boundary keeps no compaction flag) and `attachedClients` (no client
/// attaches to an in-process session).
pub(super) struct InlineObserve(Arc<InlineSessionNode>);

/// One family member as the tree traversal finds it.
struct FamilyNode {
    node: Arc<InlineSessionNode>,
    /// `None` for the caller itself.
    relationship: Option<AgentFamilyRelationship>,
    /// The node's child id under its parent, when a child handle carries one.
    rlm_child_id: Option<String>,
}

/// The live facts one observation row reports.
struct NodeActivity {
    /// The family lifecycle: `inactive` for a disposed session, `running`
    /// while any work is in flight (its descendants' included), else `idle`.
    status: AgentFamilyStatus,
    /// The live activity axis (TS #2493): what the session is doing right now,
    /// as distinct from its family lifecycle status.
    activity: AgentObserveActivity,
    /// The session's OWN work in flight — the daemon's `isSessionActive`, which
    /// deliberately excludes delegated child work.
    own_work: bool,
    streaming: bool,
    /// Queued steer/follow-up batches in the session's own lanes.
    queued: usize,
}

/// One session's transcript as the observation types read it.
#[derive(Default)]
struct Transcript {
    /// The conversation rows the previews read, in persistence order.
    rows: Vec<pa_types::session::AgentMessage>,
    /// The persisted `message`-entry count, the daemon's `messageCount`.
    message_count: u64,
}

impl InlineObserve {
    pub(super) fn new(node: Arc<InlineSessionNode>) -> Self {
        InlineObserve(node)
    }

    /// The caller plus its nuclear family, in the messaging family's order:
    /// the caller, the parent, the siblings by name, then the children.
    async fn family_nodes(&self) -> Vec<FamilyNode> {
        let node = &self.0;
        let mut siblings = Vec::new();
        let mut own_child_id = None;
        let mut parent = None;
        if let Some(parent_node) = node.parent_node() {
            for sibling in parent_node.children.lock().await.iter() {
                if Arc::ptr_eq(&sibling.node, node) {
                    own_child_id = Some(sibling.rlm_child_id.clone());
                    continue;
                }
                siblings.push(FamilyNode {
                    node: Arc::clone(&sibling.node),
                    relationship: Some(AgentFamilyRelationship::Sibling),
                    rlm_child_id: Some(sibling.rlm_child_id.clone()),
                });
            }
            parent = Some(FamilyNode {
                node: parent_node,
                relationship: Some(AgentFamilyRelationship::Parent),
                rlm_child_id: None,
            });
        }
        let mut children = Vec::new();
        for child in node.children.lock().await.iter() {
            children.push(FamilyNode {
                node: Arc::clone(&child.node),
                relationship: Some(AgentFamilyRelationship::Child),
                rlm_child_id: Some(child.rlm_child_id.clone()),
            });
        }
        sort_family_nodes(&mut siblings);
        sort_family_nodes(&mut children);
        let mut family = vec![FamilyNode {
            node: Arc::clone(node),
            relationship: None,
            rlm_child_id: own_child_id,
        }];
        family.extend(parent);
        family.extend(siblings);
        family.extend(children);
        family
    }

    /// One family member by session id, session name, or child id. An
    /// ambiguous selector is an error, never a silent pick.
    async fn find(&self, target: &str) -> anyhow::Result<Option<FamilyNode>> {
        let mut matches: Vec<FamilyNode> = self
            .family_nodes()
            .await
            .into_iter()
            .filter(|member| family_node_matches(member, target))
            .collect();
        match matches.len() {
            0 => Ok(None),
            1 => Ok(matches.pop()),
            _ => {
                let rendered = matches
                    .iter()
                    .map(|member| member.node.session_id.clone())
                    .collect::<Vec<_>>()
                    .join(", ");
                anyhow::bail!("Ambiguous family selector \"{target}\": matches {rendered}")
            }
        }
    }
}

impl AgentObserveController for InlineObserve {
    async fn list_agents(&self) -> anyhow::Result<AgentObserveListResult> {
        let mut agents = Vec::new();
        let mut current = None;
        for member in self.family_nodes().await {
            let summary = observe_summary(&member).await;
            if member.relationship.is_none() {
                current = Some(summary);
            } else {
                agents.push(summary);
            }
        }
        Ok(AgentObserveListResult {
            current: current.expect("the family walk always yields the caller's own row"),
            agents,
        })
    }

    async fn get_agent(&self, target: &str) -> anyhow::Result<Option<AgentObserveSummary>> {
        Ok(match self.find(target).await? {
            Some(member) => Some(observe_summary(&member).await),
            None => None,
        })
    }

    async fn recent_messages(
        &self,
        target: &str,
        limit: usize,
        max_chars: usize,
    ) -> anyhow::Result<AgentObserveRecentMessagesResult> {
        let member = self
            .find(target)
            .await?
            .ok_or_else(|| anyhow::anyhow!("agent {target} is not reachable"))?;
        let transcript = observe_transcript(&member.node).await?;
        let rows = &transcript.rows;
        let start = rows.len().saturating_sub(limit);
        let messages = rows
            .iter()
            .enumerate()
            .skip(start)
            .map(|(index, row)| create_agent_observe_message_preview(row, index, max_chars))
            .collect::<Vec<_>>();
        Ok(AgentObserveRecentMessagesResult {
            agent: observe_summary(&member).await,
            messages,
            limit,
            max_chars,
            truncated: start > 0,
        })
    }
}

/// Sort family rows by the name the roster shows (the recorded session name,
/// else the id) — the messaging family's own order.
fn sort_family_nodes(nodes: &mut [FamilyNode]) {
    nodes.sort_by_cached_key(|member| {
        member
            .node
            .session_name()
            .unwrap_or_else(|| member.node.session_id.clone())
    });
}

/// Whether a family row answers `target`: its child id, session id, or session
/// name exactly, or a dash-insensitive id/name suffix — the daemon registry's
/// selector contract.
fn family_node_matches(member: &FamilyNode, target: &str) -> bool {
    let name = member.node.session_name();
    if member.node.session_id == target
        || name.as_deref() == Some(target)
        || member.rlm_child_id.as_deref() == Some(target)
    {
        return true;
    }
    let normalize = |value: &str| -> String { value.replace('-', "").to_lowercase() };
    let suffix = normalize(target);
    !suffix.is_empty()
        && (normalize(&member.node.session_id).ends_with(&suffix)
            || name.is_some_and(|name| normalize(&name).ends_with(&suffix)))
}

/// One node's live facts: the loop state plus everything the quiescence
/// predicate already folds (queued rows, receipts, descendants, background
/// bash).
async fn node_activity(node: &Arc<InlineSessionNode>) -> NodeActivity {
    let mut streaming = false;
    let mut running_tools = false;
    let mut queued = 0;
    let mut own_work = false;
    if let Some(engine) = node.engine.get().and_then(Weak::upgrade) {
        let agent = engine.session.agent();
        let state = agent.state().await;
        streaming = state.is_streaming;
        running_tools = !state.pending_tool_calls.is_empty();
        queued = agent.steering_previews().len() + agent.follow_up_previews().len();
        // The session's own work: a streaming turn, a row waiting in its lane,
        // or live background bash — the engine arm of the quiescence predicate
        // without its children arm.
        own_work = streaming || agent.has_queued_messages() || engine.has_background_bash();
    }
    // The family status reads the WHOLE subtree (`a session whose subagents
    // still run reports working`); the activity axis reads the session alone.
    let unsettled = has_unsettled_rlm_work(node).await;
    let status = if node.is_disposed() {
        AgentFamilyStatus::Inactive
    } else if unsettled || streaming {
        AgentFamilyStatus::Running
    } else {
        AgentFamilyStatus::Idle
    };
    // The daemon's own classification over the facts an inline node has: tool
    // work beats a bare stream, any work in flight is `busy`, and with no
    // attached client and no compaction flag the resting verdict is idle.
    let activity = if streaming && running_tools {
        AgentObserveActivity::Tool
    } else if streaming {
        AgentObserveActivity::Model
    } else if own_work || queued > 0 || unsettled {
        AgentObserveActivity::Busy
    } else {
        AgentObserveActivity::Idle
    };
    NodeActivity {
        status,
        activity,
        own_work,
        streaming,
        queued,
    }
}

/// One node's transcript as the observe types read it: the session's own
/// entries, in persistence order.
///
/// This is what the daemon's `get_messages` request serves, read in process: a
/// live engine hands over its retained entries, and a node whose engine is not
/// published (or already disposed) reads its durable file. Both sides fold
/// through [`entry_message`], so the previews and the message count agree
/// whichever source answered.
async fn observe_transcript(node: &Arc<InlineSessionNode>) -> anyhow::Result<Transcript> {
    let entries = if let Some(engine) = node.engine.get().and_then(Weak::upgrade) {
        engine.session.entries().await
    } else {
        // No engine is published yet (the build is in flight) or the session
        // never had one. A persisted session still answers from its own file;
        // an in-memory one has no transcript to read — the daemon's "no live
        // session" verdict for a member with nothing resident.
        let Some(path) = &node.session_file else {
            anyhow::bail!(
                "agent {} has no live session in this print process",
                node.session_id
            );
        };
        pa_core::session::manager::load_entries_from_file(std::path::Path::new(path), false)
    };
    let mut message_count = 0;
    let mut rows = Vec::new();
    for entry in entries {
        if matches!(entry, pa_types::session::FileEntry::Message { .. }) {
            message_count += 1;
        }
        if let Some(row) = entry_message(entry) {
            rows.push(row);
        }
    }
    Ok(Transcript {
        rows,
        message_count,
    })
}

/// The conversation rows one persisted entry contributes (the daemon's
/// `get_messages` fold: `message` rows as-is, `custom_message` rows rejoined as
/// custom messages).
fn entry_message(entry: pa_types::session::FileEntry) -> Option<pa_types::session::AgentMessage> {
    match entry {
        pa_types::session::FileEntry::Message { message, .. } => Some(message),
        pa_types::session::FileEntry::CustomMessage { payload, base } => Some(
            pa_types::session::AgentMessage::Custom(pa_types::session::CustomMessage {
                custom_type: payload.custom_type,
                content: payload.content,
                display: payload.display,
                details: payload.details,
                timestamp: pa_core::session::timestamp_to_millis(
                    base.timestamp.as_deref().unwrap_or_default(),
                ),
                rest: pa_types::JsonMap::new(),
            }),
        ),
        _ => None,
    }
}

/// The first non-empty user row's text, the identity signal a family row
/// carries (TS `firstMessage`), preview-capped.
fn first_message_text(rows: &[pa_types::session::AgentMessage]) -> Option<String> {
    rows.iter().find_map(|row| match row {
        pa_types::session::AgentMessage::User(message) => {
            let text = message.content.text();
            (!text.is_empty()).then(|| text.chars().take(AGENT_OBSERVE_PREVIEW_MAX_CHARS).collect())
        }
        _ => None,
    })
}

/// One family member's observation summary.
async fn observe_summary(member: &FamilyNode) -> AgentObserveSummary {
    let node = &member.node;
    let activity = node_activity(node).await;
    // A row's summary never fails on a missing transcript: the list and get
    // requests answer with what the tree holds, and only `recent_messages`
    // reports an unreadable session.
    let transcript = observe_transcript(node).await.unwrap_or_default();
    let rows = &transcript.rows;
    let parent = node.parent_node();
    AgentObserveSummary {
        active_session_id: Some(node.session_id.clone()),
        session_id: node.session_id.clone(),
        session_name: node.session_name().filter(|name| !name.is_empty()),
        relationship: member.relationship,
        runtime_kind: Some(
            if node.depth == 0 {
                "top-level"
            } else {
                "subagent"
            }
            .to_string(),
        ),
        cwd: Some(node.recipe.options.config.cwd.display().to_string()),
        status: activity.status,
        activity: (activity.status != AgentFamilyStatus::Inactive).then_some(activity.activity),
        is_current: member.relationship.is_none(),
        is_streaming: activity.streaming,
        // The print boundary keeps no compaction flag; see the type's note.
        is_compacting: false,
        attached_clients: 0,
        message_count: Some(transcript.message_count),
        queued_count: activity.queued,
        is_session_active: activity.own_work,
        // The daemon distinguishes a parent's live `activeSessionId` from its
        // durable `sessionId`; an inline node has one id, so both fields carry
        // it.
        parent_active_session_id: parent.as_ref().map(|parent| parent.session_id.clone()),
        parent_session_id: parent.as_ref().map(|parent| parent.session_id.clone()),
        rlm_child_id: member.rlm_child_id.clone(),
        first_message: first_message_text(rows),
        // An inline node's transcript is resolved fresh on every observe, so the
        // row carries no file path; `recent_messages` here reads the tree.
        session_file: None,
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::run_options;
    use super::super::*;
    use serde_json::json;

    /// The inline wiring must register the WHOLE family surface the static
    /// core layer advertises for "a print session with inline subagents":
    /// `agent_observe.list_agents()` in such a session used to fail with
    /// "no handler", and the `agent-observe` skill dropped out of the
    /// inventory with it.
    #[tokio::test]
    async fn inline_print_sessions_register_the_agent_observe_surface() {
        let directory = tempfile::tempdir().unwrap();
        let options = run_options(directory.path());
        let manager = pa_core::session::manager::SessionManager::in_memory(directory.path());
        let node = root_node(&options, &manager, None);
        let (_host, handlers) = inline_subagent_wiring(&node);
        for request in [
            "agent_observe.list",
            "agent_observe.get",
            "agent_observe.recent",
        ] {
            assert!(
                handlers.get(request).is_some(),
                "the inline print wiring must register {request}"
            );
        }
        let list = handlers
            .get("agent_observe.list")
            .expect("registered above")
            .clone();
        let response = list(pa_core::kernel::shared::HostRequestPayload { data: json!({}) })
            .await
            .expect("the inline roster answers");
        assert_eq!(response["current"]["activeSessionId"], node.session_id);
        assert_eq!(response["current"]["isCurrent"], true);
        assert_eq!(response["current"]["relationship"], Value::Null);
        assert_eq!(
            response["agents"],
            json!([]),
            "an unfamilied root has no rows"
        );
    }

    /// The inline roster is the node tree: the caller, its parent, its
    /// siblings, and its children — addressed by session id, session name, or
    /// child id, exactly like the daemon's supervisor roster.
    #[tokio::test]
    async fn inline_observe_lists_the_nuclear_family_from_the_node_tree() {
        let directory = tempfile::tempdir().unwrap();
        let options = run_options(directory.path());
        std::fs::create_dir_all(&options.config.agent_dir).unwrap();
        let manager = pa_core::session::manager::SessionManager::persisted(
            directory.path(),
            &directory.path().join("sessions"),
        );
        let script = json!({
            "responses": ["parent done"],
            "children": {
                "kid": { "responses": ["kid done"] },
                "twin": { "responses": ["twin done"] },
            },
        });
        let node = root_node(&options, &manager, Some(script.clone()));
        let headless = crate::print_runtime::build_engine(
            &options,
            Some(&script),
            Some(manager),
            "print",
            &HeadlessEmbedding::Root {
                node: Arc::clone(&node),
            },
        )
        .await
        .unwrap();
        let engine = Arc::new(headless.engine);
        adopt_built_engine(&node, &engine, &headless.model).await;

        let spawned = InlineHost(Arc::clone(&node))
            .spawn(RlmSpawnRequest {
                prompt: "run the kid task".to_string(),
                name: Some("kid".to_string()),
                model: None,
                thinking: None,
                cwd: None,
                spawned_by_request_id: None,
                factory: None,
            })
            .await
            .expect("the inline host admits a child");
        assert_eq!(spawned.name, "kid");
        // A second child, so the sibling arm of the roster has a subject.
        InlineHost(Arc::clone(&node))
            .spawn(RlmSpawnRequest {
                prompt: "run the twin task".to_string(),
                name: Some("twin".to_string()),
                model: None,
                thinking: None,
                cwd: None,
                spawned_by_request_id: None,
                factory: None,
            })
            .await
            .expect("the inline host admits a second child");
        let kid = node
            .children
            .lock()
            .await
            .first()
            .cloned()
            .expect("the admitted child is on the parent's roster");

        // Both drivers settle before the transcript is read: the spawn kickoff
        // row and the child's scripted turn are then durable, so the preview
        // window below has a fixed transcript.
        let settled = InlineHost(Arc::clone(&node))
            .collect(Vec::new(), 30_000)
            .await
            .expect("the scripted children settle");
        assert_eq!(settled.len(), 2, "both children settled");
        assert!(settled.iter().all(|child| child.settled), "{settled:?}");

        // Two known durable rows on the child's own session, the second long
        // enough to prove the preview cap.
        let long_row = "L".repeat(400);
        for text in ["hello from the kid".to_string(), long_row.clone()] {
            kid.engine
                .session
                .shared_persistence()
                .lock()
                .await
                .append_message(pa_types::session::AgentMessage::User(
                    pa_types::ai::UserMessage {
                        content: pa_types::ai::UserContent::Text(text),
                        timestamp: 1,
                        rest: pa_types::JsonMap::new(),
                    },
                ))
                .unwrap();
        }

        // The wiring restores the `agent-observe` skill: the model-visible
        // inventory only carries a controller-backed skill whose host request
        // registered, so before this wiring both the handler and the skill
        // were missing from a print session with inline subagents.
        assert!(
            engine.system_prompt.contains("<name>agent-observe</name>"),
            "the print prompt lists agent-observe once its handlers are wired"
        );

        // The parent's view: itself plus its child.
        let (_host, handlers) = inline_subagent_wiring(&node);
        let list = handlers
            .get("agent_observe.list")
            .expect("registered")
            .clone();
        let listed = list(pa_core::kernel::shared::HostRequestPayload { data: json!({}) })
            .await
            .expect("the inline roster answers");
        assert_eq!(listed["current"]["activeSessionId"], node.session_id);
        assert_eq!(listed["current"]["runtimeKind"], "top-level");
        let rows = listed["agents"].as_array().expect("family rows");
        let names: Vec<&str> = rows
            .iter()
            .map(|row| row["sessionName"].as_str().unwrap_or_default())
            .collect();
        assert_eq!(names, ["kid", "twin"], "children are named and ordered");
        assert_eq!(rows[0]["activeSessionId"], kid.node.session_id);
        assert_eq!(rows[0]["sessionId"], kid.node.session_id);
        assert_eq!(rows[0]["relationship"], "child");
        assert_eq!(rows[0]["rlmChildId"], kid.rlm_child_id);
        assert_eq!(rows[0]["parentActiveSessionId"], node.session_id);
        assert_eq!(rows[0]["runtimeKind"], "subagent");
        assert_eq!(rows[0]["isCurrent"], false);
        // `messageCount` counts persisted `message` rows (the spawn kickoff is
        // a custom_message, not one), and the identity preview is the first
        // non-empty USER row.
        assert_eq!(rows[0]["messageCount"], 3, "{rows:?}");
        assert_eq!(rows[0]["firstMessage"], "hello from the kid");
        assert_eq!(rows[0]["cwd"], directory.path().to_string_lossy().as_ref());

        // The child's view: itself, its parent, and its sibling — every
        // selector form.
        let (_child_host, child_handlers) = inline_subagent_wiring(&kid.node);
        let child_list = child_handlers
            .get("agent_observe.list")
            .expect("registered")
            .clone();
        let child_listed =
            child_list(pa_core::kernel::shared::HostRequestPayload { data: json!({}) })
                .await
                .expect("the child's roster answers");
        assert_eq!(child_listed["current"]["sessionName"], "kid");
        assert_eq!(child_listed["current"]["relationship"], Value::Null);
        let kin = child_listed["agents"].as_array().expect("family rows");
        let kin: Vec<(&str, &str)> = kin
            .iter()
            .map(|row| {
                (
                    row["relationship"].as_str().unwrap_or_default(),
                    row["sessionName"].as_str().unwrap_or_default(),
                )
            })
            .collect();
        assert_eq!(kin, [("parent", ""), ("sibling", "twin")], "{kin:?}");
        // The parent is the root print session: no recorded name, no parent
        // edge of its own.
        assert_eq!(
            child_listed["agents"][0]["activeSessionId"],
            node.session_id
        );
        assert_eq!(child_listed["agents"][0]["sessionName"], Value::Null);
        assert_eq!(
            child_listed["agents"][0]["parentActiveSessionId"],
            Value::Null
        );

        let get = handlers
            .get("agent_observe.get")
            .expect("registered")
            .clone();
        for target in [
            "kid",
            kid.rlm_child_id.as_str(),
            kid.node.session_id.as_str(),
        ] {
            let found = get(pa_core::kernel::shared::HostRequestPayload {
                data: json!({ "target": target }),
            })
            .await
            .unwrap_or_else(|error| panic!("{target}: {error:#}"));
            assert_eq!(
                found["agent"]["activeSessionId"], kid.node.session_id,
                "{target}"
            );
        }
        let stranger = get(pa_core::kernel::shared::HostRequestPayload {
            data: json!({ "target": "no-such-agent" }),
        })
        .await
        .expect_err("an unknown selector is an error");
        assert!(
            stranger.to_string().contains("not reachable"),
            "{stranger:#}"
        );

        // The transcript preview reads the child's own session.
        let recent = handlers
            .get("agent_observe.recent")
            .expect("registered")
            .clone();
        let preview = recent(pa_core::kernel::shared::HostRequestPayload {
            data: json!({ "target": "kid", "limit": 5, "max_chars": 100 }),
        })
        .await
        .expect("the child's transcript answers");
        assert_eq!(preview["agent"]["activeSessionId"], kid.node.session_id);
        assert_eq!(preview["limit"], 5);
        assert_eq!(preview["maxChars"], 100);
        let messages = preview["messages"].as_array().expect("preview rows");
        let rows: Vec<(&str, &str, bool)> = messages
            .iter()
            .map(|row| {
                (
                    row["role"].as_str().unwrap_or_default(),
                    row["text"].as_str().unwrap_or_default(),
                    row["truncated"].as_bool().unwrap_or_default(),
                )
            })
            .collect();
        // The child's durable transcript, in persistence order: the spawn
        // kickoff row, the scripted turn's answer, the "hello" row, then the
        // long one — whose 400 chars the preview cap cuts to 100 and REPORTS
        // as cut, so the model never reads a shortened row as the whole
        // message.
        assert_eq!(
            rows,
            [
                ("custom", "[task from parent]\n\nrun the kid task", false),
                ("assistant", "kid done", false),
                ("user", "hello from the kid", false),
                ("user", &long_row[..100], true),
            ]
        );
        assert_eq!(messages[0]["customType"], "agent_message");
        assert_eq!(preview["truncated"], false);

        // A window smaller than the transcript reports the older rows as cut.
        let windowed = recent(pa_core::kernel::shared::HostRequestPayload {
            data: json!({ "target": "kid", "limit": 1 }),
        })
        .await
        .expect("the window answers");
        assert_eq!(windowed["truncated"], true);
        assert_eq!(windowed["messages"].as_array().map(Vec::len), Some(1));
        assert_eq!(windowed["maxChars"], 800);

        dispose_inline_subagents(&node).await;
        engine.dispose_kernel().await;
    }

    /// A session with no engine and no session file is the one row an inline
    /// print process cannot read: `list_agents`/`get_agent` still answer from
    /// the tree, and `recent_messages` names the reason instead of reporting an
    /// empty transcript as fact.
    #[tokio::test]
    async fn inline_observe_reports_an_unreadable_transcript_explicitly() {
        let directory = tempfile::tempdir().unwrap();
        let options = run_options(directory.path());
        let manager = pa_core::session::manager::SessionManager::in_memory(directory.path());
        let node = root_node(&options, &manager, None);
        let (_host, handlers) = inline_subagent_wiring(&node);
        let list = handlers
            .get("agent_observe.list")
            .expect("registered")
            .clone();
        let listed = list(pa_core::kernel::shared::HostRequestPayload { data: json!({}) })
            .await
            .expect("the roster answers without a transcript");
        assert_eq!(listed["current"]["activeSessionId"], node.session_id);
        assert_eq!(listed["current"]["messageCount"], 0);
        assert_eq!(listed["current"]["status"], "idle");
        assert_eq!(listed["current"]["activity"], "idle");
        assert_eq!(listed["current"]["isSessionActive"], false);
        assert_eq!(listed["current"]["attachedClients"], 0);

        let recent = handlers
            .get("agent_observe.recent")
            .expect("registered")
            .clone();
        let error = recent(pa_core::kernel::shared::HostRequestPayload {
            data: json!({ "target": node.session_id }),
        })
        .await
        .expect_err("an in-memory session with no engine has no transcript");
        assert!(error.to_string().contains("no live session"), "{error:#}");
    }
}
