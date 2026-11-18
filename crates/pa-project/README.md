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

- `pa-daemon` — records plan, evidence, and review events for a project.
