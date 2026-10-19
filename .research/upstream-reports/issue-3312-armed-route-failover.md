# Issue #3312: Failover switch keeps the armed image-route target — backup retries re-send to the failed provider

**Status:** draft / not submitted (upstream `#3312` is still OPEN). **Type:** bug (retry/failover correctness). **Severity:** P2. **Area:** `crates/pa-daemon/src/agent_engine/turn/model.rs`.

## Summary

When an image-model route is armed, the failover switch re-installs the **route's** target instead of the resolved **backup candidate**, so the "backup" retry goes back to the provider that just failed. Candidate enumeration is derived from the *routed* model (`model.rs:103-105`), and the whole turn keeps serving the route target until the episode ends. The behaviour is upstream-inherited (not a fork regression) and is pinned by a comment and a reload test, so it needs an upstream decision.

## Reproduction

1. Session model is text-only; `imageModel` is set to a model `M` served by ≥2 providers (e.g. `provA/M` and `provB/M`).
2. Send a turn that attaches an image → the episode arms the image route and swaps the serving `provider_target` to `provA/M`.
3. Make `provA` fail with a retryable stream failure so the failover ladder switches to `provB`.
4. Observe: `provider_target.model` is **not** the `provB/M` backup candidate — the switch re-installs the armed route target, and the retry hits `provA` again.

Regression assertion to add: *armed route + scripted primary failure → `provider_target.model` == backup candidate (`provB/M`)*.

## Evidence (both eras)

| Era | File:line | Code |
|---|---|---|
| TS `v0.9.8` | `packages/coding-agent/src/core/agent-session.ts:13347` | `if (backupModel && !modelsAreEqual(this._runModel(), backupModel))` — eligibility compares against the **mutable** `_runModel()` (the routed image model), comment `:13339-13344` shows partial awareness ("a backup equal to a routed turn's image model is recognized as the duplicate it is") |
| Rust `v0.10.x` / `main` | `crates/pa-daemon/src/agent_engine/turn/model.rs:367` | `if !self.install_armed_route_target() { … install backup target … }` — the backup install block (`:368-381`) is skipped whenever a route is armed |
| Rust | `model.rs:103-105` | `Some(route) => self.failover_candidates(&route.target.model)` — candidates derived from the routed model |
| Rust | comment `model.rs:363-366` | "A routed episode keeps serving the route's target across the failover switch" — documents the current behaviour |
| Rust | `crates/pa-daemon/src/agent_engine/turn/reload.rs:299+` | test `reload_refreshes_the_armed_image_route_target_auth` pins the current semantics |

Blame: `model.rs:363-367` is upstream (`7fa55b6b95` 2026-10-06 comment, `eba4a6504`/`#3393` 2026-10-13, both ≤ fork branch-point 2026-10-13) — **upstream inheritance, not a fork regression**. Upstream `#3312` has been OPEN since 2026-10-02 with no fix.

## Proposed fix

Two independent parts.

**A. Upstream `Solution` (from the body of #3312).** A switch inside a routed episode must install `next` (route-aware: keep the image override, change provider), honour the provider's `Retry-After`, and emit events that name the real recipient.

**B. Third-party TS fix — `thomaswillner/prime-agent` PR #24** (OPEN, not merged; base `release/v0.9.8` = tag `7d442aafa`), which the port should mirror:

1. Eligibility must compare against the model that **served the failed message** (`message.provider/model`), not the current selection.
2. A session that has already left the failed route re-issues the turn immediately (`auto_retry_start.reason = "selected"`); selection outranks an earlier backup hop; nothing is restored afterwards.
3. Each zero-delay re-route spends one `retry.maxRetries` attempt (budget checked **before** the re-route).
4. +4 RED tests in `agent-session-retry-events.test.ts`.

Key fragment for the Rust port (mirror of part A) — the switch callback at `model.rs:367` must install the backup candidate for a routed episode too, instead of short-circuiting on the armed route:

```rust
// before: if !self.install_armed_route_target() { /* install backup target */ }
let (api_key, headers) = self.resolve_request_key_and_headers(&next);
let mut target = self.provider_target.write().expect("provider target lock");
*target = Some(ProviderTarget { service_tier: …, api_key, model: next.clone(), headers });
// the route's image override survives in the per-run route; only the provider changes
```

The Rust port already dropped the unbounded part of the TS bug (`MAX_TOTAL_PROVIDER_RETRIES` is checked before each switch), so what remains is the attribution/serving-target issue.

## Impact

- Every routed image episode loses failover: backup retries re-hit the failed provider, burning latency and quota, and the failover ladder is defeated until the episode ends.
- Exposure is narrow but real: requires an image-model override **and** the same image model configured on ≥2 providers.
- Fixing in the fork alone is a parity divergence (the current behaviour is pinned by a comment + reload test) → coordinate with upstream `#3312`.

## References

- Upstream `#3312` (OPEN) — body contains the `Solution`.
- `thomaswillner/prime-agent` PR #24 (OPEN, unmerged) — the four-point recipe + RED tests.
- Fork research note: `.research/verify-ts-era.md` §B2, `.research/verify-upstream-wire.md` #38.
