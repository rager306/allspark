# New issue: Provider calls carry no timeout when a cancel signal is present — a stalled peer hangs the turn forever

**Status:** draft / not submitted. **Type:** bug (reliability / hang). **Severity:** P1. **Area:** `crates/pa-ai/src/utils_inner/http.rs` (+ `crates/pa-core/src/session_engine/provider_adapter.rs`).

## Summary

`send()` dispatches on `match (&signal, request.timeout_ms)`. The `(Some(signal), _)` arm **discards `timeout_ms`** with a wildcard and only races the abort signal — so when a cancel signal is present (every session turn), the request has **no deadline at all**. Worse, the adapter sends `timeout_ms: None` for session turns, so even the timeout arm is unreachable. SSE body reads (`next_text`) race `select(cancel, chunk)` with **no idle bound**, so a mid-stream black hole hangs until a user abort. The bedrock h2 transport already does this correctly (`timeout(select_cancel(...))`), so the shared http path is the outlier. We hit three live hangs in a single day on our machine.

## Reproduction

1. Start a session turn (a cancel signal is always attached).
2. Point a provider at a peer that accepts the TCP connection but never sends response headers (or sends headers then stalls mid-SSE).
3. The turn hangs indefinitely — `timeout_ms` is dropped by the `(Some(signal), _)` arm, and the adapter sent `None` anyway. Only a manual abort ends it.

## Evidence (both eras)

| Era | File:line | Code |
|---|---|---|
| Rust `v0.10.x` / `main` | `crates/pa-ai/src/utils_inner/http.rs:253-261` | `match (&signal, request.timeout_ms) { (Some(signal), _) => { select!(cancelled, send_future) } … }` — `timeout_ms` discarded when a signal is present; the `(None, Some(_))` timeout arm is unreachable |
| Rust | `crates/pa-core/src/session_engine/provider_adapter.rs:180` | `timeout_ms: None` — session turns always take the first arm |
| Rust | `crates/pa-ai/src/utils_inner/http.rs:67-86` | `next_text`: `select!(signal.cancelled(), self.body.chunk())` — no idle/chunk deadline for a stalled stream |
| Rust | `crates/pa-ai/src/utils_inner/http.rs:17-41` | clients set only `http1_only()` + `pool_idle_timeout(90s)` (connection pool, not a request timeout); `tls.rs:35-43` adds only root certs — no `.timeout()`/`.connect_timeout()` |
| Rust (correct pattern) | `crates/pa-ai/src/providers/bedrock/h2.rs:281-287` | `match timeout_ms { Some(t) => tokio::time::timeout(t, select_cancel(request_future, outer_signal)) … }` — the timeout wraps the cancel race |
| TS `v0.9.8` | `packages/ai/src/providers/anthropic.ts:530-531` | `...(options?.signal ? { signal } : {}), ...(options?.timeoutMs !== undefined ? { timeout: options.timeoutMs } : {})` — the TS SDK passes **both** signal and timeout to fetch, i.e. the two are independent |

## Proposed fix

Make the timeout and the abort signal independent bounds: race the signal against the request, then wrap the **whole** wait in the timeout — the bedrock `h2.rs` shape. And give session turns a finite request timeout.

Key fragment (`utils_inner/http.rs`):

```rust
let send_or_abort = async {
    match &signal {
        Some(signal) => tokio::select! {
            () = signal.cancelled() => Err(ProviderError::Aborted),
            result = send_future => Ok(result),
        },
        None => Ok(send_future.await),
    }
};
let response = match request.timeout_ms {
    Some(timeout_ms) => match tokio::time::timeout(Duration::from_millis(timeout_ms), send_or_abort).await {
        Ok(result) => result?,
        Err(_) => return Err(ProviderError::Connection(ProviderConnectionError {
            kind: ConnectionErrorKind::Timeout, profile: request.connection.clone(),
            cause: format!("request exceeded the {timeout_ms}ms timeout"),
        })),
    },
    None => send_or_abort.await?,
};
```

And in the adapter (`provider_adapter.rs`):

```rust
pub const PROVIDER_REQUEST_TIMEOUT_MS: u64 = 300_000;   // 5 min: bounds time-to-first-byte, not a long stream body
…
timeout_ms: Some(PROVIDER_REQUEST_TIMEOUT_MS),          // was: None
```

Tests: `a_signal_present_request_still_times_out_on_a_stalled_peer` (mock server hangs after connect; assert a `Connection/Timeout` error and that the signal was **not** fired), and `every_stream_attempt_carries_a_bounded_request_timeout` (adapter observes `Some(PROVIDER_REQUEST_TIMEOUT_MS)`). A follow-up should add an idle deadline to `next_text` for the mid-stream black-hole case.

## Impact

- A connected-but-silent provider (or a black hole mid-stream) hangs the turn **forever** on every session call — no retry ladder, no failover, no user-visible error until a manual abort.
- Three live hangs observed in one day on our machine.
- The divergence is internal: the bedrock transport already implements the correct shape, so the fix is a known-good pattern.

## References

- `.research/verify-reliability.md` #8.
- Correct pattern: `crates/pa-ai/src/providers/bedrock/h2.rs:118-135, 281-287`.
- TS reference: `git show v0.9.8:packages/ai/src/providers/anthropic.ts` (lines 530-531).
