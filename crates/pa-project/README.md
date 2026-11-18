# pa-project

`FORK-ONLY` — never pushed upstream.

Metadata-driven project store: the `meta/` definitions, the canonical
append-only event log, and the derived SQLite read model.

**FORK-ONLY.**

## Scope

- Load and validate the human-authored `meta/` YAML: entities, relations,
  dictionaries, projections, policies.
- The event envelope: schema version, idempotent ids, actor, payload.
- Compose and parse the event log.

## Non-goals

- No SQLite in this crate yet: the read model is a later layer, so the
  metadata and the canonical log stay testable without a database.
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
