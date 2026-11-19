# pa-project

`FORK-ONLY` — never pushed upstream.

The project store: the canonical append-only event log and the derived SQLite
read model.

**FORK-ONLY.**

## Scope

- The event envelope: schema version, idempotent ids, actor, payload.
- The append-only log: append, read all, read from a byte checkpoint.
- The derived read model and the projector that applies the log to it.

## Non-goals

- No session or plan ownership. The plan store owns live plan state; this
  crate records what a project wrote, not what a session is doing now.
- No path resolution policy: project identity is derived by the caller.

## Why a separate crate

`pa-core` owns session behavior and `pa-graph` owns the derived code graph. A
project's durable record outlives both a session and an index rebuild, so it
must not share their ownership or their writer locks. Keeping the metadata and
the event log here also means the derived read model can be rebuilt or dropped
without touching canonical data.

## Consumers

- `pa-daemon` — records a session's plan for a project: `entity.upserted` and
  `entity.removed` for `plan_item` rows, written by `ProjectRecorder`.

Recorded as a gap rather than left implied: **no producer writes evidence or
review events yet.** The crate projects `evidence` entities, `relation.*` edges
and `ref.*` links, and the projector is tested on all of them, but the only
writer in the tree writes plan rows. So the "evidence" and "review" halves of
this record exist as a shape with a test and no data. Adding a producer is the
missing step; until then a reader must not assume a project has any.

## Public API

`Event`, `EventLog`, `Actor`, `SCHEMA_VERSION`, `ReadModel`. The `event`,
`model` and `project` modules are public, so `project()` and
`READ_MODEL_VERSION` are reachable by full path; they are not re-exported at the
crate root because nothing outside the crate reads them yet, and a re-export with
no caller is surface the repo asks to delete rather than keep. Measured
2026-11-19: `ProjectionReport`, `READ_MODEL_VERSION` and `project` had zero
consumers outside this crate.
