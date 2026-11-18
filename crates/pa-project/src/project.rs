//! The projector: applies the event log to the read model.
//!
//! Application is idempotent and resumable. Every event is recorded in the
//! `applied_events` ledger by id, so a replay after a crash skips what already
//! landed; `projection_state` records the checkpoint the projector resumes from.

use anyhow::{Context, Result};
use rusqlite::{OptionalExtension, params};
use serde_json::Value;

use crate::event::Event;
use crate::model::ReadModel;

/// What one projection pass did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ProjectionReport {
    /// Events applied in this pass.
    pub applied: usize,
    /// Events skipped because they were already applied.
    pub skipped: usize,
}

impl ProjectionReport {
    /// Whether the pass changed anything.
    #[must_use]
    pub fn changed(&self) -> bool {
        self.applied > 0
    }
}

/// Apply every unapplied event to `model`, in log order.
///
/// Each event applies in its own transaction together with its ledger row, so
/// a crash between events leaves the model exactly at the last applied one and
/// a replay resumes there. An event whose type this build does not know is
/// recorded as applied and left out of the model: the log keeps it for a later
/// reader, and silently dropping it from the ledger would replay it forever.
///
/// # Errors
///
/// Returns an error when a transaction fails. A failed event stops the pass at
/// the previous checkpoint.
pub fn project<'a>(
    model: &mut ReadModel,
    project_id: &str,
    events: impl IntoIterator<Item = &'a Event>,
) -> Result<ProjectionReport> {
    let mut report = ProjectionReport::default();
    let mut seq: i64 = model.last_seq(project_id)?;
    for event in events {
        if event.project_id != project_id {
            continue;
        }
        seq += 1;
        if model.already_applied(&event.event_id)? {
            report.skipped += 1;
            seq = model.applied_seq(&event.event_id)?.unwrap_or(seq);
            continue;
        }
        model.apply(event, seq).with_context(|| {
            format!(
                "apply event {} ({}) at seq {seq}",
                event.event_id, event.event_type
            )
        })?;
        report.applied += 1;
    }
    Ok(report)
}

impl ReadModel {
    /// The last sequence number the projector recorded for `project_id`.
    pub(crate) fn last_seq(&self, project_id: &str) -> Result<i64> {
        Ok(self
            .conn()
            .query_row(
                "SELECT last_seq FROM projection_state WHERE project_id = ?1",
                params![project_id],
                |row| row.get(0),
            )
            .optional()?
            .unwrap_or(0))
    }

    /// Whether `event_id` is already in the ledger.
    pub(crate) fn already_applied(&self, event_id: &str) -> Result<bool> {
        Ok(self
            .conn()
            .query_row(
                "SELECT 1 FROM applied_events WHERE event_id = ?1",
                params![event_id],
                |_| Ok(()),
            )
            .optional()?
            .is_some())
    }

    /// The sequence number an applied event was recorded at.
    pub(crate) fn applied_seq(&self, event_id: &str) -> Result<Option<i64>> {
        Ok(self
            .conn()
            .query_row(
                "SELECT seq FROM applied_events WHERE event_id = ?1",
                params![event_id],
                |row| row.get(0),
            )
            .optional()?)
    }

    /// Apply one event and its ledger row in a single transaction.
    fn apply(&mut self, event: &Event, seq: i64) -> Result<()> {
        let tx = self.conn_mut().transaction()?;
        tx.execute(
            "INSERT INTO applied_events(event_id, seq, event_type, schema_version, occurred_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                event.event_id,
                seq,
                event.event_type,
                event.schema_version,
                event.occurred_at
            ],
        )?;
        tx.execute(
            "INSERT INTO projection_state(project_id, last_event_id, last_seq, schema_version, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(project_id) DO UPDATE SET
                last_event_id = excluded.last_event_id,
                last_seq = excluded.last_seq,
                schema_version = excluded.schema_version,
                updated_at = excluded.updated_at",
            params![
                event.project_id,
                event.event_id,
                seq,
                event.schema_version,
                event.occurred_at
            ],
        )?;
        apply_entity_event(&tx, event)?;
        tx.commit()?;
        Ok(())
    }
}

/// Apply the entity-shaped event kinds.
///
/// A type this build does not know applies nothing: the ledger row already
/// recorded it, so the log keeps the record for a build that does.
fn apply_entity_event(tx: &rusqlite::Transaction<'_>, event: &Event) -> Result<()> {
    match event.event_type.as_str() {
        "entity.upserted" => apply_entity_upsert(tx, event),
        "entity.removed" => apply_entity_removal(tx, event),
        "relation.added" => apply_relation_added(tx, event),
        "relation.removed" => apply_relation_removed(tx, event),
        "ref.added" => apply_ref_added(tx, event),
        "ref.removed" => apply_ref_removed(tx, event),
        _ => Ok(()),
    }
}

/// A missing field means the event cannot describe its row; the ledger row
/// already holds the event, so the pass is not blocked by it.
fn field<'a>(payload: &'a Value, name: &str) -> Option<&'a str> {
    payload.get(name).and_then(Value::as_str)
}

fn apply_entity_upsert(tx: &rusqlite::Transaction<'_>, event: &Event) -> Result<()> {
    let Some(id) = field(&event.payload, "id") else {
        return Ok(());
    };
    let type_name = field(&event.payload, "entityType").unwrap_or_default();
    let data = event
        .payload
        .get("data")
        .cloned()
        .unwrap_or_else(|| Value::Object(serde_json::Map::new()));
    tx.execute(
        "INSERT INTO entities(id, project_id, type, data_json, version, deleted, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, 1, 0, ?5, ?5)
         ON CONFLICT(id) DO UPDATE SET
            type = excluded.type,
            data_json = excluded.data_json,
            version = entities.version + 1,
            deleted = 0,
            updated_at = excluded.updated_at",
        params![
            id,
            event.project_id,
            type_name,
            data.to_string(),
            event.occurred_at
        ],
    )?;
    Ok(())
}

fn apply_entity_removal(tx: &rusqlite::Transaction<'_>, event: &Event) -> Result<()> {
    let Some(id) = field(&event.payload, "id") else {
        return Ok(());
    };
    tx.execute(
        "UPDATE entities SET deleted = 1, version = version + 1, updated_at = ?1 WHERE id = ?2",
        params![event.occurred_at, id],
    )?;
    Ok(())
}

fn apply_relation_added(tx: &rusqlite::Transaction<'_>, event: &Event) -> Result<()> {
    let payload = &event.payload;
    let (Some(id), Some(from_id), Some(predicate), Some(to_id)) = (
        field(payload, "id"),
        field(payload, "from"),
        field(payload, "predicate"),
        field(payload, "to"),
    ) else {
        return Ok(());
    };
    let scope = field(payload, "scope").unwrap_or("local");
    tx.execute(
        "INSERT INTO relations(id, project_id, from_id, predicate, to_id, scope, data_json)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, '{}')
         ON CONFLICT(id) DO UPDATE SET
            from_id = excluded.from_id,
            predicate = excluded.predicate,
            to_id = excluded.to_id,
            scope = excluded.scope",
        params![id, event.project_id, from_id, predicate, to_id, scope],
    )?;
    Ok(())
}

fn apply_relation_removed(tx: &rusqlite::Transaction<'_>, event: &Event) -> Result<()> {
    if let Some(id) = field(&event.payload, "id") {
        tx.execute("DELETE FROM relations WHERE id = ?1", params![id])?;
    }
    Ok(())
}

fn apply_ref_added(tx: &rusqlite::Transaction<'_>, event: &Event) -> Result<()> {
    let payload = &event.payload;
    let (Some(id), Some(from_id), Some(predicate), Some(target_db), Some(target_key)) = (
        field(payload, "id"),
        field(payload, "from"),
        field(payload, "predicate"),
        field(payload, "targetDb"),
        field(payload, "targetKey"),
    ) else {
        return Ok(());
    };
    let target_type = field(payload, "targetType").unwrap_or_default();
    let resolver = field(payload, "resolver").unwrap_or_default();
    let status = field(payload, "status").unwrap_or("pending");
    tx.execute(
        "INSERT INTO entity_refs(id, project_id, from_id, predicate, target_db, target_type,
                                 target_key, status, resolver, data_json)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, '{}')
         ON CONFLICT(id) DO UPDATE SET
            from_id = excluded.from_id,
            predicate = excluded.predicate,
            target_db = excluded.target_db,
            target_type = excluded.target_type,
            target_key = excluded.target_key,
            status = excluded.status,
            resolver = excluded.resolver",
        params![
            id,
            event.project_id,
            from_id,
            predicate,
            target_db,
            target_type,
            target_key,
            status,
            resolver
        ],
    )?;
    Ok(())
}

fn apply_ref_removed(tx: &rusqlite::Transaction<'_>, event: &Event) -> Result<()> {
    if let Some(id) = field(&event.payload, "id") {
        tx.execute("DELETE FROM entity_refs WHERE id = ?1", params![id])?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::event::{Actor, Event, SCHEMA_VERSION};
    use crate::model::ReadModel;

    const PROJECT: &str = "sha256:abc";

    fn event(id: &str, event_type: &str, payload: serde_json::Value) -> Event {
        Event {
            event_id: id.to_string(),
            event_type: event_type.to_string(),
            schema_version: SCHEMA_VERSION,
            project_id: PROJECT.to_string(),
            session_id: None,
            occurred_at: "2026-11-18T12:00:00Z".to_string(),
            actor: Actor {
                kind: "agent".to_string(),
                id: None,
            },
            payload,
        }
    }

    fn upsert(id: &str, entity_type: &str, title: &str) -> Event {
        event(
            &format!("e-{id}-{title}"),
            "entity.upserted",
            json!({ "id": id, "entityType": entity_type, "data": { "title": title } }),
        )
    }

    fn model() -> (tempfile::TempDir, ReadModel) {
        let dir = tempfile::tempdir().unwrap();
        let model = ReadModel::open(dir.path().join("project.db")).unwrap();
        (dir, model)
    }

    #[test]
    fn an_upsert_lands_as_one_row_of_its_type() {
        let (_dir, mut model) = model();
        let events = vec![upsert("P1", "plan_item", "first")];

        let report = project(&mut model, PROJECT, &events).unwrap();
        assert_eq!(report.applied, 1);
        assert!(report.changed());
        assert_eq!(model.count_entities(PROJECT, "plan_item").unwrap(), 1);
        assert_eq!(
            model.entity_json("P1").unwrap(),
            Some(json!({ "title": "first" }))
        );
    }

    #[test]
    fn replaying_the_same_events_applies_nothing_twice() {
        let (_dir, mut model) = model();
        let events = vec![upsert("P1", "plan_item", "first")];

        project(&mut model, PROJECT, &events).unwrap();
        let second = project(&mut model, PROJECT, &events).unwrap();

        assert_eq!(second.applied, 0);
        assert_eq!(second.skipped, 1);
        assert_eq!(model.count_entities(PROJECT, "plan_item").unwrap(), 1);
        assert_eq!(
            model.last_event_id(PROJECT).unwrap().as_deref(),
            Some("e-P1-first")
        );
    }

    #[test]
    fn a_later_event_wins_and_bumps_the_version() {
        let (_dir, mut model) = model();
        project(&mut model, PROJECT, &[upsert("P1", "plan_item", "first")]).unwrap();
        project(&mut model, PROJECT, &[upsert("P1", "plan_item", "second")]).unwrap();

        assert_eq!(
            model.entity_json("P1").unwrap(),
            Some(json!({ "title": "second" }))
        );
        assert_eq!(model.count_entities(PROJECT, "plan_item").unwrap(), 1);
    }

    #[test]
    fn a_resumed_pass_continues_from_the_checkpoint() {
        let (_dir, mut model) = model();
        let first = vec![upsert("P1", "plan_item", "a")];
        let second = [upsert("P2", "plan_item", "b")];

        project(&mut model, PROJECT, &first).unwrap();
        // The caller replays the whole log, as a rebuild does: the applied
        // prefix is skipped and only the new event lands.
        let all: Vec<&Event> = first.iter().chain(second.iter()).collect();
        let report = project(&mut model, PROJECT, all).unwrap();

        assert_eq!(report.applied, 1);
        assert_eq!(report.skipped, 1);
        assert_eq!(model.count_entities(PROJECT, "plan_item").unwrap(), 2);
        assert_eq!(
            model.last_event_id(PROJECT).unwrap().as_deref(),
            Some("e-P2-b")
        );
    }

    #[test]
    fn a_dropped_model_is_rebuilt_from_the_log() {
        let dir = tempfile::tempdir().unwrap();
        let log = crate::event::EventLog::at(dir.path().join("events.jsonl"));
        let events = vec![
            upsert("P1", "plan_item", "a"),
            upsert("P2", "plan_item", "b"),
        ];
        for event in &events {
            log.append(event).unwrap();
        }
        let db = dir.path().join("project.db");
        {
            let mut model = ReadModel::open(&db).unwrap();
            project(&mut model, PROJECT, &events).unwrap();
            assert_eq!(model.count_entities(PROJECT, "plan_item").unwrap(), 2);
        }
        std::fs::remove_file(&db).unwrap();

        let mut rebuilt = ReadModel::open(&db).unwrap();
        let report = project(&mut rebuilt, PROJECT, &log.read_all().unwrap()).unwrap();
        assert_eq!(report.applied, 2);
        assert_eq!(rebuilt.count_entities(PROJECT, "plan_item").unwrap(), 2);
    }

    #[test]
    fn a_removal_marks_the_row_and_keeps_the_event() {
        let (_dir, mut model) = model();
        project(&mut model, PROJECT, &[upsert("P1", "plan_item", "a")]).unwrap();
        project(
            &mut model,
            PROJECT,
            &[event("e-del", "entity.removed", json!({ "id": "P1" }))],
        )
        .unwrap();

        assert_eq!(model.count_entities(PROJECT, "plan_item").unwrap(), 0);
        assert_eq!(model.entity_json("P1").unwrap(), None);
        assert_eq!(
            model.last_event_id(PROJECT).unwrap().as_deref(),
            Some("e-del")
        );
    }

    #[test]
    fn a_relation_and_a_reference_land_in_their_own_tables() {
        let (_dir, mut model) = model();
        let events = vec![
            upsert("P1", "plan_item", "a"),
            upsert("E1", "evidence", "b"),
            event(
                "e-rel",
                "relation.added",
                json!({ "id": "r1", "from": "P1", "predicate": "item_has_evidence", "to": "E1" }),
            ),
            event(
                "e-ref",
                "ref.added",
                json!({
                    "id": "f1",
                    "from": "E1",
                    "predicate": "evidence_about_code",
                    "targetDb": "graph",
                    "targetType": "code_ref",
                    "targetKey": "src/lib.rs#foo",
                    "resolver": "code_ref",
                    "status": "pending"
                }),
            ),
        ];

        project(&mut model, PROJECT, &events).unwrap();
        assert_eq!(model.relation_count("item_has_evidence").unwrap(), 1);
        let refs = model.refs_from("E1").unwrap();
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].0, "src/lib.rs#foo");
        assert_eq!(refs[0].1, "pending");
    }

    #[test]
    fn an_unknown_event_type_is_recorded_but_applies_nothing() {
        let (_dir, mut model) = model();
        let events = vec![event("e-future", "future.thing", json!({ "x": 1 }))];

        let report = project(&mut model, PROJECT, &events).unwrap();
        assert_eq!(report.applied, 1);
        assert_eq!(model.count_entities(PROJECT, "plan_item").unwrap(), 0);
        // A second pass must not replay it: the ledger holds it.
        assert_eq!(project(&mut model, PROJECT, &events).unwrap().skipped, 1);
    }

    #[test]
    fn another_projects_events_are_not_applied() {
        let (_dir, mut model) = model();
        let mut alien = upsert("X1", "plan_item", "foreign");
        alien.project_id = "sha256:other".to_string();

        let report = project(&mut model, PROJECT, &[alien]).unwrap();
        assert_eq!(report.applied, 0);
        assert_eq!(model.count_entities(PROJECT, "plan_item").unwrap(), 0);
    }

    #[test]
    fn a_model_from_a_future_version_refuses_to_open() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("project.db");
        {
            let model = ReadModel::open(&db).unwrap();
            model
                .conn()
                .execute(
                    "UPDATE read_model_meta SET v = '99' WHERE k = 'version'",
                    [],
                )
                .unwrap();
        }

        let error = ReadModel::open(&db).unwrap_err().to_string();
        assert!(error.contains("read-model version 99"), "{error}");
    }

    /// The daemon's fallback replays the whole log when an incremental pass
    /// fails. That replay must be a no-op for what landed and keep `seq`
    /// contiguous for what did not, or a resumed pass writes a sequence with a
    /// hole in it.
    #[test]
    fn a_full_replay_after_a_partial_pass_keeps_the_sequence_contiguous() {
        let (_dir, mut model) = model();
        let first = vec![
            upsert("P1", "plan_item", "a"),
            upsert("P2", "plan_item", "b"),
        ];
        assert_eq!(project(&mut model, PROJECT, &first).unwrap().applied, 2);

        // The fallback: everything from the start, plus one new event.
        let mut all: Vec<Event> = first.clone();
        all.push(upsert("P3", "plan_item", "c"));
        let report = project(&mut model, PROJECT, &all).unwrap();
        assert_eq!(report.applied, 1, "only the new event lands");
        assert_eq!(report.skipped, 2, "the applied prefix is skipped");

        // The new event's seq follows the last applied one, with no hole.
        assert_eq!(model.applied_seq("e-P3-c").unwrap(), Some(3));
        assert_eq!(model.count_entities(PROJECT, "plan_item").unwrap(), 3);
    }
}

#[cfg(test)]
mod sync_cost {
    use std::time::Instant;

    use serde_json::json;

    use crate::event::{Actor, Event, EventLog, SCHEMA_VERSION};
    use crate::model::ReadModel;
    use crate::project::project;

    fn event(index: usize) -> Event {
        Event {
            event_id: format!("e-{index}"),
            event_type: "entity.upserted".to_string(),
            schema_version: SCHEMA_VERSION,
            project_id: "proj".to_string(),
            session_id: None,
            occurred_at: "2026-11-18T12:00:00Z".to_string(),
            actor: Actor {
                kind: "agent".to_string(),
                id: None,
            },
            payload: json!({
                "id": format!("P{}", index % 5),
                "entityType": "plan_item",
                "data": { "status": "in_progress", "text": format!("step {index}") }
            }),
        }
    }

    /// One session writing `updates` plan snapshots of five items, syncing after
    /// each the way `ProjectRecorder::sync` does.
    ///
    /// Measured 2026-11-18, this tree. Reading the whole log per sync: 16.1 ms
    /// at 100 events, 20.3 ms at 250, 31.9 ms at 500, 41.7 ms at 1 000 —
    /// quadratic, 8.34 s of replay over 200 updates. From the checkpoint:
    /// 13.6 / 13.3 / 13.4 / 14.3 ms, 2.85 s total. The residue is opening the
    /// model (WAL plus the idempotent schema batch), not replay.
    #[test]
    #[ignore = "measurement, not a gate"]
    fn the_cost_of_syncing_after_every_plan_update() {
        for updates in [20usize, 50, 100, 200] {
            let dir = tempfile::tempdir().unwrap();
            let log = EventLog::at(dir.path().join("events.jsonl"));
            let db = dir.path().join("project.db");
            let start = Instant::now();
            let mut total = 0usize;
            for update in 0..updates {
                for item in 0..5 {
                    log.append(&event(update * 5 + item)).unwrap();
                    total += 1;
                }
                // What `sync` does: read the log from the checkpoint, project,
                // record the new checkpoint.
                let mut model = ReadModel::open(&db).unwrap();
                let offset = model.log_offset().unwrap().unwrap_or(0);
                let (events, end) = log.read_from(offset).unwrap();
                project(&mut model, "proj", &events).unwrap();
                model.set_log_offset(end).unwrap();
            }
            let elapsed = start.elapsed();
            eprintln!(
                "updates={updates:3} events={total:4} syncs={updates:3} total={elapsed:?} per-sync={:?}",
                elapsed.checked_div(u32::try_from(updates).unwrap_or(1))
            );
        }
    }
}
