# pa-jev

Typed-judgment ("System One") wire types and client: three question primitives
with calibrated probabilities.

**FORK-ONLY.**

## Scope

- **Question kinds**: `choice`, `score`, `noul` — the wire vocabulary a judge
  answers in.
- **Client**: a minimal transport of its own, so the crate stays a leaf below
  the session engine rather than pulling `pa-ai` upward.

## Why a leaf crate

The ADR's layering is `pa-jev -> pa-core::jev -> session engine`. Keeping the
transport here means the judgment vocabulary can be used by a consumer that does
not want the provider stack.

## Consumers

- `pa-core::jev` — the `jev` tool, the tool-selection arbiter, and the
  auto-thinking gate.
