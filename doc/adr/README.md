# ADRs — Prime Agent (allspark fork) architectural decisions

## Format

Every ADR is **MADR-lite with YAML front matter**:

```yaml
---
id: ADR-NNNN
title: <one line>
status: Proposed
lifecycle: "[bounded]"
date: YYYY-MM-DD
supersedes: none
related: []
---
```

Mandatory front-matter fields:

- `id` — `ADR-NNNN`, zero-padded, matching the filename prefix.
- `title` — one line, repeated as the first body heading.
- `status` — `Proposed`, `Accepted`, or `Superseded`.
- `lifecycle` — exactly one tag from the table below, on the record's core claim.
- `date` — `YYYY-MM-DD`.
- `supersedes` — `none` or the id(s) of the record(s) it replaces.
- `related` — list of related ADR ids; may be empty.

Lifecycle tags, mandatory on every architectural or state claim in the body:

| tag | meaning |
|---|---|
| `[validated]` | measured, reproducible |
| `[bounded]` | implemented, scope stated |
| `[smoke]` | local, skip-capable, not promotion proof |
| `[proposed]` | design only |
| `[deferred]` | decided against, for now |

Supersession metadata uses the canonical keys `supersedes` and `superseded_by`.
The misspelling `superseds` is not accepted on any ADR.

## Citation hygiene

Active ADRs must not cite `.research/` paths as durable authority — `.research/`
is gitignored and does not survive a clone. Research reports an ADR is built
from live as tracked, verbatim copies under
[`doc/research/`](../research/INDEX.md); cite those, or the ADR corpus itself.

## Current ADRs

| id | title | status | lifecycle |
|---|---|---|---|
| [ADR-0001](0001-multi-dimensional-test-contract.md) | Multi-dimensional test contract with a machine-checked coverage governor | Proposed | [bounded] |
