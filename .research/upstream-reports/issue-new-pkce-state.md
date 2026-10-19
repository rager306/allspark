# New issue: Anthropic OAuth uses the PKCE `code_verifier` as the OAuth `state` — the verifier rides the authorization URL

**Status:** draft / not submitted. **Type:** security (CWE-319 / broken PKCE). **Severity:** P1 formally, P2 in practice. **Area:** `crates/pa-ai/src/oauth/anthropic.rs` (+ `oauth/pkce.rs`).

## Summary

The Anthropic login flow sets `state = verifier` — the PKCE `code_verifier` is sent as the OAuth `state` on the authorization URL and is echoed back on the redirect URL next to the `code`. Anyone who observes the redirect (browser history, referrer, logs, a pasted redirect URL) then holds both the `code` **and** the `code_verifier` and can complete the token exchange: PKCE is defeated. Anthropic's own Claude Code client mints a **fresh, independent** `state` (never the verifier). The current behaviour is an exact port of TS v0.9.8 and is still identical in upstream `main`.

## Reproduction

1. Run the Anthropic OAuth login.
2. Inspect the authorization URL: the `state` parameter **equals** the PKCE `code_verifier` (and `code_challenge = base64url(sha256(verifier))`, so the challenge and the state share one secret).
3. Inspect the redirect URL: it carries `?code=…&state=…` where `state == verifier`.
4. A party that captures the redirect can replay the token exchange with `code_verifier = state`.

## Evidence (both eras)

| Era | File:line | Code |
|---|---|---|
| TS `v0.9.8` | `packages/ai/src/utils/oauth/anthropic.ts:252` | `state: verifier` in `authParams` (with `code_challenge: challenge`, `code_challenge_method: "S256"`) |
| TS `v0.9.8` | `…/oauth/anthropic.ts` (callback + paste) | every state check compares the echoed value to the verifier |
| Rust `v0.10.x` / `main` | `crates/pa-ai/src/oauth/anthropic.rs:120` | `("state", verifier)` in `authorization_url`; module doc says the flow "doubles its PKCE verifier as the OAuth `state` — kept for wire parity" |
| Rust | `anthropic.rs:56-57` | PKCE pair generated; callback server started with the verifier as its expected state |
| Rust | `anthropic.rs:582-583` (test) | `// TS doubles the PKCE verifier as the state.` `assert_eq!(param("state"), "the-verifier")` — the weakness is pinned by a test |
| Sibling flow | `crates/pa-ai/src/oauth/openai_codex.rs:78, 218-224` | codex already mints an independent `create_state()` — the correct pattern is in the same crate |

**Independent confirmation — Claude Code v2.1.296** (binary `/root/.local/share/claude/versions/2.1.296`, not stripped). Class `IU` (the Anthropic OAuth client) does:

```js
constructor(){ this.codeVerifier = A() }          // A() = base64url(randomBytes(32))
async startOAuthFlow(){ let r = S(this.codeVerifier),   // S() = base64url(sha256(x))  → code_challenge
    o = w(),                                        // w() = base64url(randomBytes(32)) → INDEPENDENT state
    n = { codeChallenge: r, state: o, ... };
  ... let b = await this.waitForAuthorizationCode(o, ...);
  let a = await eho(b, { state: o, codeVerifier: this.codeVerifier, ... }) }
```

and the callback listener (`class u`) verifies `rejectOnStateMismatch(e, t){ if (e === this.expectedState) return false; … "Invalid state parameter" }` — i.e. `state` is a CSRF token, independent of the verifier; the verifier only rides the **token-exchange** body. So the `state == verifier` constraint the comment cites as "wire parity" does **not** exist in Anthropic's own client.

## Proposed fix

Mint a fresh random CSRF `state` and keep the verifier out of the URL. Key fragment (`oauth/pkce.rs` + `oauth/anthropic.rs`):

```rust
// pkce.rs
pub fn generate_state() -> String { base64url(&random_bytes(32)) }   // 43 chars, URL-safe

// anthropic.rs
let state = generate_state();
let server = AnthropicCallbackServer::start(&state)?;                 // was: &verifier
login_with_server(http, ui, &server, &verifier, &challenge, &state).await
// authorization_url(challenge, state) → ("state", state); exchange still sends code_verifier = verifier
```

Test `the_verifier_never_rides_the_authorization_url`: assert `param("state") != verifier`, `param("state") == state`, `!url.contains(verifier)`, and that the token-exchange body still sends `code_verifier == verifier` and echoes `state`. The existing test that pins `state == verifier` is replaced. This is a deliberate parity divergence — the fix belongs upstream (or as a documented parity exception).

## Impact

- **PKCE is neutralised for the Anthropic login:** the verifier leaks into the authorization URL (browser history) and the redirect URL next to the code; whoever captures the redirect can exchange the code.
- Affects both eras identically (TS v0.9.8 == upstream `main`).
- Fix is trivial and mirrors the sibling codex flow; the only reason it wasn't done is the "wire parity" comment, which Anthropic's own client contradicts.

## References

- `.research/verify-security.md` #6; `.research/verify-ts-era.md` §C (#6).
- Claude Code binary evidence: `/root/.local/share/claude/versions/2.1.296` (class `IU`, `state` = independent `base64url(randomBytes(32))`).
- TS reference: `git show v0.9.8:packages/ai/src/utils/oauth/anthropic.ts` (line 252).
