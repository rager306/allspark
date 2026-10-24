# Model discovery contract

Status: **proposal**, one open decision (§6). Written 2026-10-24 against `0c24251d5`.
Scope: how a provider's model list and each model's properties are discovered, cached,
and retired. Today the catalog is a hand-maintained Rust mirror
(`crates/pa-ai/src/models_generated.rs`) plus a hand-written fork list
(`crates/pa-ai/src/models_fork.rs`); `crates/pa-models/src/opencode_models.rs` already
does the live thing for one provider.

## 1. Why this exists

Two problems, both measured:

- **A compiled catalog goes stale silently.** `models_generated.rs` mirrors upstream's TS
  object literal and there is no generator; a provider that adds or retires a model is
  invisible until someone edits Rust. For `opencode` we stopped doing that.
- **Discovery and capability are different questions.** Whether an id exists is what the
  provider answers; what its context window is may come from `models.dev`, from the
  provider's own metadata, or be a guess. Conflating them is how a removed model
  becomes a broken config rather than a warning.

## 2. Layers

**Discovery** — `list_models()` per provider. A provider with a list endpoint (`opencode`
`/inference/v1/models`, `ninerouter`) answers live; one without (`muse`, `typesafe`)
declares a static list. The live list is the only authority on EXISTENCE.

**Properties** — never merged blindly; resolved by precedence and tagged:

| precedence | source | tag |
|---|---|---|
| 1 | measured on this install (a probe, a live header) | `Measured` |
| 2 | the provider's own model metadata | `ProviderDeclared` |
| 3 | `models.dev` entry, as a delta against the hand table | `CatalogDelta` |
| 4 | inferred from the id's family | `Estimated` |

`opencode_models.rs` implements rows 1 and 3-4 today and proves the shape works: 85 ids,
9 free, 7 measured, exactly one `jev-1.13-free` estimated.

**Existence outranks property.** A model the endpoint serves but nothing describes is
kept as `Estimated`, never dropped — dropping it turns a working selection into
`NotFound`.

## 3. Freshness and cost

A refresh is a conditional GET: the previous `ETag` goes back as `If-None-Match`, and an
unchanged catalog costs 0 bytes (this is what 9router does, and what `opencode_models.rs`
copies). The snapshot persists with:
- `etag` per source,
- `fetched_at`,
- the resolved properties and each one's tag.

Offline, the last known snapshot is used and every row is marked stale. A session must
never fail to start because a refresh failed.

## 4. Free-ness is a discovery fact, not a name pattern

`opencode` serves `big-pickle` for free with no `-free` suffix, so free-ness is read from
the live list, never from the id. The same rule should apply to every provider that can
answer it; a suffix heuristic is a fallback, tagged `Estimated`.

## 5. What the catalog must NOT do

- **Never** make existence depend on a hand table. A hand table is a hint for properties.
- **Never** invent a credential. `get_available` stays narrow (a test pins that native
  free models do not become failover candidates); discovery has its own predicate
  (`get_discoverable_models`).
- **Never** let a refresh failure reach a paint or a turn start (AGENTS.md, "the critical
  path").

## 6. OPEN: what happens to a model that was listed and is gone

This is the decision this document exists to surface. Three options:

| option | behaviour | cost |
|---|---|---|
| **hide** | absent from `model list` and the picker; still resolvable by exact id | a saved config keeps working, but the user cannot see why the model vanished |
| **deprecate** (recommended) | still listed, still resolvable, marked deprecated with the date it disappeared | one flag through resolution; nothing breaks |
| **delete** | gone from resolution | a saved `settings.json` naming it stops the session from starting |

Today a switch to an unknown model fails with `SetModelSelectionError::NotFound`
(`crates/pa-core/src/models/registry.rs:57`), but a *saved* selection that disappears is
worse than an error at switch time: the resolution happens at start-up.

**Recommendation: deprecate.** A live list is the provider's opinion of what it sells
today, not a statement that yesterday's selection is invalid. Retiring it hard makes one
provider's catalogue edit break every config that named the model.

Pruning should exist only as an explicit command (`model prune`) that names the affected
configs first.

## 7. Shape of the change (when approved)

1. Lift `MetadataSource` and `LiveModel` out of `pa-models/src/opencode_models.rs` into a
   provider-neutral module; `opencode` becomes one implementation.
2. Extend `MetadataSource` with `ProviderDeclared`, `CatalogDelta`, and `Deprecated`.
3. A `ModelDiscovery` trait per provider: `list() -> Vec<LiveModel>` plus the ETag and
   TTL policy. Static providers implement it with their table.
4. `get_discoverable_models` merges live lists over the static catalog, keeping deprecated
   rows.
5. One test per rule in §2-§6, each red-first against the current behaviour.
