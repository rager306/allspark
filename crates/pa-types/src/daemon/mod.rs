//! Daemon wire protocol (TS `daemon-protocol.ts`, `daemon-worker-protocol.ts`): the local JSONL
//! transport between clients (TUI/CLI), the supervisor, and per-session workers; shapes match the
//! TS
//! wire exactly, payloads owned by other subsystems ride as opaque [`Value`]s.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::session::AgentMessage;
use crate::JsonMap;

pub const DAEMON_PROTOCOL_NAME: &str = "prime-agent.daemon";
pub const DAEMON_PROTOCOL_VERSION: u64 = 7;
/// Revision 30 publishes `deletedDescendantUsage` on saved-session rows (landing ahead of TS main:
/// the Rust lifecycle captures the tombstoned child's usage before the unlink).
pub const DAEMON_SCHEMA_REVISION: u64 = 30;
pub const DAEMON_SCHEMA_ID: &str = "protocol-7-schema-30-8e4b17c2a9f5";

pub type DaemonClientId = String;
pub type DaemonCommandId = String;
pub type DaemonEventId = String;
pub type DaemonEventSequence = u64;
/// Client/server capability wire strings (closed TS unions, open on the wire, so raw strings).
pub type DaemonClientCapability = String;
pub type DaemonServerCapability = String;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DaemonProtocolInfo {
    pub name: String,
    pub version: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DaemonEventCursor {
    pub generation: String,
    pub sequence: DaemonEventSequence,
}

/// Resume cursor accepted on attach. The TS wire shape is a union (`DaemonEventCursor`, or a bare
/// `eventSequence`, each with optional `activeSessionId`); this optional-field struct accepts both
/// forms and serializes each back to its original shape.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DaemonResumeCursor {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generation: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sequence: Option<DaemonEventSequence>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event_sequence: Option<DaemonEventSequence>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DaemonReplayStatus {
    Complete,
    Partial,
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DaemonReplayInfo {
    pub status: DaemonReplayStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from_sequence: Option<DaemonEventSequence>,
    pub to_sequence: DaemonEventSequence,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from_cursor: Option<DaemonEventCursor>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to_cursor: Option<DaemonEventCursor>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DaemonEventMeta {
    pub id: DaemonEventId,
    pub protocol: DaemonProtocolInfo,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sequence: Option<DaemonEventSequence>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<DaemonEventCursor>,
    pub emitted_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replayed: Option<bool>,
}

pub mod agent_roster;
mod command;
pub mod framing;
pub mod herdr_env;
pub mod launch_env;
mod outbound;
mod plane;
pub mod update_flow;
mod worker;

pub use command::{
    CycleDirection, DaemonCommand, DaemonCommandEnvelope, DaemonCommandFrameType,
    DaemonCommandWire, DaemonSessionLifecycle, ForkPosition, PromptInput, StreamingBehavior,
    TranscriptPage,
};
pub use outbound::{
    DaemonClosingReason, DaemonErrorInfo, DaemonEventEnvelope, DaemonOutbound,
    DaemonPeerTransportTicket, DaemonResponse, DaemonRuntimeIdentity, DaemonSavedSessionInfo,
    DaemonSessionClosedReason, DaemonSessionSnapshot, SnapshotPurpose, SocketIdentity,
    KERNEL_NOT_RUNNING_MESSAGE, UPDATE_RESTART_PREPARING_MESSAGE,
};
pub use plane::{
    command_plane, is_daemon_mutating_command, is_session_plane_daemon_command,
    is_update_drain_command, DaemonCommandPlane,
};
pub use update_flow::{
    legacy_update_restart_status, legacy_update_restarts_dir, prepared_marker_expiry,
    socket_update_dir, update_intent_path, update_marker_path, update_prepared_dir,
    update_restarts_dir, update_roster_path, update_status_path, update_transition_allowed,
    PreparedMarkerExpiry, UpdateHeartbeatDeliveryMode, UpdateHeartbeatStatus, UpdateId,
    UpdateIntent, UpdatePreparedMarker, UpdateProcessIdentity, UpdateRoster, UpdateRosterBinary,
    UpdateRosterHeartbeat, UpdateRosterInFlight, UpdateRosterQueue, UpdateRosterSession,
    UpdateRosterSessionKind, UpdateRosterSubagent, UpdateRosterSubagentStatus, UpdateRosterWorker,
    UpdateState, UpdateStatus, UpdateStatusCounts, UpdateStatusFailure, UpdateSupervisorIdentity,
    UpdateTimeoutBudget, UPDATE_ENV_PREFIX, UPDATE_ROSTER_FORMAT_VERSION,
    UPDATE_STATUS_FORMAT_VERSION,
};
pub use worker::{
    DaemonPeerCommand, DaemonUpdateRestartManifest, DaemonUpdateRestartQueue,
    DaemonUpdateRestartSession, DaemonWorkerCommand, DaemonWorkerDescriptor,
    DaemonWorkerFrameHeader, DaemonWorkerLifecycle, DaemonWorkerPeerGrant,
    DaemonWorkerRosterOutbound, DurableDaemonCreateCommand, PayloadEncoding,
    DAEMON_UPDATE_RESTART_FORMAT_VERSION,
};

/// Round-trip helper: a parsed type must serialize back to the exact original value.
#[cfg(test)]
pub(crate) fn rt<T: serde::Serialize + for<'de> serde::Deserialize<'de>>(json: &str) {
    let original: serde_json::Value = serde_json::from_str(json).unwrap();
    let parsed: T = serde_json::from_str(json).expect("deserialize");
    let out = serde_json::to_string(&parsed).expect("serialize");
    let reparsed: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(original, reparsed, "round trip changed the value: {out}");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_wire_bare_and_envelope() {
        rt::<DaemonCommandWire>(r#"{"type":"list","id":"c1","cwd":"/w","future":1}"#);
        rt::<DaemonCommandWire>(
            r#"{"type":"command","id":"e1","protocol":{"name":"prime-agent.daemon","version":7},"clientId":"cl","command":{"type":"prompt","activeSessionId":"s1","message":"hi","queueIfBusy":true,"extra":"kept"}}"#,
        );
    }

    #[test]
    fn resume_cursor_and_replay_roundtrip() {
        rt::<DaemonResumeCursor>(r#"{"generation":"g","sequence":3,"activeSessionId":"s"}"#);
        rt::<DaemonResumeCursor>(r#"{"activeSessionId":"s","eventSequence":3}"#);
        rt::<DaemonReplayInfo>(
            r#"{"status":"unavailable","fromSequence":1,"toSequence":5,"fromCursor":{"generation":"g","sequence":1},"toCursor":{"generation":"g","sequence":5},"reason":"event_replay_not_available"}"#,
        );
    }

    #[test]
    fn protocol_constants_match_ts() {
        assert_eq!(DAEMON_PROTOCOL_NAME, "prime-agent.daemon");
        assert_eq!(DAEMON_PROTOCOL_VERSION, 7);
        assert_eq!(DAEMON_SCHEMA_REVISION, 30);
        assert_eq!(DAEMON_SCHEMA_ID, "protocol-7-schema-30-8e4b17c2a9f5");
        assert_eq!(DAEMON_UPDATE_RESTART_FORMAT_VERSION, 1);
    }
}

pub use crate::build_identity::BuildCompat;

/// The daemon-attachment compatibility decision, made in exactly one place.
///
/// Two previous implementations disagreed (`interactive_mode/daemon.rs`
/// compared protocol+schemaId; `daemon_discovery::classify_reachable` compared
/// protocol+schemaId+appVersion), so a TUI silently attached to a daemon built
/// from different code. This is the single authority; both call sites must
/// delegate to it.
///
/// Fail-closed on identity: when either side carries no build stamp, the
/// answer is [`BuildCompat::Unknown`], which callers treat as not-attachable.
#[must_use]
pub fn daemon_attach_compat(
    hello: &HelloFacts,
    own: &OwnFacts,
) -> crate::build_identity::BuildCompat {
    let protocol_matches = hello.protocol_version == Some(own.protocol_version);
    let schema_matches = hello.schema_id.as_deref() == Some(own.schema_id);
    let version_matches = hello.app_version == own.app_version;
    if protocol_matches && schema_matches && version_matches {
        crate::build_identity::build_compatible(hello.build_hash.clone(), own.build_hash.clone())
    } else {
        BuildCompat::Mismatch
    }
}

/// The facts about one side, extracted from a `daemon_hello`-shaped payload.
#[derive(Debug, Clone, Default)]
pub struct HelloFacts {
    pub protocol_version: Option<u64>,
    pub schema_id: Option<String>,
    pub app_version: Option<String>,
    pub build_hash: Option<String>,
}

/// This process's own facts, as the composition root knows them.
#[derive(Debug, Clone)]
pub struct OwnFacts {
    pub protocol_version: u64,
    pub schema_id: &'static str,
    pub app_version: Option<String>,
    pub build_hash: Option<String>,
}

#[cfg(test)]
mod attach_compat_tests {
    use super::{
        daemon_attach_compat, HelloFacts, OwnFacts, DAEMON_PROTOCOL_VERSION, DAEMON_SCHEMA_ID,
    };

    fn own(commit: &str) -> OwnFacts {
        OwnFacts {
            protocol_version: DAEMON_PROTOCOL_VERSION,
            schema_id: DAEMON_SCHEMA_ID,
            app_version: Some("0.10.2-beta.3".to_string()),
            build_hash: Some(commit.to_string()),
        }
    }

    fn hello(commit: &str, version: &str) -> HelloFacts {
        HelloFacts {
            protocol_version: Some(DAEMON_PROTOCOL_VERSION),
            schema_id: Some(DAEMON_SCHEMA_ID.to_string()),
            app_version: Some(version.to_string()),
            build_hash: Some(commit.to_string()),
        }
    }

    #[test]
    fn the_incident_shape_is_a_mismatch() {
        // Old daemon (yesterday's build) against today's client: same protocol,
        // same schema, different commit -> not attachable.
        let verdict = daemon_attach_compat(
            &hello("559751fdcec0", "0.10.1-beta.35"),
            &own("01de64db25e5"),
        );
        assert!(!matches!(
            verdict,
            crate::build_identity::BuildCompat::Match
        ));
    }

    #[test]
    fn same_commit_different_version_is_a_mismatch() {
        // The silent-empty-list class: same code lineage, drifting version file.
        let verdict = daemon_attach_compat(
            &hello("559751fdcec0", "0.10.1-beta.35"),
            &own("559751fdcec0"),
        );
        assert!(!matches!(
            verdict,
            crate::build_identity::BuildCompat::Match
        ));
    }

    #[test]
    fn an_unstamped_side_is_unknown_fail_closed() {
        let mut unstamped = hello("559751fdcec0", "0.10.2-beta.3");
        unstamped.build_hash = None;
        let verdict = daemon_attach_compat(&unstamped, &own("01de64db25e5"));
        assert!(!matches!(
            verdict,
            crate::build_identity::BuildCompat::Match
        ));
    }

    #[test]
    fn a_protocol_drift_beats_everything() {
        let mut drifted = hello("01de64db25e5", "0.10.2-beta.3");
        drifted.protocol_version = Some(DAEMON_PROTOCOL_VERSION - 1);
        let verdict = daemon_attach_compat(&drifted, &own("01de64db25e5"));
        assert!(!matches!(
            verdict,
            crate::build_identity::BuildCompat::Match
        ));
    }

    #[test]
    fn an_identical_build_matches() {
        let verdict = daemon_attach_compat(
            &hello("01de64db25e5", "0.10.2-beta.3"),
            &own("01de64db25e5"),
        );
        assert!(matches!(verdict, crate::build_identity::BuildCompat::Match));
    }
}
